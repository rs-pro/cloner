//! Database dump, restore, reset and query operations.
//!
//! Command strings are built by [`crate::engine`] and wrapped by
//! [`crate::runtime`], so the same code path works for a host, a Compose
//! service or a Kubernetes pod.

use anyhow::{Result, anyhow, bail};
use std::path::Path;

use crate::config::{ResolvedDbConfig, ResolvedLocal, ResolvedRemote};
use crate::engine::{Engine, ResolvedDb};
use crate::runtime::CommandTarget;
use crate::shell::{self, LocalCmdOutput, shq};
use crate::ssh::Ssh;
use crate::util::human_size;

pub use crate::engine::mysql_shell;

pub fn dump_file_name(engine: Engine, custom: Option<&str>) -> String {
    custom
        .map(str::to_string)
        .unwrap_or_else(|| engine.default_dump_file().to_string())
}

/// Dump a database on the remote host, container or pod to a temporary file
/// and return that remote path.
pub async fn dump_remote(
    ssh: &Ssh,
    remote: &ResolvedRemote,
    cfg: &ResolvedDbConfig,
    db: &ResolvedDb,
    dry_run: bool,
) -> Result<String> {
    let name = db.dump_file_name(cfg.dump_name.as_deref());
    let dump_path = cfg
        .remote_dump_path
        .clone()
        .unwrap_or_else(|| remote.dump_path.clone());
    let remote_file = format!("{}/{}", dump_path.trim_end_matches('/'), name);

    let target = remote.command_target();
    let service = db.service.as_deref().or(cfg.remote_service.as_deref());
    let inner = db.dump_command(&cfg.dump_args);
    let body = target.wrap(service, &inner)?;

    let command = format!(
        "mkdir -p {dump} && rm -f {file} && ({body}) > {file} && test -s {file} && wc -c < {file}",
        dump = shq(&dump_path),
        file = shq(&remote_file),
        body = body,
    );
    log::info!(
        "dumping {} database {} on {} ({})",
        db.engine.as_str(),
        db.database,
        remote.host,
        target.kind()
    );
    if dry_run {
        println!("DRY RUN remote: {command}");
        return Ok(remote_file);
    }
    let output = ssh.exec_checked(&command).await?;
    let size = output.stdout.trim().parse::<u64>().unwrap_or(0);
    if size == 0 {
        bail!("remote dump {remote_file} is empty");
    }
    log::info!("remote dump ready: {} ({})", remote_file, human_size(size));
    Ok(remote_file)
}

/// Dump a database to a local file, using a host/Compose/Kubernetes target.
///
/// This is the entry point reusable by backup tooling: no SSH is involved.
pub fn dump_to_path(
    target: &CommandTarget,
    service: Option<&str>,
    db: &ResolvedDb,
    dump_args: &[String],
    destination: &Path,
    dry_run: bool,
) -> Result<u64> {
    let inner = db.dump_command(dump_args);
    let body = target.wrap(service, &inner)?;
    let parent = destination
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let file = destination.to_string_lossy();
    let command = format!(
        "mkdir -p {parent} && rm -f {file} && ({body}) > {file} && test -s {file} && wc -c < {file}",
        parent = shq(&parent.to_string_lossy()),
        file = shq(&file),
    );
    log::info!(
        "dumping {} database {} via {} to {}",
        db.engine.as_str(),
        db.database,
        target.kind(),
        destination.display()
    );
    if dry_run {
        println!("DRY RUN local: {command}");
        return Ok(0);
    }
    let output = shell::run(&command, None, &[], None, false)?;
    let size = output.stdout.trim().parse::<u64>().unwrap_or(0);
    if size == 0 {
        bail!("dump {} is empty", destination.display());
    }
    log::info!(
        "dump ready: {} ({})",
        destination.display(),
        human_size(size)
    );
    Ok(size)
}

pub async fn fetch_dump(
    ssh: &Ssh,
    remote_file: &str,
    local_file: &Path,
    dry_run: bool,
) -> Result<u64> {
    if dry_run {
        println!(
            "DRY RUN download {} -> {}",
            remote_file,
            local_file.display()
        );
        return Ok(0);
    }
    if let Some(parent) = local_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let size = ssh.download(remote_file, local_file).await?;
    log::info!(
        "downloaded {} to {}",
        human_size(size),
        local_file.display()
    );
    Ok(size)
}

pub fn restore_local(
    local: &ResolvedLocal,
    cfg: &ResolvedDbConfig,
    db: &ResolvedDb,
    dump_file: &Path,
    dry_run: bool,
) -> Result<()> {
    let target = local.command_target();
    let service = cfg.local_service.as_deref().or(db.service.as_deref());
    let inner = db.restore_command(&cfg.restore_args);
    let command = format!(
        "{} < {}",
        target.wrap_stdin(service, &inner)?,
        shq(&dump_file.to_string_lossy())
    );

    log::info!(
        "restoring {} database {} locally via {}",
        db.engine.as_str(),
        db.database,
        target.kind()
    );
    shell::run(&command, None, &[], None, dry_run).map(|_| ())
}

pub fn reset_local(
    local: &ResolvedLocal,
    cfg: &ResolvedDbConfig,
    db: &ResolvedDb,
    project_root: &Path,
    dry_run: bool,
) -> Result<()> {
    match cfg.reset {
        crate::config::ResetMode::None => Ok(()),
        crate::config::ResetMode::DropCreate => drop_create(local, db, dry_run),
        crate::config::ResetMode::Volume => volume_reset(local, cfg, db, project_root, dry_run),
    }
}

fn drop_create(local: &ResolvedLocal, db: &ResolvedDb, dry_run: bool) -> Result<()> {
    log::info!(
        "resetting local {} database {}",
        db.engine.as_str(),
        db.database
    );
    let target = local.command_target();
    if let Some(prefix) = target.compose_prefix() {
        let service = db.service.as_deref().unwrap_or("db");
        log::info!("starting local compose service {service}");
        shell::run(
            &format!("{} up -d {}", prefix, shq(service)),
            None,
            &[],
            None,
            dry_run,
        )?;
        wait_ready(local, db, dry_run)?;
    }
    let command = target.wrap(db.service.as_deref(), &db.drop_create_command())?;
    shell::run(&command, None, &[], None, dry_run).map(|_| ())
}

fn volume_reset(
    local: &ResolvedLocal,
    cfg: &ResolvedDbConfig,
    db: &ResolvedDb,
    _project_root: &Path,
    dry_run: bool,
) -> Result<()> {
    let compose = local
        .compose
        .as_ref()
        .ok_or_else(|| anyhow!("reset: volume requires local.compose"))?;
    let volume = cfg
        .volume
        .as_ref()
        .ok_or_else(|| anyhow!("reset: volume requires database.volume"))?;
    log::info!("resetting local database volume {volume}");
    let prefix = crate::runtime::compose_prefix(&compose.path, &compose.file);

    shell::run(&format!("{prefix} down"), None, &[], None, dry_run)?;
    shell::run(
        &format!("docker volume rm -f {}", shq(volume)),
        None,
        &[],
        None,
        dry_run,
    )?;
    shell::run(
        &format!("docker volume create {}", shq(volume)),
        None,
        &[],
        None,
        dry_run,
    )?;
    // Match the original cloner behavior: after recreating the volume, bring
    // the whole compose project up (not just the DB), because migrations and
    // the app may need search/redis/etc. too.
    shell::run(&format!("{prefix} up -d"), None, &[], None, dry_run)?;
    wait_ready(local, db, dry_run)
}

pub fn wait_ready(local: &ResolvedLocal, db: &ResolvedDb, dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    let target = local.command_target();
    let check = target.wrap(db.service.as_deref(), &db.ping_command())?;
    for attempt in 1..=60 {
        if shell::run(&check, None, &[], None, false).is_ok() {
            log::info!("local database is ready");
            return Ok(());
        }
        log::debug!("waiting for local database ({attempt}/60)");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    bail!("local database did not become ready within 60 seconds")
}

pub fn query_local(
    local: &ResolvedLocal,
    db: &ResolvedDb,
    sql: &str,
    dry_run: bool,
) -> Result<String> {
    let target = local.command_target();
    let command = target.wrap(db.service.as_deref(), &db.query_command(sql))?;
    let output = shell::run(&command, None, &[], None, dry_run)?;
    Ok(output.stdout)
}

/// Capture a local command without failing on a non-zero exit code.
pub fn run_local_capture(
    command: &str,
    cwd: Option<&Path>,
    envs: &[(String, String)],
    stdin_path: Option<&Path>,
) -> Result<LocalCmdOutput> {
    shell::run_capture(command, cwd, envs, stdin_path)
}

/// Run a local command, treating a non-zero exit code as an error.
pub fn run_local(
    command: &str,
    cwd: Option<&Path>,
    envs: &[(String, String)],
    stdin_path: Option<&Path>,
    dry_run: bool,
) -> Result<LocalCmdOutput> {
    shell::run(command, cwd, envs, stdin_path, dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(engine: Engine) -> ResolvedDb {
        ResolvedDb {
            engine,
            host: "localhost".into(),
            port: engine.default_port(),
            database: "app_development".into(),
            user: "app".into(),
            password: "secret".into(),
            service: Some("db".into()),
            in_container: true,
        }
    }

    fn local(compose: bool) -> ResolvedLocal {
        ResolvedLocal {
            compose: compose.then(|| crate::config::ResolvedCompose {
                path: "/project".into(),
                file: "compose.yml".into(),
                service: Some("db".into()),
            }),
            env_file: None,
            kubernetes: None,
        }
    }

    fn cfg() -> ResolvedDbConfig {
        ResolvedDbConfig {
            engine: Some(Engine::Postgres),
            remote: Default::default(),
            local: Default::default(),
            remote_service: None,
            local_service: None,
            reset: crate::config::ResetMode::DropCreate,
            volume: None,
            dump_name: None,
            dump_args: Vec::new(),
            restore_args: Vec::new(),
            remote_dump_path: None,
        }
    }

    #[test]
    fn chooses_dump_file_names() {
        assert_eq!(dump_file_name(Engine::Postgres, None), "cloner.bak");
        assert_eq!(dump_file_name(Engine::Mysql, None), "cloner.sql");
        assert_eq!(dump_file_name(Engine::Mongo, None), "cloner.archive");
        assert_eq!(
            dump_file_name(Engine::Postgres, Some("custom.dump")),
            "custom.dump"
        );
    }

    #[test]
    fn builds_mysql_shell_with_mariadb_fallback() {
        let command = mysql_shell(
            "mysql",
            "mariadb",
            &["--user=app".to_string(), "-e 'SELECT 1'".to_string()],
        );
        assert!(command.starts_with("sh -lc 'if command -v mysql"));
        assert!(command.contains("else exec mariadb \"$@\""));
        assert!(command.ends_with("sh --user=app -e 'SELECT 1'"));
    }

    #[test]
    fn dump_to_path_uses_the_command_target() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dump/cloner.bak");
        // Dry run prints the composed command but does not create the file.
        dump_to_path(
            &CommandTarget::compose("/project", "compose.yml"),
            Some("db"),
            &db(Engine::Postgres),
            &[],
            &dump,
            true,
        )
        .unwrap();
        assert!(!dump.exists());
    }

    #[test]
    fn reset_without_reset_mode_is_a_noop() {
        let mut cfg = cfg();
        cfg.reset = crate::config::ResetMode::None;
        reset_local(
            &local(false),
            &cfg,
            &db(Engine::Postgres),
            Path::new("/project"),
            true,
        )
        .unwrap();
    }

    #[test]
    fn volume_reset_requires_compose_and_a_volume() {
        let mut cfg = cfg();
        cfg.reset = crate::config::ResetMode::Volume;
        let error = reset_local(
            &local(false),
            &cfg,
            &db(Engine::Postgres),
            Path::new("/project"),
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("local.compose"));

        let error = reset_local(
            &local(true),
            &cfg,
            &db(Engine::Postgres),
            Path::new("/project"),
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("database.volume"));
    }

    #[test]
    fn captures_stdout_stderr_and_exit_codes() {
        let ok = run_local_capture("echo hello", None, &[], None).unwrap();
        assert_eq!(ok.stdout.trim(), "hello");
        assert_eq!(ok.code, 0);

        let failed = run_local_capture("echo oops >&2; exit 7", None, &[], None).unwrap();
        assert_eq!(failed.code, 7);
        assert_eq!(failed.stderr.trim(), "oops");
    }

    #[test]
    fn captures_with_cwd_env_and_stdin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "content").unwrap();
        let output = run_local_capture(
            "cat marker; printf '%s' \"$CLONER_TEST\"",
            Some(dir.path()),
            &[("CLONER_TEST".to_string(), "value".to_string())],
            None,
        )
        .unwrap();
        assert_eq!(output.stdout, "contentvalue");

        let input = dir.path().join("input.txt");
        std::fs::write(&input, "stdin-data").unwrap();
        let output = run_local_capture("cat", None, &[], Some(&input)).unwrap();
        assert_eq!(output.stdout, "stdin-data");
    }

    #[test]
    fn dry_run_returns_success_without_executing() {
        let output = run_local("exit 1", None, &[], None, true).unwrap();
        assert_eq!(output.code, 0);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn errors_include_command_and_output() {
        let error = run_local("echo boom >&2; exit 3", None, &[], None, false).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("exit code 3"));
        assert!(text.contains("boom"));
        assert!(text.contains("echo boom"));
    }
}
