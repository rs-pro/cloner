use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::util::{remote_join, resolve_path};

pub use crate::compose::{
    ServiceInfo, detect_service, engine_from_env, infer_db, parse_compose_services,
};
pub use crate::engine::{DbConn, Engine, ResolvedDb};
pub use crate::k8s::KubernetesConfig;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub project: Project,
    pub remote: Option<Remote>,
    pub local: Local,
    pub database: Option<Database>,
    pub files: Vec<FileSpec>,
    pub backup: Backup,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[derive(Default)]
pub struct Project {
    /// Deprecated: use local.root. Kept for older DSL files.
    pub root: Option<String>,
    /// Deprecated: use local.env. Kept for older DSL files.
    pub env: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Remote {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub app_path: String,
    pub dump_path: Option<String>,
    pub compose: Option<Compose>,
    pub env_file: Option<String>,
    pub identity_file: Option<String>,
    pub password: Option<String>,
    pub password_env: Option<String>,
    pub strict_host_key_checking: bool,
    pub known_hosts: Option<String>,
    pub kubernetes: Option<KubernetesConfig>,
}

impl Default for Remote {
    fn default() -> Self {
        Self {
            host: String::new(),
            user: "root".to_string(),
            port: 22,
            app_path: "/".to_string(),
            dump_path: None,
            compose: None,
            env_file: None,
            identity_file: None,
            password: None,
            password_env: None,
            strict_host_key_checking: false,
            known_hosts: None,
            kubernetes: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Local {
    /// Local project root (default: directory containing the DSL file).
    pub root: Option<String>,
    /// Local environment name, e.g. development or test.
    pub env: String,
    pub compose: Option<Compose>,
    pub env_file: Option<String>,
    pub kubernetes: Option<KubernetesConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Compose {
    pub path: Option<String>,
    pub file: String,
    pub service: Option<String>,
}

impl Default for Compose {
    fn default() -> Self {
        Self {
            path: None,
            file: "compose.yml".to_string(),
            service: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResetMode {
    #[default]
    None,
    DropCreate,
    Volume,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Database {
    pub engine: Option<Engine>,
    pub service: Option<String>,
    pub remote_service: Option<String>,
    pub local_service: Option<String>,
    pub remote: DbConn,
    pub local: DbConn,
    pub remote_env_file: Option<String>,
    pub local_env_file: Option<String>,
    pub reset: ResetMode,
    pub volume: Option<String>,
    pub dump_name: Option<String>,
    pub dump_args: Vec<String>,
    pub restore_args: Vec<String>,
    pub remote_dump_path: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileSpec {
    pub from: String,
    pub to: String,
    pub delete: bool,
    #[serde(alias = "excludes")]
    pub exclude: Vec<String>,
    #[serde(alias = "includes")]
    pub include: Vec<String>,
    pub max_size: Option<u64>,
    pub follow_symlinks: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Backup {
    pub destination: Option<String>,
    pub keep: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Resolved {
    pub config_path: PathBuf,
    pub project_root: PathBuf,
    pub project_env: String,
    pub remote: Option<ResolvedRemote>,
    pub local: ResolvedLocal,
    pub database: Option<ResolvedDbConfig>,
    pub files: Vec<ResolvedFileSpec>,
    pub backup: Backup,
}

#[derive(Clone, Debug)]
pub struct ResolvedRemote {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub app_path: String,
    pub dump_path: String,
    pub compose: Option<ResolvedCompose>,
    pub env_file: Option<String>,
    pub identity_file: Option<PathBuf>,
    pub password: Option<String>,
    pub strict_host_key_checking: bool,
    pub known_hosts: Option<PathBuf>,
    pub kubernetes: Option<KubernetesConfig>,
}

impl ResolvedRemote {
    pub fn compose_path(&self) -> &str {
        self.compose
            .as_ref()
            .map(|c| c.path.as_str())
            .unwrap_or(&self.app_path)
    }

    pub fn compose_file_path(&self) -> Option<String> {
        self.compose.as_ref().map(|c| remote_join(&c.path, &c.file))
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedLocal {
    pub compose: Option<ResolvedCompose>,
    pub env_file: Option<PathBuf>,
    pub kubernetes: Option<KubernetesConfig>,
}

#[derive(Clone, Debug)]
pub struct ResolvedCompose {
    pub path: String,
    pub file: String,
    pub service: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedDbConfig {
    pub engine: Option<Engine>,
    pub remote: DbConn,
    pub local: DbConn,
    pub remote_service: Option<String>,
    pub local_service: Option<String>,
    pub reset: ResetMode,
    pub volume: Option<String>,
    pub dump_name: Option<String>,
    pub dump_args: Vec<String>,
    pub restore_args: Vec<String>,
    pub remote_dump_path: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedFileSpec {
    pub from: String,
    pub to: PathBuf,
    pub delete: bool,
    pub exclude: Vec<String>,
    pub include: Vec<String>,
    pub max_size: Option<u64>,
    pub follow_symlinks: bool,
}

impl Config {
    pub fn resolve(&self, config_path: &Path) -> Result<Resolved> {
        let base = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let project_root = match self.local.root.as_ref().or(self.project.root.as_ref()) {
            Some(root) => resolve_path(&base, root),
            None => base.clone(),
        };
        let project_env = if !self.local.env.trim().is_empty() {
            self.local.env.clone()
        } else {
            self.project
                .env
                .clone()
                .unwrap_or_else(|| "development".to_string())
        };

        let remote = match &self.remote {
            Some(remote) => {
                if remote.host.trim().is_empty() {
                    bail!("remote.host is required in {}", config_path.display());
                }
                let app_path = remote.app_path.clone();
                let db = self.database.as_ref();
                let compose = remote.compose.as_ref().map(|c| ResolvedCompose {
                    path: c
                        .path
                        .as_deref()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| app_path.clone()),
                    file: c.file.clone(),
                    service: db
                        .and_then(|d| d.remote_service.clone().or_else(|| d.service.clone()))
                        .or_else(|| c.service.clone()),
                });
                let env_file = db
                    .and_then(|d| d.remote_env_file.clone())
                    .or_else(|| remote.env_file.clone())
                    .or_else(|| compose.as_ref().map(|c| remote_join(&c.path, ".env")))
                    .or_else(|| Some(remote_join(&app_path, ".env")));
                let dump_path = db
                    .and_then(|d| d.remote_dump_path.clone())
                    .or_else(|| remote.dump_path.clone())
                    .map(|p| {
                        if p.starts_with('/') {
                            p
                        } else {
                            remote_join(&app_path, &p)
                        }
                    })
                    .unwrap_or_else(|| remote_join(&app_path, "tmp_dump"));
                let password = remote.password.clone().or_else(|| {
                    remote
                        .password_env
                        .as_ref()
                        .and_then(|key| std::env::var(key).ok())
                });
                Some(ResolvedRemote {
                    host: remote.host.clone(),
                    user: remote.user.clone(),
                    port: remote.port,
                    app_path,
                    dump_path,
                    compose,
                    env_file,
                    identity_file: remote.identity_file.as_deref().map(resolve_path_owned),
                    password,
                    strict_host_key_checking: remote.strict_host_key_checking,
                    known_hosts: remote.known_hosts.as_deref().map(resolve_path_owned),
                    kubernetes: remote.kubernetes.clone(),
                })
            }
            None => None,
        };

        let local_compose = self.local.compose.as_ref().map(|c| ResolvedCompose {
            path: c
                .path
                .as_deref()
                .map(|p| {
                    resolve_path(&project_root, p)
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_else(|| project_root.to_string_lossy().into_owned()),
            file: c.file.clone(),
            service: self
                .database
                .as_ref()
                .and_then(|d| d.local_service.clone().or_else(|| d.service.clone()))
                .or_else(|| c.service.clone()),
        });
        let local_env_file = self
            .database
            .as_ref()
            .and_then(|d| d.local_env_file.as_deref())
            .map(|p| resolve_path(&project_root, p))
            .or_else(|| {
                self.local
                    .env_file
                    .as_deref()
                    .map(|p| resolve_path(&project_root, p))
            })
            .or_else(|| {
                local_compose
                    .as_ref()
                    .map(|c| PathBuf::from(&c.path).join(".env"))
            })
            .or_else(|| Some(project_root.join(".env")));

        let local = ResolvedLocal {
            compose: local_compose,
            env_file: local_env_file,
            kubernetes: self.local.kubernetes.clone(),
        };

        let database = self.database.as_ref().map(|db| ResolvedDbConfig {
            engine: db.engine,
            remote: db.remote.clone(),
            local: db.local.clone(),
            remote_service: db.remote_service.clone().or_else(|| db.service.clone()),
            local_service: db.local_service.clone().or_else(|| db.service.clone()),
            reset: db.reset,
            volume: db.volume.clone(),
            dump_name: db.dump_name.clone(),
            dump_args: db.dump_args.clone(),
            restore_args: db.restore_args.clone(),
            remote_dump_path: db.remote_dump_path.clone(),
        });

        let files = self
            .files
            .iter()
            .map(|file| {
                if file.from.trim().is_empty() {
                    bail!("files[].from must not be empty");
                }
                let from = if file.from.starts_with('/') {
                    file.from.clone()
                } else if let Some(remote) = &remote {
                    remote_join(&remote.app_path, &file.from)
                } else {
                    file.from.clone()
                };
                let to = if file.to.trim().is_empty() {
                    project_root.join(basename(&file.from))
                } else {
                    resolve_path(&project_root, &file.to)
                };
                Ok(ResolvedFileSpec {
                    from,
                    to,
                    delete: file.delete,
                    exclude: file.exclude.clone(),
                    include: file.include.clone(),
                    max_size: file.max_size,
                    follow_symlinks: file.follow_symlinks,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Resolved {
            config_path: config_path.to_path_buf(),
            project_root,
            project_env,
            remote,
            local,
            database,
            files,
            backup: self.backup.clone(),
        })
    }
}

fn resolve_path_owned(value: &str) -> PathBuf {
    crate::util::expand_tilde(value)
}

fn basename(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("clone")
        .to_string()
}

pub fn describe(
    resolved: &Resolved,
    remote_db: Option<&ResolvedDb>,
    local_db: Option<&ResolvedDb>,
) -> serde_json::Value {
    let remote = resolved.remote.as_ref().map(|r| {
        serde_json::json!({
            "host": r.host,
            "user": r.user,
            "port": r.port,
            "appPath": r.app_path,
            "dumpPath": r.dump_path,
            "compose": r.compose.as_ref().map(|c| serde_json::json!({"path": c.path, "file": c.file, "service": c.service})),
            "envFile": r.env_file,
            "identityFile": r.identity_file.as_ref().map(|p| p.display().to_string()),
            "strictHostKeyChecking": r.strict_host_key_checking,
            "kubernetes": r.kubernetes.as_ref().map(kubernetes_json),
        })
    });
    serde_json::json!({
        "config": resolved.config_path.display().to_string(),
        "projectRoot": resolved.project_root.display().to_string(),
        "projectEnv": resolved.project_env,
        "remote": remote,
        "local": {
            "compose": resolved.local.compose.as_ref().map(|c| serde_json::json!({"path": c.path, "file": c.file, "service": c.service})),
            "envFile": resolved.local.env_file.as_ref().map(|p| p.display().to_string()),
            "kubernetes": resolved.local.kubernetes.as_ref().map(kubernetes_json),
        },
        "database": resolved.database.as_ref().map(|d| serde_json::json!({
            "engine": d.engine,
            "reset": d.reset,
            "volume": d.volume,
            "dumpName": d.dump_name,
            "remoteDumpPath": d.remote_dump_path,
            "remote": remote_db.map(db_json),
            "local": local_db.map(db_json),
        })),
        "files": resolved.files.iter().map(|f| serde_json::json!({
            "from": f.from,
            "to": f.to.display().to_string(),
            "delete": f.delete,
            "exclude": f.exclude,
            "include": f.include,
        })).collect::<Vec<_>>(),
        "backup": {"destination": resolved.backup.destination, "keep": resolved.backup.keep},
    })
}

fn kubernetes_json(k: &KubernetesConfig) -> serde_json::Value {
    serde_json::json!({
        "context": k.context,
        "namespace": k.namespace,
        "pod": k.pod,
        "selector": k.selector,
        "container": k.container,
    })
}

fn db_json(db: &ResolvedDb) -> serde_json::Value {
    serde_json::json!({
        "engine": db.engine,
        "host": db.host,
        "port": db.port,
        "database": db.database,
        "user": db.user,
        "password": if db.password.is_empty() { "" } else { "********" },
        "service": db.service,
        "inContainer": db.in_container,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::parse_env;
    use std::collections::BTreeMap;

    #[test]
    fn resolves_compose_service_from_env() {
        let env = parse_env("DB_USER=app\nDB_PASSWORD=secret\nDB_NAME=app_development\n");
        let compose = r#"
services:
  app:
    image: example/app
  db:
    image: postgres:17
    environment:
      POSTGRES_USER: ${DB_USER}
      POSTGRES_PASSWORD: ${DB_PASSWORD}
      POSTGRES_DB: ${DB_NAME}
"#;
        let db = infer_db(
            None,
            &DbConn::default(),
            &env,
            Some(compose),
            None,
            &BTreeMap::new(),
            true,
        )
        .expect("resolve db");
        assert_eq!(db.engine, Engine::Postgres);
        assert_eq!(db.service.as_deref(), Some("db"));
        assert_eq!(db.database, "app_development");
        assert_eq!(db.user, "app");
        assert_eq!(db.password, "secret");
        assert_eq!(db.host, "localhost");
        assert_eq!(db.port, 5432);
    }

    #[test]
    fn explicit_overrides_win() {
        let env = parse_env("DB_NAME=from_env\nDB_USER=env_user\nDB_PASSWORD=env_pass\n");
        let overrides = DbConn {
            database: Some("explicit".to_string()),
            user: Some("explicit_user".to_string()),
            ..DbConn::default()
        };
        let db = infer_db(
            Some(Engine::Postgres),
            &overrides,
            &env,
            None,
            None,
            &BTreeMap::new(),
            false,
        )
        .expect("resolve db");
        assert_eq!(db.database, "explicit");
        assert_eq!(db.user, "explicit_user");
        assert_eq!(db.password, "env_pass");
    }

    #[test]
    fn detects_mysql_and_mongo_services() {
        let mysql = r#"
services:
  db:
    image: mariadb:11
    environment:
      MYSQL_DATABASE: app
      MYSQL_USER: app
      MYSQL_PASSWORD: pw
"#;
        let db = infer_db(
            None,
            &DbConn::default(),
            &BTreeMap::new(),
            Some(mysql),
            None,
            &BTreeMap::new(),
            true,
        )
        .unwrap();
        assert_eq!(db.engine, Engine::Mysql);
        assert_eq!(db.port, 3306);

        let mongo = r#"
services:
  db:
    image: mongo:7
    environment:
      MONGO_INITDB_DATABASE: app
"#;
        let db = infer_db(
            None,
            &DbConn::default(),
            &BTreeMap::new(),
            Some(mongo),
            None,
            &BTreeMap::new(),
            true,
        )
        .unwrap();
        assert_eq!(db.engine, Engine::Mongo);
        assert_eq!(db.port, 27017);
    }

    #[test]
    fn engine_metadata_is_stable() {
        assert_eq!(Engine::Postgres.as_str(), "postgres");
        assert_eq!(Engine::Mysql.as_str(), "mysql");
        assert_eq!(Engine::Mongo.as_str(), "mongo");
        assert_eq!(Engine::Postgres.default_port(), 5432);
        assert_eq!(Engine::Mysql.default_port(), 3306);
        assert_eq!(Engine::Mongo.default_port(), 27017);
    }

    #[test]
    fn deserializes_reset_modes() {
        assert_eq!(
            serde_json::from_str::<ResetMode>("\"none\"").unwrap(),
            ResetMode::None
        );
        assert_eq!(
            serde_json::from_str::<ResetMode>("\"drop-create\"").unwrap(),
            ResetMode::DropCreate
        );
        assert_eq!(
            serde_json::from_str::<ResetMode>("\"volume\"").unwrap(),
            ResetMode::Volume
        );
        assert!(serde_json::from_str::<ResetMode>("\"drop\"").is_err());
    }

    #[test]
    fn deserializes_camel_case_config_and_file_aliases() {
        let config: Config = serde_json::from_str(
            r#"{
                "project": {"root": "/legacy", "env": "test"},
                "local": {"root": "/project", "env": "development",
                          "compose": {"path": "dev-docker", "file": "compose.yml", "service": "db"}},
                "remote": {"host": "example.com", "appPath": "/srv/app", "strictHostKeyChecking": true},
                "database": {"engine": "postgres", "reset": "drop-create", "restoreArgs": ["-Fc"]},
                "files": [{"from": "uploads", "excludes": ["*.tmp"], "includes": ["*.jpg"]}],
                "backup": {"destination": "backups", "keep": 3}
            }"#,
        )
        .unwrap();
        assert_eq!(config.local.root.as_deref(), Some("/project"));
        assert_eq!(config.local.env, "development");
        assert_eq!(config.remote.as_ref().unwrap().app_path, "/srv/app");
        assert!(config.remote.as_ref().unwrap().strict_host_key_checking);
        assert_eq!(
            config.database.as_ref().unwrap().reset,
            ResetMode::DropCreate
        );
        assert_eq!(config.database.as_ref().unwrap().restore_args, ["-Fc"]);
        assert_eq!(config.files[0].exclude, ["*.tmp"]);
        assert_eq!(config.files[0].include, ["*.jpg"]);
        assert_eq!(config.backup.keep, Some(3));
    }

    #[test]
    fn parses_compose_services_from_maps_and_sequences() {
        let env = parse_env("DB_NAME=app\nDB_PASSWORD=secret\n");
        let yaml = r#"
services:
  app:
    image: example/app
  db:
    image: postgres:17
    environment:
      - POSTGRES_DB=${DB_NAME}
      - POSTGRES_PASSWORD=${DB_PASSWORD}
      - NUMERIC=42
      - BOOL=true
      - FROM_ENV
    env_file:
      - .env
      - path: extra.env
  ignored: not-a-mapping
"#;
        let services = parse_compose_services(yaml, &env).unwrap();
        assert_eq!(services.len(), 2);
        let db = &services["db"];
        assert_eq!(db.image, "postgres:17");
        assert_eq!(db.env["POSTGRES_DB"], "app");
        assert_eq!(db.env["POSTGRES_PASSWORD"], "secret");
        assert_eq!(db.env["NUMERIC"], "42");
        assert_eq!(db.env["BOOL"], "true");
        assert_eq!(db.env_files, [".env", "extra.env"]);
        assert_eq!(services["app"].image, "example/app");
    }

    #[test]
    fn rejects_compose_without_services() {
        assert!(parse_compose_services("version: '3'\n", &BTreeMap::new()).is_err());
        assert!(parse_compose_services("not: [valid", &BTreeMap::new()).is_err());
    }

    #[test]
    fn detects_service_using_image_env_and_name() {
        let env = BTreeMap::new();
        let services = parse_compose_services(
            r#"
services:
  web:
    image: nginx
  cache:
    image: redis
  storage:
    image: pgvector/pgvector:pg17
"#,
            &env,
        )
        .unwrap();
        assert_eq!(detect_service(&services, None).as_deref(), Some("storage"));
        assert_eq!(
            detect_service(&services, Some(Engine::Postgres)).as_deref(),
            Some("storage")
        );

        let by_name =
            parse_compose_services("services:\n  rtdb:\n    image: example/custom\n", &env)
                .unwrap();
        assert_eq!(detect_service(&by_name, None).as_deref(), Some("rtdb"));

        let unrelated =
            parse_compose_services("services:\n  web:\n    image: nginx\n", &env).unwrap();
        assert_eq!(detect_service(&unrelated, None), None);
    }

    #[test]
    fn detects_engine_from_environment_keys() {
        let mut env = BTreeMap::new();
        env.insert("POSTGRES_DB".into(), "app".into());
        assert_eq!(engine_from_env(&env), Some(Engine::Postgres));

        let mut env = BTreeMap::new();
        env.insert("MARIADB_DATABASE".into(), "app".into());
        assert_eq!(engine_from_env(&env), Some(Engine::Mysql));

        let mut env = BTreeMap::new();
        env.insert("MONGO_URI".into(), "mongodb://localhost".into());
        assert_eq!(engine_from_env(&env), Some(Engine::Mongo));

        assert_eq!(engine_from_env(&BTreeMap::new()), None);
    }

    #[test]
    fn infers_database_from_sequence_environment() {
        let yaml = r#"
services:
  db:
    image: mariadb:11
    environment:
      - MYSQL_DATABASE=app
      - MYSQL_USER=app
      - MYSQL_PASSWORD=pw
"#;
        let db = infer_db(
            None,
            &DbConn::default(),
            &BTreeMap::new(),
            Some(yaml),
            None,
            &BTreeMap::new(),
            true,
        )
        .unwrap();
        assert_eq!(db.engine, Engine::Mysql);
        assert_eq!(db.database, "app");
        assert_eq!(db.user, "app");
        assert_eq!(db.password, "pw");
        assert_eq!(db.service.as_deref(), Some("db"));
    }

    #[test]
    fn infers_connection_outside_containers() {
        let env = parse_env("DB_HOST=db.internal\nDB_PORT=15432\nDB_NAME=app\nDB_USER=app\n");
        let db = infer_db(
            Some(Engine::Postgres),
            &DbConn::default(),
            &env,
            None,
            None,
            &BTreeMap::new(),
            false,
        )
        .unwrap();
        assert_eq!(db.host, "db.internal");
        assert_eq!(db.port, 15432);
        assert!(!db.in_container);
    }

    #[test]
    fn explicit_connection_values_override_everything() {
        let env = parse_env("DB_HOST=env-host\nDB_PORT=1234\nDB_NAME=env-db\nDB_USER=env-user\n");
        let overrides = DbConn {
            host: Some("explicit-host".into()),
            port: Some(9999),
            database: Some("explicit-db".into()),
            user: Some("explicit-user".into()),
            password: Some("explicit-pass".into()),
        };
        let db = infer_db(
            Some(Engine::Postgres),
            &overrides,
            &env,
            None,
            None,
            &BTreeMap::new(),
            false,
        )
        .unwrap();
        assert_eq!(db.host, "explicit-host");
        assert_eq!(db.port, 9999);
        assert_eq!(db.database, "explicit-db");
        assert_eq!(db.user, "explicit-user");
        assert_eq!(db.password, "explicit-pass");
    }

    #[test]
    fn missing_compose_service_is_an_error() {
        let yaml = "services:\n  db:\n    image: postgres:17\n";
        let error = infer_db(
            None,
            &DbConn::default(),
            &BTreeMap::new(),
            Some(yaml),
            Some("missing"),
            &BTreeMap::new(),
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("service \"missing\" not found"));
    }

    #[test]
    fn missing_engine_or_database_is_an_error() {
        let error = infer_db(
            None,
            &DbConn::default(),
            &BTreeMap::new(),
            None,
            None,
            &BTreeMap::new(),
            true,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot determine database engine")
        );

        let error = infer_db(
            Some(Engine::Postgres),
            &DbConn::default(),
            &BTreeMap::new(),
            None,
            None,
            &BTreeMap::new(),
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot determine database name"));
    }

    #[test]
    fn resolves_local_paths_and_file_targets() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("dl.js");
        std::fs::write(&config_path, "").unwrap();
        let config = Config {
            local: Local {
                root: Some("app".into()),
                env: "test".into(),
                compose: Some(Compose {
                    path: Some("dev-docker".into()),
                    file: "compose.yml".into(),
                    service: None,
                }),
                env_file: None,
                kubernetes: None,
            },
            remote: Some(Remote {
                host: "example.com".into(),
                app_path: "/srv/app".into(),
                compose: Some(Compose {
                    path: None,
                    file: "compose.yml".into(),
                    service: None,
                }),
                ..Remote::default()
            }),
            database: Some(Database {
                engine: Some(Engine::Postgres),
                service: Some("postgres".into()),
                remote_dump_path: Some("dumps".into()),
                ..Database::default()
            }),
            files: vec![
                FileSpec {
                    from: "uploads".into(),
                    to: "files/uploads".into(),
                    ..FileSpec::default()
                },
                FileSpec {
                    from: "logs".into(),
                    to: String::new(),
                    ..FileSpec::default()
                },
            ],
            ..Config::default()
        };
        let resolved = config.resolve(&config_path).unwrap();
        assert_eq!(resolved.project_root, dir.path().join("app"));
        assert_eq!(resolved.project_env, "test");
        let remote = resolved.remote.unwrap();
        assert_eq!(remote.dump_path, "/srv/app/dumps");
        assert_eq!(remote.env_file.as_deref(), Some("/srv/app/.env"));
        assert_eq!(
            remote.compose.as_ref().unwrap().service.as_deref(),
            Some("postgres")
        );
        let compose = resolved.local.compose.as_ref().unwrap();
        assert_eq!(
            PathBuf::from(&compose.path),
            dir.path().join("app").join("dev-docker")
        );
        assert_eq!(
            resolved.local.env_file,
            Some(dir.path().join("app/dev-docker/.env"))
        );
        assert_eq!(resolved.files[0].from, "/srv/app/uploads");
        assert_eq!(resolved.files[0].to, dir.path().join("app/files/uploads"));
        assert_eq!(resolved.files[1].to, dir.path().join("app/logs"));
    }

    #[test]
    fn falls_back_to_deprecated_project_settings() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("dl.js");
        std::fs::write(&config_path, "").unwrap();
        let root = dir.path().join("legacy");
        let config = Config {
            project: Project {
                root: Some(root.to_string_lossy().into_owned()),
                env: Some("staging".into()),
            },
            ..Config::default()
        };
        let resolved = config.resolve(&config_path).unwrap();
        assert_eq!(resolved.project_root, root);
        assert_eq!(resolved.project_env, "staging");
        assert_eq!(resolved.local.env_file, Some(root.join(".env")));
    }

    #[test]
    fn rejects_missing_remote_host_and_empty_file_source() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("dl.js");
        std::fs::write(&config_path, "").unwrap();
        let config = Config {
            remote: Some(Remote::default()),
            ..Config::default()
        };
        assert!(
            config
                .resolve(&config_path)
                .unwrap_err()
                .to_string()
                .contains("remote.host")
        );

        let config = Config {
            files: vec![FileSpec {
                from: String::new(),
                ..FileSpec::default()
            }],
            ..Config::default()
        };
        assert!(
            config
                .resolve(&config_path)
                .unwrap_err()
                .to_string()
                .contains("files[].from")
        );
    }

    #[test]
    fn describe_never_exposes_passwords() {
        let resolved = Resolved {
            config_path: PathBuf::from("/project/dl.js"),
            project_root: PathBuf::from("/project"),
            project_env: "development".into(),
            remote: Some(ResolvedRemote {
                host: "example.com".into(),
                user: "root".into(),
                port: 22,
                app_path: "/srv/app".into(),
                dump_path: "/srv/app/tmp_dump".into(),
                compose: None,
                env_file: None,
                identity_file: None,
                password: Some("remote-secret".into()),
                strict_host_key_checking: true,
                known_hosts: None,
                kubernetes: None,
            }),
            local: ResolvedLocal {
                compose: None,
                env_file: None,
                kubernetes: None,
            },
            database: Some(ResolvedDbConfig {
                engine: Some(Engine::Postgres),
                remote: DbConn::default(),
                local: DbConn::default(),
                remote_service: None,
                local_service: None,
                reset: ResetMode::DropCreate,
                volume: None,
                dump_name: Some("cloner.bak".into()),
                dump_args: Vec::new(),
                restore_args: Vec::new(),
                remote_dump_path: None,
            }),
            files: Vec::new(),
            backup: Backup::default(),
        };
        let db = ResolvedDb {
            engine: Engine::Postgres,
            host: "localhost".into(),
            port: 5432,
            database: "app_development".into(),
            user: "app".into(),
            password: "db-secret".into(),
            service: Some("db".into()),
            in_container: true,
        };
        let json = describe(&resolved, Some(&db), Some(&db)).to_string();
        assert!(!json.contains("db-secret"));
        assert!(!json.contains("remote-secret"));
        assert!(json.contains("********"));
        assert!(json.contains("app_development"));
        assert!(json.contains("drop-create"));
    }
}
