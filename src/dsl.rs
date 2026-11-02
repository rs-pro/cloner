use anyhow::{Context, Result, anyhow, bail};
use rquickjs::prelude::Rest;
use rquickjs::{Context as JsContext, Ctx, Function, Object, Runtime, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use crate::backup::{self, Manifest, ManifestDatabase, ManifestFile, ManifestRemote};
use crate::compose::{detect_service, engine_from_env, infer_db, parse_compose_services};
use crate::config::{Config, Resolved, ResolvedDb, ResolvedDbConfig, ResolvedFileSpec};
use crate::db;
use crate::k8s::KubernetesTarget;
use crate::ssh::Ssh;
use crate::sync::{self, SyncStats};
use crate::util::{parse_env, read_env_file, resolve_path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunMode {
    Clone,
    Backup,
    Validate,
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub skip_database: bool,
    pub skip_files: bool,
    pub dry_run: bool,
    pub mode: RunMode,
    pub backup_dir: Option<PathBuf>,
    pub from: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            skip_database: false,
            skip_files: false,
            dry_run: false,
            mode: RunMode::Clone,
            backup_dir: None,
            from: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LocalContext {
    pub resolved: Resolved,
    pub db: ResolvedDb,
}

/// Host services available to DSL extensions.
///
/// Extensions are installed before the config module is evaluated, so they can
/// provide variables, namespaces and functions that `dl.js` uses at load time
/// (for example `rsbackup.serverUrl` or `postgres.databases()`).
pub struct DslEnvironment {
    pub config_path: PathBuf,
    pub project_root: PathBuf,
    pub mode: RunMode,
    pub dry_run: bool,
    pub from: Option<String>,
    pub runner: Rc<dyn DslCommandRunner>,
}

impl DslEnvironment {
    fn new(
        config_path: &Path,
        mode: RunMode,
        dry_run: bool,
        from: Option<&str>,
        runner: Rc<dyn DslCommandRunner>,
    ) -> Self {
        let project_root = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self {
            config_path: config_path.to_path_buf(),
            project_root,
            mode,
            dry_run,
            from: from.map(str::to_string),
            runner,
        }
    }
}

/// Run local/remote commands from an extension without depending on the
/// Cloner runtime state. Closures are evaluated lazily, so an extension may be
/// installed before the DSL config has been exported.
pub trait DslCommandRunner {
    fn run_local(&self, command: &str) -> Result<String>;
    fn run_remote(&self, command: &str) -> Result<String>;
}

struct StateRunner {
    state: Rc<RefCell<Option<RuntimeState>>>,
    handle: tokio::runtime::Handle,
}

impl DslCommandRunner for StateRunner {
    fn run_local(&self, command: &str) -> Result<String> {
        let cwd = {
            let guard = self.state.borrow();
            guard
                .as_ref()
                .map(|st| st.resolved.project_root.clone())
                .ok_or_else(|| anyhow!("config is not loaded yet"))?
        };
        let output = crate::shell::run_capture(command, Some(&cwd), &[], None)?;
        if output.code != 0 {
            bail!(
                "local command failed with exit code {}: {command}\n{}",
                output.code,
                output.stderr.trim()
            );
        }
        Ok(output.stdout)
    }

    fn run_remote(&self, command: &str) -> Result<String> {
        let mut guard = self.state.borrow_mut();
        let st = guard
            .as_mut()
            .ok_or_else(|| anyhow!("config is not loaded yet"))?;
        let ssh = st.ssh_mut(&self.handle)?;
        let output = self.handle.block_on(ssh.exec(command))?;
        if output.code != 0 {
            bail!(
                "remote command failed with exit code {}: {command}\n{}",
                output.code,
                output.stderr.trim()
            );
        }
        Ok(output.stdout)
    }
}

struct UnavailableRunner;

impl DslCommandRunner for UnavailableRunner {
    fn run_local(&self, _command: &str) -> Result<String> {
        bail!("local commands are not available while evaluating only the config")
    }

    fn run_remote(&self, _command: &str) -> Result<String> {
        bail!("remote commands are not available while evaluating only the config")
    }
}

/// Extension point for products built on Cloner (for example RSBackup).
///
/// An extension can add globals, variables, namespaces and functions. It can
/// also declare extra hook names; those hooks are called after `afterAll` when
/// the default pipeline runs. A config-level `execute()` can call the extension
/// functions directly instead.
pub trait DslExtension {
    fn name(&self) -> &'static str;

    fn install<'js>(
        &self,
        ctx: &Ctx<'js>,
        globals: &Object<'js>,
        env: &DslEnvironment,
    ) -> Result<()>;

    /// Extra hook names implemented in the DSL `hooks` object.
    fn hooks(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Fluent entry point for embedding the Cloner DSL.
#[derive(Default)]
pub struct DslBuilder {
    options: RunOptions,
    extensions: Vec<Rc<dyn DslExtension>>,
}

impl DslBuilder {
    pub fn new(options: RunOptions) -> Self {
        Self {
            options,
            extensions: Vec::new(),
        }
    }

    pub fn with_extension(mut self, extension: impl DslExtension + 'static) -> Self {
        self.extensions.push(Rc::new(extension));
        self
    }

    pub fn eval_config(&self, config_path: &Path) -> Result<Config> {
        eval_config_with_extensions(config_path, &self.extensions)
    }

    pub fn execute(&self, config_path: &Path) -> Result<()> {
        execute_with_extensions(config_path, self.options.clone(), &self.extensions)
    }
}

fn install_extensions<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    env: &DslEnvironment,
    extensions: &[Rc<dyn DslExtension>],
) -> Result<()> {
    for extension in extensions {
        extension
            .install(ctx, globals, env)
            .with_context(|| format!("install DSL extension {}", extension.name()))?;
    }
    Ok(())
}

fn run_extension_hooks<'js>(
    ctx: &Ctx<'js>,
    exports: &Object<'js>,
    extensions: &[Rc<dyn DslExtension>],
) -> Result<()> {
    for extension in extensions {
        for hook in extension.hooks() {
            call_hook(ctx, exports, hook)?;
        }
    }
    Ok(())
}

/// Evaluate only the exported config. Hooks are not run and no remote
/// connection is made; this is used by `cloner db ...` and `validate`.
pub fn eval_config(config_path: &Path) -> Result<Config> {
    eval_config_with_extensions(config_path, &[])
}

pub fn eval_config_with_extensions(
    config_path: &Path,
    extensions: &[Rc<dyn DslExtension>],
) -> Result<Config> {
    let source = std::fs::read_to_string(config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
    let canonical = crate::util::clean_path(
        config_path
            .canonicalize()
            .unwrap_or_else(|_| config_path.to_path_buf()),
    );
    let js_runtime = Runtime::new().context("create QuickJS runtime")?;
    let ctx = JsContext::full(&js_runtime).context("create QuickJS context")?;
    ctx.with(|ctx| -> Result<Config> {
        let globals = ctx.globals();
        globals.set(
            "__dirname",
            canonical
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .into_owned(),
        )?;
        globals.set("__filename", canonical.to_string_lossy().into_owned())?;
        let module = Object::new(ctx.clone())?;
        let exports = Object::new(ctx.clone())?;
        module.set("exports", exports.clone())?;
        globals.set("module", module)?;
        globals.set("exports", exports)?;
        let process = Object::new(ctx.clone())?;
        let env = Object::new(ctx.clone())?;
        for (key, value) in std::env::vars() {
            env.set(key, value)?;
        }
        process.set("env", env)?;
        globals.set("process", process)?;
        globals.set("__cloner_log", Function::new(ctx.clone(), |_: String| {})?)?;
        ctx.eval::<(), _>(PRELUDE)
            .map_err(|e| anyhow!("initialize DSL runtime: {e}"))?;
        let env = DslEnvironment::new(
            &canonical,
            RunMode::Validate,
            true,
            None,
            Rc::new(UnavailableRunner),
        );
        install_extensions(&ctx, &globals, &env, extensions)?;
        ctx.eval::<(), _>(source.as_str())
            .map_err(|e| anyhow!("evaluate {}: {e}", canonical.display()))?;

        let module: Object = ctx.globals().get("module")?;
        let mut exports: Value = module.get("exports")?;
        if exports.is_function() {
            let function = exports
                .clone()
                .into_function()
                .ok_or_else(|| anyhow!("module.exports is not a function"))?;
            exports = function.call::<_, Value>(())?;
        }
        if !exports.is_object() {
            bail!(
                "{} must export a config object (module.exports = {{ ... }})",
                canonical.display()
            );
        }
        let exports = exports
            .into_object()
            .ok_or_else(|| anyhow!("module.exports is not an object"))?;
        let json_object: Object = ctx.globals().get("JSON")?;
        let stringify: Function = json_object.get("stringify")?;
        let text: String = stringify.call((exports,))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parse config exported by {}", canonical.display()))
    })
}

/// Resolve the local database described by a DSL file without contacting the
/// remote host. Used by pg_dev-style `cloner db` commands.
pub fn load_local_context(config_path: &Path) -> Result<LocalContext> {
    let config = eval_config(config_path)?;
    let resolved = config.resolve(config_path)?;
    let db_cfg = resolved
        .database
        .as_ref()
        .ok_or_else(|| anyhow!("{} does not configure a database", config_path.display()))?;
    let app_env = resolved
        .local
        .env_file
        .as_ref()
        .map(|path| read_env_file(path))
        .unwrap_or_default();
    let db = resolve_local_db(&resolved, db_cfg, &app_env)?;
    Ok(LocalContext { resolved, db })
}

struct RuntimeState {
    resolved: Resolved,
    ssh: Option<Ssh>,
    remote_db: Option<ResolvedDb>,
    local_db: Option<ResolvedDb>,
    remote_env: BTreeMap<String, String>,
    local_env: BTreeMap<String, String>,
    options: RunOptions,
    file_stats: Vec<(String, PathBuf, SyncStats)>,
    /// Only set for automatically timestamped backup directories; explicit
    /// `--to` destinations are never pruned without the user asking.
    backup_prune_root: Option<PathBuf>,
}

impl RuntimeState {
    fn ssh_mut(&mut self, handle: &tokio::runtime::Handle) -> Result<&mut Ssh> {
        if self.ssh.is_none() {
            let cfg = self
                .resolved
                .remote
                .as_ref()
                .ok_or_else(|| anyhow!("remote is not configured"))?;
            self.ssh = Some(handle.block_on(Ssh::connect(cfg))?);
        }
        Ok(self.ssh.as_mut().unwrap())
    }
}

const PRELUDE: &str = r#"
globalThis.defineConfig = (value) => value;
globalThis.dirname = (p) => {
  const s = String(p).replace(/[\\/]+$/, '');
  if (!s) return '/';
  const i = s.search(/[\\/][^\\/]*$/);
  if (i < 0) return '.';
  return s.slice(0, i).replace(/[\\/]+$/, '') || '/';
};
globalThis.basename = (p) => {
  const parts = String(p).replace(/[\\/]+$/, '').split(/[\\/]/);
  return parts[parts.length - 1];
};
globalThis.console = {
  log: (...args) => __cloner_log(args.map(fmt).join(' ')),
  info: (...args) => __cloner_log(args.map(fmt).join(' ')),
  warn: (...args) => __cloner_log(args.map(fmt).join(' ')),
  error: (...args) => __cloner_log(args.map(fmt).join(' ')),
};
function fmt(value) {
  if (typeof value === 'string') return value;
  try { return JSON.stringify(value); } catch (_) { return String(value); }
}
"#;

pub fn execute(config_path: &Path, options: RunOptions) -> Result<()> {
    execute_with_extensions(config_path, options, &[])
}

pub fn execute_with_extensions(
    config_path: &Path,
    options: RunOptions,
    extensions: &[Rc<dyn DslExtension>],
) -> Result<()> {
    let source = std::fs::read_to_string(config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
    let canonical = crate::util::clean_path(
        config_path
            .canonicalize()
            .unwrap_or_else(|_| config_path.to_path_buf()),
    );

    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create tokio runtime")?;
    let handle = tokio_runtime.handle().clone();

    let js_runtime = Runtime::new().context("create QuickJS runtime")?;
    let ctx = JsContext::full(&js_runtime).context("create QuickJS context")?;
    let state: Rc<RefCell<Option<RuntimeState>>> = Rc::new(RefCell::new(None));

    ctx.with(|ctx| -> Result<()> {
        install_globals(&ctx, &state, &handle, &canonical, options.from.as_deref())?;

        ctx.eval::<(), _>(PRELUDE)
            .map_err(|e| anyhow!("initialize DSL runtime: {e}"))?;
        let runner = Rc::new(StateRunner {
            state: state.clone(),
            handle: handle.clone(),
        });
        let env = DslEnvironment::new(
            &canonical,
            options.mode,
            options.dry_run,
            options.from.as_deref(),
            runner,
        );
        install_extensions(&ctx, &ctx.globals(), &env, extensions)?;
        ctx.eval::<(), _>(source.as_str())
            .map_err(|e| anyhow!("evaluate {}: {e}", canonical.display()))?;

        let module: Object = ctx.globals().get("module").context("read module")?;
        let mut exports: Value = module.get("exports").context("read module.exports")?;
        if exports.is_function() {
            let function = exports
                .clone()
                .into_function()
                .ok_or_else(|| anyhow!("module.exports is not a function"))?;
            exports = function
                .call::<_, Value>(())
                .context("call module.exports()")?;
        }
        if !exports.is_object() {
            bail!(
                "{} must export a config object (module.exports = {{ ... }})",
                canonical.display()
            );
        }
        let exports = exports
            .into_object()
            .ok_or_else(|| anyhow!("module.exports is not an object"))?;

        let json_object: Object = ctx.globals().get("JSON").context("read JSON")?;
        let stringify: Function = json_object.get("stringify").context("JSON.stringify")?;
        let text: String = stringify
            .call((exports.clone(),))
            .context("serialize exported config")?;
        let config: Config = serde_json::from_str(&text)
            .with_context(|| format!("parse config exported by {}", canonical.display()))?;
        let resolved = config.resolve(&canonical)?;

        let mut runtime_state = build_state(&resolved, &handle, options.clone())?;
        if options.mode == RunMode::Backup {
            let configured = resolved
                .backup
                .destination
                .as_deref()
                .map(|p| resolve_path(&resolved.project_root, p));
            let dir = backup::resolve_backup_dir(
                options.backup_dir.as_deref(),
                configured.as_deref(),
                &resolved.project_root,
                resolved.backup.keep,
            )?;
            backup::prepare_dir(&dir)?;
            runtime_state.backup_prune_root = if options.backup_dir.is_none()
                && resolved.backup.keep.is_some_and(|keep| keep > 0)
            {
                dir.parent().map(Path::to_path_buf)
            } else {
                None
            };
            runtime_state.options.backup_dir = Some(dir);
        }
        *state.borrow_mut() = Some(runtime_state);

        if options.mode == RunMode::Validate {
            let guard = state.borrow();
            let st = guard.as_ref().unwrap();
            let text = serde_json::to_string_pretty(&crate::config::describe(
                &st.resolved,
                st.remote_db.as_ref(),
                st.local_db.as_ref(),
            ))?;
            println!("{text}");
            return Ok(());
        }

        // A user-supplied execute() replaces the whole default action. The
        // default clone steps are still available as clone_db()/clone_files().
        let execute_hook: Value = exports
            .get("execute")
            .unwrap_or(Value::new_undefined(ctx.clone()));
        if execute_hook.is_function() {
            log::info!("running custom execute() action");
            let function = execute_hook.into_function().unwrap();
            let result: Value = match function.call::<_, Value>(()) {
                Ok(result) => result,
                Err(error) => {
                    let caught = ctx.catch();
                    let detail = caught
                        .as_exception()
                        .map(|exception| exception.message().unwrap_or_default())
                        .unwrap_or_else(|| format!("{caught:?}"));
                    let stack = caught
                        .as_exception()
                        .and_then(|exception| exception.stack())
                        .unwrap_or_default();
                    return Err(anyhow!(
                        "execute() failed: {detail}\n{stack}\n(quickjs: {error})"
                    ));
                }
            };
            if result.is_promise() {
                bail!("execute() returned a Promise; execute must be synchronous");
            }
            if options.mode == RunMode::Backup {
                write_manifest(&state)?;
            }
            run_extension_hooks(&ctx, &exports, extensions)?;
            return Ok(());
        }

        call_hook(&ctx, &exports, "beforeAll")?;
        if !options.skip_database && !options.skip_files {
            log::info!("mode: {:?}", options.mode);
        }

        if !options.skip_database {
            run_database_core(&state, &handle, Some((&ctx, &exports)))?;
        }
        if !options.skip_files {
            run_files_core(&state, &handle, Some((&ctx, &exports)))?;
        }

        if options.mode == RunMode::Backup {
            write_manifest(&state)?;
        }

        call_hook(&ctx, &exports, "afterAll")?;
        run_extension_hooks(&ctx, &exports, extensions)?;
        Ok(())
    })
}

fn build_state(
    resolved: &Resolved,
    handle: &tokio::runtime::Handle,
    options: RunOptions,
) -> Result<RuntimeState> {
    let mut resolved = resolved.clone();
    let mut ssh = None;
    if let Some(remote) = resolved.remote.clone() {
        ssh = Some(handle.block_on(Ssh::connect(&remote))?);
    }

    // Kubernetes pods are ephemeral: resolve a label selector to a concrete
    // pod once, and read the container environment for database credentials
    // when the project does not spell them out.
    let mut local_env = resolved
        .local
        .env_file
        .as_ref()
        .map(|path| read_env_file(path))
        .unwrap_or_default();
    if let Some(kubernetes) = resolved.local.kubernetes.clone() {
        let kubernetes = resolve_pod(&kubernetes, |command| {
            let output = crate::shell::run_capture(command, None, &[], None)?;
            if output.code != 0 {
                bail!(
                    "kubectl failed with exit code {}: {}",
                    output.code,
                    output.stderr.trim()
                );
            }
            Ok(output.stdout)
        })?;
        local_env.extend(read_pod_env(&kubernetes, |command| {
            crate::shell::run_capture(command, None, &[], None).map(|output| output.stdout)
        })?);
        resolved.local.kubernetes = Some(kubernetes);
    }

    let mut remote_env = BTreeMap::new();
    if let Some(remote) = resolved.remote.clone() {
        if let Some(env_file) = &remote.env_file {
            match handle.block_on(ssh.as_ref().unwrap().read_file(env_file)) {
                Ok(content) => {
                    log::debug!(
                        "loaded {} remote env values from {env_file}",
                        parse_env(&content).len()
                    );
                    remote_env = parse_env(&content);
                }
                Err(error) => {
                    log::debug!("no remote env file {env_file}: {error:#}");
                }
            }
        }
        if let Some(kubernetes) = remote.kubernetes.clone() {
            let kubernetes = resolve_pod(&kubernetes, |command| {
                let output = handle.block_on(ssh.as_ref().unwrap().exec(command))?;
                if output.code != 0 {
                    bail!("kubectl failed: {}", output.stderr.trim());
                }
                Ok(output.stdout)
            })?;
            remote_env.extend(read_pod_env(&kubernetes, |command| {
                Ok(handle.block_on(ssh.as_ref().unwrap().exec(command))?.stdout)
            })?);
            if let Some(remote) = resolved.remote.as_mut() {
                remote.kubernetes = Some(kubernetes);
            }
        }
    }

    let (remote_db, local_db) = if let Some(cfg) = &resolved.database {
        let remote_db = match (&resolved.remote, ssh.as_ref()) {
            (Some(remote), Some(ssh)) => Some(resolve_remote_db(
                &resolved,
                remote,
                ssh,
                cfg,
                handle,
                &remote_env,
            )?),
            _ => None,
        };
        let local_db = Some(resolve_local_db(&resolved, cfg, &local_env)?);
        (remote_db, local_db)
    } else {
        (None, None)
    };

    Ok(RuntimeState {
        resolved,
        ssh,
        remote_db,
        local_db,
        remote_env,
        local_env,
        options,
        file_stats: Vec::new(),
        backup_prune_root: None,
    })
}

/// Resolve `kubernetes.selector` to a concrete pod name when needed.
fn resolve_pod(
    kubernetes: &crate::k8s::KubernetesConfig,
    run: impl Fn(&str) -> Result<String>,
) -> Result<crate::k8s::KubernetesConfig> {
    let mut kubernetes = kubernetes.clone();
    if kubernetes.pod.as_deref().unwrap_or("").trim().is_empty() {
        let target = KubernetesTarget::from(kubernetes.clone());
        let command = target.resolve_pod_command()?;
        let output = run(&command)?;
        let pod = output
            .trim()
            .lines()
            .last()
            .unwrap_or("")
            .trim()
            .to_string();
        if pod.is_empty() {
            anyhow::bail!(
                "no running pod matched kubernetes.selector {:?}",
                kubernetes.selector
            );
        }
        log::info!("resolved kubernetes pod {pod}");
        kubernetes.pod = Some(pod);
    }
    Ok(kubernetes)
}

/// Read `KEY=VALUE` pairs from a container (`kubectl exec ... -- env`).
fn read_pod_env(
    kubernetes: &crate::k8s::KubernetesConfig,
    run: impl Fn(&str) -> Result<String>,
) -> Result<BTreeMap<String, String>> {
    let target = KubernetesTarget::from(kubernetes.clone());
    let command = target.wrap("env", false)?;
    let output = run(&command)?;
    Ok(parse_env(&output))
}

fn resolve_remote_db(
    resolved: &Resolved,
    remote: &crate::config::ResolvedRemote,
    ssh: &Ssh,
    cfg: &ResolvedDbConfig,
    handle: &tokio::runtime::Handle,
    app_env: &BTreeMap<String, String>,
) -> Result<ResolvedDb> {
    if remote.kubernetes.is_some() {
        let engine_hint = cfg.engine.or_else(|| engine_from_env(app_env));
        return infer_db(
            engine_hint,
            &cfg.remote,
            app_env,
            None,
            None,
            &BTreeMap::new(),
            true,
        )
        .with_context(|| format!("resolve remote database on {}", remote.host));
    }
    let compose_yaml = match remote.compose_file_path() {
        Some(path) => match handle.block_on(ssh.read_file(&path)) {
            Ok(content) => Some(content),
            Err(error) => {
                if remote.compose.is_some() {
                    return Err(error).with_context(|| format!("read remote compose {path}"));
                }
                None
            }
        },
        None => None,
    };
    let mut file_env = BTreeMap::new();
    let compose_service = remote.compose.as_ref().and_then(|c| c.service.clone());
    if let Some(yaml) = &compose_yaml {
        let services = parse_compose_services(yaml, app_env)?;
        let chosen = compose_service
            .clone()
            .or_else(|| detect_service(&services, cfg.engine));
        if let Some(name) = chosen
            && let Some(info) = services.get(&name)
        {
            for env_file in &info.env_files {
                let path = crate::util::remote_join(remote.compose_path(), env_file);
                if let Ok(content) = handle.block_on(ssh.read_file(&path)) {
                    file_env.extend(parse_env(&content));
                }
            }
        }
    }
    let engine_hint = cfg.engine.or_else(|| engine_from_env(app_env));
    infer_db(
        engine_hint,
        &cfg.remote,
        app_env,
        compose_yaml.as_deref(),
        compose_service.as_deref(),
        &file_env,
        remote.compose.is_some(),
    )
    .with_context(|| format!("resolve remote database on {}", remote.host))
    .inspect(|_db| {
        let _ = resolved;
    })
}

fn resolve_local_db(
    resolved: &Resolved,
    cfg: &ResolvedDbConfig,
    app_env: &BTreeMap<String, String>,
) -> Result<ResolvedDb> {
    if resolved.local.kubernetes.is_some() {
        let engine_hint = cfg.engine.or_else(|| engine_from_env(app_env));
        return infer_db(
            engine_hint,
            &cfg.local,
            app_env,
            None,
            None,
            &BTreeMap::new(),
            true,
        )
        .with_context(|| "resolve local database");
    }
    let compose_yaml = resolved
        .local
        .compose
        .as_ref()
        .map(|c| Path::new(&c.path).join(&c.file))
        .filter(|p| p.exists())
        .map(|p| std::fs::read_to_string(&p))
        .transpose()
        .with_context(|| "read local compose file")?;
    let mut file_env = BTreeMap::new();
    let compose_service = resolved
        .local
        .compose
        .as_ref()
        .and_then(|c| c.service.clone());
    if let Some(yaml) = &compose_yaml {
        let services = parse_compose_services(yaml, app_env)?;
        let chosen = compose_service
            .clone()
            .or_else(|| detect_service(&services, cfg.engine));
        if let Some(name) = chosen
            && let Some(info) = services.get(&name)
        {
            for env_file in &info.env_files {
                let path = resolved
                    .local
                    .compose
                    .as_ref()
                    .map(|c| Path::new(&c.path).join(env_file))
                    .unwrap_or_else(|| PathBuf::from(env_file));
                file_env.extend(read_env_file(&path));
            }
        }
    }
    let engine_hint = cfg.engine.or_else(|| engine_from_env(app_env));
    infer_db(
        engine_hint,
        &cfg.local,
        app_env,
        compose_yaml.as_deref(),
        compose_service.as_deref(),
        &file_env,
        resolved.local.compose.is_some(),
    )
    .with_context(|| "resolve local database")
}

fn run_database_core<'js>(
    state: &Rc<RefCell<Option<RuntimeState>>>,
    handle: &tokio::runtime::Handle,
    hooks: Option<(&Ctx<'js>, &Object<'js>)>,
) -> Result<()> {
    let (has_db, dry_run, mode, backup_dir) = {
        let guard = state.borrow();
        let st = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime state missing"))?;
        (
            st.resolved.database.is_some(),
            st.options.dry_run,
            st.options.mode,
            st.options.backup_dir.clone(),
        )
    };
    if !has_db {
        log::info!("no database configured, skipping database step");
        return Ok(());
    }

    if let Some((ctx, exports)) = hooks {
        call_hook(ctx, exports, "beforeDatabase")?;
    }

    let (remote, cfg, remote_db, project_root) = {
        let guard = state.borrow();
        let st = guard.as_ref().unwrap();
        let remote = st
            .resolved
            .remote
            .clone()
            .ok_or_else(|| anyhow!("database configured but remote is missing"))?;
        let cfg = st.resolved.database.clone().unwrap();
        let remote_db = st
            .remote_db
            .clone()
            .ok_or_else(|| anyhow!("remote database could not be resolved"))?;
        (remote, cfg, remote_db, st.resolved.project_root.clone())
    };

    let remote_file = {
        let mut guard = state.borrow_mut();
        let st = guard.as_mut().unwrap();
        let ssh = st.ssh_mut(handle)?;
        handle.block_on(db::dump_remote(ssh, &remote, &cfg, &remote_db, dry_run))?
    };

    let name = db::dump_file_name(remote_db.engine, cfg.dump_name.as_deref());
    let local_dump = if let Some(dir) = &backup_dir {
        dir.join(backup::dump_relative_path(&name))
    } else {
        project_root.join("tmp").join("dump").join(&name)
    };
    {
        let mut guard = state.borrow_mut();
        let st = guard.as_mut().unwrap();
        let ssh = st.ssh_mut(handle)?;
        handle.block_on(db::fetch_dump(ssh, &remote_file, &local_dump, dry_run))?;
    }

    if mode == RunMode::Backup {
        log::info!("backup dump saved to {}", local_dump.display());
        if let Some((ctx, exports)) = hooks {
            call_hook(ctx, exports, "afterDatabase")?;
        }
        return Ok(());
    }

    if let Some((ctx, exports)) = hooks {
        call_hook(ctx, exports, "beforeRestore")?;
    }

    let (local, local_db) = {
        let guard = state.borrow();
        let st = guard.as_ref().unwrap();
        let local = st.resolved.local.clone();
        let local_db = st
            .local_db
            .clone()
            .ok_or_else(|| anyhow!("local database could not be resolved"))?;
        (local, local_db)
    };
    db::reset_local(&local, &cfg, &local_db, &project_root, dry_run)?;
    db::restore_local(&local, &cfg, &local_db, &local_dump, dry_run)?;

    if let Some((ctx, exports)) = hooks {
        call_hook(ctx, exports, "afterRestore")?;
        call_hook(ctx, exports, "afterDatabase")?;
    }
    Ok(())
}

fn run_files_core<'js>(
    state: &Rc<RefCell<Option<RuntimeState>>>,
    handle: &tokio::runtime::Handle,
    hooks: Option<(&Ctx<'js>, &Object<'js>)>,
) -> Result<Vec<(String, PathBuf, SyncStats)>> {
    let (files, dry_run, backup_dir) = {
        let guard = state.borrow();
        let st = guard.as_ref().unwrap();
        (
            st.resolved.files.clone(),
            st.options.dry_run,
            st.options.backup_dir.clone(),
        )
    };
    if files.is_empty() {
        log::info!("no files configured, skipping file step");
        return Ok(Vec::new());
    }
    if let Some((ctx, exports)) = hooks {
        call_hook(ctx, exports, "beforeFiles")?;
    }

    let mut stats = Vec::new();
    {
        let mut guard = state.borrow_mut();
        let st = guard.as_mut().unwrap();
        for spec in files {
            let mut spec = spec;
            if let Some(dir) = &backup_dir {
                spec.to = dir.join(backup::files_relative_path(&spec.from));
            }
            let ssh = st.ssh_mut(handle)?;
            let sftp = handle.block_on(ssh.sftp())?;
            let result = handle.block_on(sync::sync(&sftp, &spec, dry_run))?;
            stats.push((spec.from.clone(), spec.to.clone(), result));
        }
        st.file_stats = stats.clone();
    }
    if let Some((ctx, exports)) = hooks {
        call_hook(ctx, exports, "afterFiles")?;
    }
    Ok(stats)
}

fn write_manifest(state: &Rc<RefCell<Option<RuntimeState>>>) -> Result<()> {
    let guard = state.borrow();
    let st = guard.as_ref().unwrap();
    let dir = st
        .options
        .backup_dir
        .clone()
        .ok_or_else(|| anyhow!("backup directory is not set"))?;

    let mut manifest = Manifest::new(
        st.resolved.config_path.display().to_string(),
        st.resolved.project_root.display().to_string(),
        st.resolved.project_env.clone(),
    );
    manifest.remote = st.resolved.remote.as_ref().map(|remote| ManifestRemote {
        host: remote.host.clone(),
        user: remote.user.clone(),
        port: remote.port,
        app_path: remote.app_path.clone(),
    });
    manifest.database = st.remote_db.as_ref().map(|db| ManifestDatabase {
        engine: db.engine.as_str().to_string(),
        database: db.database.clone(),
        user: db.user.clone(),
    });
    if let Some((cfg, db)) = st.resolved.database.as_ref().zip(st.remote_db.as_ref()) {
        let path = dir.join(backup::dump_relative_path(
            &db.dump_file_name(cfg.dump_name.as_deref()),
        ));
        if path.exists() {
            manifest.set_dump(&path)?;
        }
    }
    manifest.files = st
        .file_stats
        .iter()
        .map(|(from, to, stats)| ManifestFile {
            from: from.clone(),
            to: to.display().to_string(),
            files: stats.files,
            downloaded: stats.downloaded,
            skipped: stats.skipped,
            deleted: stats.deleted,
            bytes: stats.bytes,
        })
        .collect();
    manifest.write(&dir)?;

    if let (Some(keep), Some(root)) = (st.resolved.backup.keep, st.backup_prune_root.as_deref()) {
        let plan = backup::prune(root, keep)?;
        if !plan.removed.is_empty() {
            log::info!("retention removed {} old backup(s)", plan.removed.len());
        }
    }
    Ok(())
}

fn call_hook<'js>(ctx: &Ctx<'js>, exports: &Object<'js>, name: &str) -> Result<()> {
    let hooks: Value = exports
        .get("hooks")
        .unwrap_or(Value::new_undefined(ctx.clone()));
    if !hooks.is_object() {
        return Ok(());
    }
    let hooks = hooks.into_object().unwrap();
    let hook: Value = hooks
        .get(name)
        .with_context(|| format!("read hooks.{name}"))?;
    if !hook.is_function() {
        return Ok(());
    }
    let function = hook.into_function().unwrap();
    log::info!("running hook {name}");
    let result: Value = match function.call::<_, Value>(()) {
        Ok(result) => result,
        Err(error) => {
            let caught = ctx.catch();
            let detail = caught
                .as_exception()
                .map(|exception| exception.message().unwrap_or_default())
                .unwrap_or_else(|| format!("{caught:?}"));
            let stack = caught
                .as_exception()
                .and_then(|exception| exception.stack())
                .unwrap_or_default();
            return Err(anyhow!(
                "hooks.{name} failed: {detail}\n{stack}\n(quickjs: {error})"
            ));
        }
    };
    if result.is_promise() {
        bail!("hooks.{name} returned a Promise; hooks must be synchronous");
    }
    Ok(())
}

fn js_error(message: impl std::fmt::Display) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message("cloner", "value", message.to_string())
}

fn echo_output(stdout: &str, stderr: &str) {
    if !stdout.is_empty() {
        print!("{stdout}");
        if !stdout.ends_with('\n') {
            println!();
        }
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
        if !stderr.ends_with('\n') {
            eprintln!();
        }
    }
}

fn install_globals(
    ctx: &Ctx<'_>,
    state: &Rc<RefCell<Option<RuntimeState>>>,
    handle: &tokio::runtime::Handle,
    config_path: &Path,
    from: Option<&str>,
) -> Result<()> {
    let globals = ctx.globals();
    let dirname = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .into_owned();
    let filename = config_path.to_string_lossy().into_owned();
    globals.set("__dirname", dirname)?;
    globals.set("__filename", filename)?;

    let process = Object::new(ctx.clone())?;
    let env = Object::new(ctx.clone())?;
    for (key, value) in std::env::vars() {
        env.set(key, value)?;
    }
    if let Some(from) = from {
        env.set("CLONE_FROM", from)?;
    }
    process.set("env", env)?;
    globals.set("process", process)?;

    let module = Object::new(ctx.clone())?;
    let exports = Object::new(ctx.clone())?;
    module.set("exports", exports.clone())?;
    globals.set("module", module)?;
    globals.set("exports", exports)?;

    let log_fn = Function::new(ctx.clone(), |message: String| {
        println!("{message}");
    })?;
    globals.set("__cloner_log", log_fn)?;

    // sh.local / sh.localIn / sh.remote
    let sh = Object::new(ctx.clone())?;
    {
        let state = state.clone();
        let function = Function::new(
            ctx.clone(),
            move |command: String| -> rquickjs::Result<String> {
                let (stdout, stderr, code) =
                    run_local_js(&state, &command, None).map_err(js_error)?;
                if code != 0 {
                    return Err(js_error(format!(
                        "local command failed with exit code {code}: {command}\n{stderr}"
                    )));
                }
                Ok(stdout)
            },
        )?;
        sh.set("local", function)?;
    }
    {
        let state = state.clone();
        let function = Function::new(
            ctx.clone(),
            move |cwd: String, command: String| -> rquickjs::Result<String> {
                let (stdout, stderr, code) =
                    run_local_js(&state, &command, Some(PathBuf::from(cwd))).map_err(js_error)?;
                if code != 0 {
                    return Err(js_error(format!(
                        "local command failed with exit code {code}: {command}\n{stderr}"
                    )));
                }
                Ok(stdout)
            },
        )?;
        sh.set("localIn", function)?;
    }
    {
        let state = state.clone();
        let handle = handle.clone();
        let function = Function::new(
            ctx.clone(),
            move |command: String| -> rquickjs::Result<String> {
                let mut guard = state.borrow_mut();
                let st = guard
                    .as_mut()
                    .ok_or_else(|| js_error("config is not loaded yet"))?;
                let ssh = st.ssh_mut(&handle).map_err(js_error)?;
                let output = handle
                    .block_on(ssh.exec(&command))
                    .map_err(|e| js_error(format!("{e:#}")))?;
                if output.code != 0 {
                    return Err(js_error(format!(
                        "remote command failed with exit code {}: {command}\n{}",
                        output.code,
                        output.stderr.trim()
                    )));
                }
                Ok(output.stdout)
            },
        )?;
        sh.set("remote", function)?;
    }
    globals.set("sh", sh.clone())?;

    // shell("cmd") runs in the project root; shell.in(cwd, cmd) and
    // shell.remote(cmd) are the explicit variants.
    let shell = {
        let state = state.clone();
        Function::new(
            ctx.clone(),
            move |command: String| -> rquickjs::Result<String> {
                let (stdout, stderr, code) =
                    run_local_js(&state, &command, None).map_err(js_error)?;
                echo_output(&stdout, &stderr);
                if code != 0 {
                    return Err(js_error(format!(
                        "shell command failed with exit code {code}: {command}\n{stderr}"
                    )));
                }
                Ok(stdout)
            },
        )?
    };
    {
        let state_in = state.clone();
        let in_fn = Function::new(
            ctx.clone(),
            move |cwd: String, command: String| -> rquickjs::Result<String> {
                let (stdout, stderr, code) =
                    run_local_js(&state_in, &command, Some(PathBuf::from(cwd)))
                        .map_err(js_error)?;
                echo_output(&stdout, &stderr);
                if code != 0 {
                    return Err(js_error(format!(
                        "shell command failed with exit code {code}: {command}\n{stderr}"
                    )));
                }
                Ok(stdout)
            },
        )?;
        shell.set("in", in_fn)?;
        shell.set("local", shell.clone())?;
        let state_remote = state.clone();
        let handle = handle.clone();
        let remote_fn = Function::new(
            ctx.clone(),
            move |command: String| -> rquickjs::Result<String> {
                let mut guard = state_remote.borrow_mut();
                let st = guard
                    .as_mut()
                    .ok_or_else(|| js_error("config is not loaded yet"))?;
                let ssh = st.ssh_mut(&handle).map_err(js_error)?;
                let output = handle
                    .block_on(ssh.exec(&command))
                    .map_err(|e| js_error(format!("{e:#}")))?;
                echo_output(&output.stdout, &output.stderr);
                if output.code != 0 {
                    return Err(js_error(format!(
                        "remote command failed with exit code {}: {command}\n{}",
                        output.code,
                        output.stderr.trim()
                    )));
                }
                Ok(output.stdout)
            },
        )?;
        shell.set("remote", remote_fn)?;
    }
    globals.set("shell", shell)?;

    // env.local / env.remote
    let env_obj = Object::new(ctx.clone())?;
    {
        let state = state.clone();
        let function = Function::new(ctx.clone(), move |key: String| -> String {
            let st = state.borrow();
            st.as_ref()
                .and_then(|st| st.local_env.get(&key).cloned())
                .unwrap_or_default()
        })?;
        env_obj.set("local", function)?;
    }
    {
        let state = state.clone();
        let function = Function::new(ctx.clone(), move |key: String| -> String {
            let st = state.borrow();
            st.as_ref()
                .and_then(|st| st.remote_env.get(&key).cloned())
                .unwrap_or_default()
        })?;
        env_obj.set("remote", function)?;
    }
    globals.set("env", env_obj)?;

    // Default clone actions, callable from a custom execute().
    let clone_db_fn = {
        let state = state.clone();
        let handle = handle.clone();
        Function::new(ctx.clone(), move || -> rquickjs::Result<()> {
            run_database_core(&state, &handle, None).map_err(js_error)
        })?
    };
    let clone_files_fn = {
        let state = state.clone();
        let handle = handle.clone();
        Function::new(ctx.clone(), move || -> rquickjs::Result<()> {
            run_files_core(&state, &handle, None)
                .map(|_| ())
                .map_err(js_error)
        })?
    };
    {
        let state = state.clone();
        let handle = handle.clone();
        let function = Function::new(ctx.clone(), move || -> rquickjs::Result<()> {
            run_database_core(&state, &handle, None).map_err(js_error)?;
            run_files_core(&state, &handle, None)
                .map(|_| ())
                .map_err(js_error)
        })?;
        globals.set("clone_all", function)?;
    }
    globals.set("clone_db", clone_db_fn.clone())?;
    globals.set("clone_files", clone_files_fn.clone())?;

    // db.query / db.reset / db.clone
    let db_obj = Object::new(ctx.clone())?;
    {
        let state = state.clone();
        let function = Function::new(
            ctx.clone(),
            move |sql: String| -> rquickjs::Result<String> {
                let guard = state.borrow();
                let st = guard
                    .as_ref()
                    .ok_or_else(|| js_error("config is not loaded yet"))?;
                let local = st.resolved.local.clone();
                let db = st
                    .local_db
                    .clone()
                    .ok_or_else(|| js_error("local database is not configured"))?;
                db::query_local(&local, &db, &sql, st.options.dry_run)
                    .map_err(|e| js_error(format!("{e:#}")))
            },
        )?;
        db_obj.set("query", function)?;
    }
    {
        let state = state.clone();
        let function = Function::new(ctx.clone(), move || -> rquickjs::Result<()> {
            let guard = state.borrow();
            let st = guard
                .as_ref()
                .ok_or_else(|| js_error("config is not loaded yet"))?;
            let cfg = st
                .resolved
                .database
                .clone()
                .ok_or_else(|| js_error("database is not configured"))?;
            let local = st.resolved.local.clone();
            let db = st
                .local_db
                .clone()
                .ok_or_else(|| js_error("local database is not configured"))?;
            let project_root = st.resolved.project_root.clone();
            db::reset_local(&local, &cfg, &db, &project_root, st.options.dry_run)
                .map_err(|e| js_error(format!("{e:#}")))
        })?;
        db_obj.set("reset", function)?;
    }
    db_obj.set("clone", clone_db_fn)?;
    globals.set("db", db_obj)?;

    // files.clone
    let files_obj = Object::new(ctx.clone())?;
    files_obj.set("clone", clone_files_fn)?;
    globals.set("files", files_obj)?;

    // sync(from, to)
    {
        let state = state.clone();
        let handle = handle.clone();
        let function = Function::new(
            ctx.clone(),
            move |from: String, to: String| -> rquickjs::Result<()> {
                let mut guard = state.borrow_mut();
                let st = guard
                    .as_mut()
                    .ok_or_else(|| js_error("config is not loaded yet"))?;
                let resolved = st.resolved.clone();
                let dry_run = st.options.dry_run;
                let spec = ResolvedFileSpec {
                    from: if from.starts_with('/') {
                        from
                    } else {
                        crate::util::remote_join(
                            &resolved
                                .remote
                                .as_ref()
                                .map(|r| r.app_path.clone())
                                .unwrap_or_default(),
                            &from,
                        )
                    },
                    to: resolve_path(&resolved.project_root, &to),
                    delete: false,
                    exclude: Vec::new(),
                    include: Vec::new(),
                    max_size: None,
                    follow_symlinks: false,
                };
                let ssh = st.ssh_mut(&handle).map_err(js_error)?;
                let sftp = handle
                    .block_on(ssh.sftp())
                    .map_err(|e| js_error(format!("{e:#}")))?;
                handle
                    .block_on(sync::sync(&sftp, &spec, dry_run))
                    .map(|_| ())
                    .map_err(|e| js_error(format!("{e:#}")))
            },
        )?;
        globals.set("sync", function)?;
    }

    // compose.local / compose.remote
    let compose_obj = Object::new(ctx.clone())?;
    {
        let state = state.clone();
        let function = Function::new(
            ctx.clone(),
            move |args: Rest<String>| -> rquickjs::Result<String> {
                let guard = state.borrow();
                let st = guard
                    .as_ref()
                    .ok_or_else(|| js_error("config is not loaded yet"))?;
                let compose = st
                    .resolved
                    .local
                    .compose
                    .as_ref()
                    .ok_or_else(|| js_error("local compose is not configured"))?;
                let command = format!(
                    "{} {}",
                    crate::runtime::compose_prefix(&compose.path, &compose.file),
                    args.0.join(" ")
                );
                let output = db::run_local_capture(&command, None, &[], None)
                    .map_err(|e| js_error(format!("{e:#}")))?;
                if output.code != 0 {
                    return Err(js_error(format!(
                        "compose.local failed: {}",
                        output.stderr.trim()
                    )));
                }
                Ok(output.stdout)
            },
        )?;
        compose_obj.set("local", function)?;
    }
    {
        let state = state.clone();
        let handle = handle.clone();
        let function = Function::new(
            ctx.clone(),
            move |args: Rest<String>| -> rquickjs::Result<String> {
                let mut guard = state.borrow_mut();
                let st = guard
                    .as_mut()
                    .ok_or_else(|| js_error("config is not loaded yet"))?;
                let compose = st
                    .resolved
                    .remote
                    .as_ref()
                    .and_then(|r| r.compose.as_ref())
                    .ok_or_else(|| js_error("remote compose is not configured"))?
                    .clone();
                let command = format!(
                    "{} {}",
                    crate::runtime::compose_prefix(&compose.path, &compose.file),
                    args.0.join(" ")
                );
                let ssh = st.ssh_mut(&handle).map_err(js_error)?;
                let output = handle
                    .block_on(ssh.exec(&command))
                    .map_err(|e| js_error(format!("{e:#}")))?;
                if output.code != 0 {
                    return Err(js_error(format!(
                        "compose.remote failed: {}",
                        output.stderr.trim()
                    )));
                }
                Ok(output.stdout)
            },
        )?;
        compose_obj.set("remote", function)?;
    }
    globals.set("compose", compose_obj)?;

    // log / sleep
    let log_fn = Function::new(ctx.clone(), |message: String| {
        println!("{message}");
    })?;
    globals.set("log", log_fn)?;
    let sleep_fn = Function::new(ctx.clone(), |ms: u64| {
        std::thread::sleep(Duration::from_millis(ms));
    })?;
    globals.set("sleep", sleep_fn)?;

    Ok(())
}

fn run_local_js(
    state: &Rc<RefCell<Option<RuntimeState>>>,
    command: &str,
    cwd: Option<PathBuf>,
) -> Result<(String, String, i32)> {
    let guard = state.borrow();
    let st = guard
        .as_ref()
        .ok_or_else(|| anyhow!("config is not loaded yet"))?;
    let cwd = cwd.or_else(|| Some(st.resolved.project_root.clone()));
    drop(guard);
    let output = db::run_local_capture(command, cwd.as_deref(), &[], None)?;
    Ok((output.stdout, output.stderr, output.code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn config_file(dir: &tempfile::TempDir, name: &str, source: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(source.as_bytes()).unwrap();
        path
    }

    #[test]
    fn evaluates_exported_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(
            &dir,
            "dl.js",
            r#"
            module.exports = defineConfig({
              local: { root: __dirname, env: "test" },
              remote: { host: "example.com", port: 2222, appPath: "/srv/app" },
              database: { engine: "postgres", reset: "drop-create" },
              files: [{ from: "uploads", to: "uploads", exclude: ["*.tmp"] }],
            });
            "#,
        );
        let config = eval_config(&path).unwrap();
        let root = crate::util::clean_path(dir.path().canonicalize().unwrap());
        assert_eq!(config.local.root.as_deref(), Some(root.to_str().unwrap()));
        assert_eq!(config.local.env, "test");
        let remote = config.remote.unwrap();
        assert_eq!(remote.host, "example.com");
        assert_eq!(remote.port, 2222);
        assert_eq!(remote.app_path, "/srv/app");
        assert_eq!(config.files.len(), 1);
        assert_eq!(config.files[0].from, "uploads");
        assert_eq!(config.files[0].exclude, ["*.tmp"]);
    }

    #[test]
    fn evaluates_function_exports() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(
            &dir,
            "dl.js",
            r#"
            module.exports = () => ({
              local: { env: "staging" },
              remote: { host: "staging.example.com" },
            });
            "#,
        );
        let config = eval_config(&path).unwrap();
        assert_eq!(config.local.env, "staging");
        assert_eq!(config.remote.unwrap().host, "staging.example.com");
    }

    #[test]
    fn exposes_filename_and_process_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(
            &dir,
            "dl.js",
            r#"
            module.exports = {
              local: { root: dirname(__filename) },
              remote: {
                host: "example.com",
                appPath: basename("/srv/app/"),
              },
            };
            "#,
        );
        let config = eval_config(&path).unwrap();
        let root = crate::util::clean_path(dir.path().canonicalize().unwrap());
        assert_eq!(config.local.root.as_deref(), Some(root.to_str().unwrap()));
        assert_eq!(config.remote.unwrap().app_path, "app");
    }

    #[test]
    fn dirname_and_basename_accept_windows_separators() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(
            &dir,
            "dl.js",
            r#"
            module.exports = {
              local: { root: dirname("C:\\srv\\app\\dl.js") },
              remote: { host: "example.com", appPath: basename("C:\\srv\\app\\dumps\\") },
            };
            "#,
        );
        let config = eval_config(&path).unwrap();
        assert_eq!(config.local.root.as_deref(), Some(r"C:\srv\app"));
        assert_eq!(config.remote.unwrap().app_path, "dumps");
    }

    #[test]
    fn reports_javascript_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(&dir, "dl.js", "module.exports = {;");
        let error = format!("{:#}", eval_config(&path).unwrap_err());
        assert!(error.contains("evaluate"));
        assert!(error.contains("dl.js"));
    }

    #[test]
    fn rejects_non_object_exports() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(&dir, "dl.js", "module.exports = 42;");
        let error = format!("{:#}", eval_config(&path).unwrap_err());
        assert!(error.contains("must export a config object"));

        let path = config_file(&dir, "nil.js", "module.exports = null;");
        assert!(eval_config(&path).is_err());
    }

    #[test]
    fn rejects_missing_config_files() {
        let error = eval_config(Path::new("/definitely/not/here/dl.js")).unwrap_err();
        assert!(format!("{error:#}").contains("read"));
    }

    #[test]
    fn console_logging_does_not_break_evaluation() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(
            &dir,
            "dl.js",
            r#"
            console.log("loading", { project: "demo" });
            console.warn("warning");
            module.exports = { remote: { host: "example.com" } };
            "#,
        );
        assert_eq!(
            eval_config(&path).unwrap().remote.unwrap().host,
            "example.com"
        );
    }

    struct ExtensionFixture {
        marker: PathBuf,
    }

    impl DslExtension for ExtensionFixture {
        fn name(&self) -> &'static str {
            "extension-fixture"
        }

        fn install<'js>(
            &self,
            ctx: &Ctx<'js>,
            globals: &Object<'js>,
            env: &DslEnvironment,
        ) -> Result<()> {
            globals.set("RSBACKUP_MODE", "test")?;
            let runner = env.runner.clone();
            let marker = self.marker.clone();
            let write_marker = Function::new(ctx.clone(), move || -> rquickjs::Result<()> {
                let command = format!(
                    "printf x >> {}",
                    crate::shell::shq(&marker.to_string_lossy())
                );
                runner.run_local(&command).map_err(js_error)?;
                Ok(())
            })?;
            globals.set("rsbackupWriteMarker", write_marker)?;
            Ok(())
        }

        fn hooks(&self) -> &'static [&'static str] {
            &["rsbackupAfter"]
        }
    }

    #[test]
    fn extensions_can_add_variables_and_functions() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(
            &dir,
            "dl.js",
            r#"
            module.exports = {
              local: { root: __dirname, env: "rb-" + RSBACKUP_MODE },
              execute() {},
            };
            "#,
        );
        let config = DslBuilder::new(RunOptions::default())
            .with_extension(ExtensionFixture {
                marker: dir.path().join("marker"),
            })
            .eval_config(&path)
            .unwrap();
        assert_eq!(config.local.env, "rb-test");
    }

    #[test]
    fn extensions_can_run_commands_and_register_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let path = config_file(
            &dir,
            "dl.js",
            r#"
            module.exports = {
              local: { root: __dirname, env: "test" },
              hooks: {
                rsbackupAfter() { rsbackupWriteMarker(); },
              },
            };
            "#,
        );
        DslBuilder::new(RunOptions {
            skip_database: true,
            skip_files: true,
            ..RunOptions::default()
        })
        .with_extension(ExtensionFixture {
            marker: marker.clone(),
        })
        .execute(&path)
        .unwrap();
        assert!(marker.is_file(), "extension hook did not run");
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "x");
    }

    #[test]
    fn run_options_defaults() {
        let options = RunOptions::default();
        assert!(!options.skip_database);
        assert!(!options.skip_files);
        assert!(!options.dry_run);
        assert!(matches!(options.mode, RunMode::Clone));
        assert!(options.backup_dir.is_none());
        assert!(options.from.is_none());
    }
}
