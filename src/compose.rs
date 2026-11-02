//! Docker Compose parsing and database connection inference.
//!
//! The same inference is used for the local and remote side, and by the
//! Kubernetes helpers when a pod env is read with `kubectl exec`.

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeMap;

use crate::engine::{DbConn, Engine, ResolvedDb};
use crate::util::interpolate;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceInfo {
    pub image: String,
    pub env: BTreeMap<String, String>,
    pub env_files: Vec<String>,
}

/// Parse a compose file and return all services.
pub fn parse_compose_services(
    yaml: &str,
    env: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ServiceInfo>> {
    let doc: serde_yaml::Value = serde_yaml::from_str(yaml).context("parse compose YAML")?;
    let services = doc
        .get("services")
        .and_then(|v| v.as_mapping())
        .ok_or_else(|| anyhow!("compose file has no services mapping"))?;
    let mut out = BTreeMap::new();
    for (name, value) in services {
        let Some(name) = name.as_str() else { continue };
        let Some(map) = value.as_mapping() else {
            continue;
        };
        let image = map
            .get("image")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut service_env = BTreeMap::new();
        if let Some(environment) = map.get("environment") {
            match environment {
                serde_yaml::Value::Mapping(m) => {
                    for (k, v) in m {
                        let Some(key) = k.as_str() else { continue };
                        let value = yaml_scalar_to_string(v);
                        service_env
                            .insert(key.to_string(), interpolate(&value, env).unwrap_or(value));
                    }
                }
                serde_yaml::Value::Sequence(items) => {
                    for item in items {
                        let Some(entry) = item.as_str() else { continue };
                        if let Some((key, value)) = entry.split_once('=') {
                            let value =
                                interpolate(value, env).unwrap_or_else(|_| value.to_string());
                            service_env.insert(key.trim().to_string(), value);
                        } else if let Some(value) = env.get(entry.trim()) {
                            service_env.insert(entry.trim().to_string(), value.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        let mut env_files = Vec::new();
        if let Some(value) = map.get("env_file") {
            match value {
                serde_yaml::Value::String(s) => env_files.push(s.clone()),
                serde_yaml::Value::Sequence(items) => {
                    for item in items {
                        match item {
                            serde_yaml::Value::String(s) => env_files.push(s.clone()),
                            serde_yaml::Value::Mapping(m) => {
                                if let Some(path) = m.get("path").and_then(|v| v.as_str()) {
                                    env_files.push(path.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        out.insert(
            name.to_string(),
            ServiceInfo {
                image,
                env: service_env,
                env_files,
            },
        );
    }
    Ok(out)
}

fn yaml_scalar_to_string(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Null => String::new(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

pub fn detect_service(
    services: &BTreeMap<String, ServiceInfo>,
    engine: Option<Engine>,
) -> Option<String> {
    let mut best: Option<(i32, String)> = None;
    for (name, info) in services {
        let image = info.image.to_ascii_lowercase();
        let mut score = 0;
        if image.contains("postgres") || image.contains("pgvector") {
            score = 100;
            if engine == Some(Engine::Postgres) {
                score += 20;
            }
        } else if image.contains("mysql") || image.contains("mariadb") {
            score = 100;
            if engine == Some(Engine::Mysql) {
                score += 20;
            }
        } else if image.contains("mongo") {
            score = 100;
            if engine == Some(Engine::Mongo) {
                score += 20;
            }
        }
        if info.env.contains_key("POSTGRES_DB") {
            score = score.max(90);
            if engine == Some(Engine::Postgres) {
                score += 20;
            }
        }
        if info.env.contains_key("MYSQL_DATABASE") || info.env.contains_key("MYSQL_ROOT_PASSWORD") {
            score = score.max(90);
            if engine == Some(Engine::Mysql) {
                score += 20;
            }
        }
        if info.env.contains_key("MONGO_INITDB_DATABASE") {
            score = score.max(90);
            if engine == Some(Engine::Mongo) {
                score += 20;
            }
        }
        let lname = name.to_ascii_lowercase();
        if matches!(
            lname.as_str(),
            "db" | "database" | "postgres" | "postgresql" | "mysql" | "mariadb" | "mongo" | "rtdb"
        ) {
            score = score.max(60);
        }
        if score > 0 && best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, name.clone()));
        }
    }
    best.map(|(_, name)| name)
}

pub fn engine_from_env(env: &BTreeMap<String, String>) -> Option<Engine> {
    if env.contains_key("POSTGRES_DB")
        || env.contains_key("POSTGRES_USER")
        || env.contains_key("PGDATABASE")
    {
        return Some(Engine::Postgres);
    }
    if env.contains_key("MYSQL_DATABASE")
        || env.contains_key("MYSQL_ROOT_PASSWORD")
        || env.contains_key("MARIADB_DATABASE")
    {
        return Some(Engine::Mysql);
    }
    if env.contains_key("MONGO_INITDB_DATABASE") || env.contains_key("MONGO_URI") {
        return Some(Engine::Mongo);
    }
    None
}

fn pick(env: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = env.get(*key)
            && !value.is_empty()
        {
            return Some(value.clone());
        }
    }
    None
}

/// Resolve one side's database connection from compose + env + explicit
/// overrides.
pub fn infer_db(
    engine_hint: Option<Engine>,
    overrides: &DbConn,
    app_env: &BTreeMap<String, String>,
    compose_yaml: Option<&str>,
    compose_service: Option<&str>,
    env_file_env: &BTreeMap<String, String>,
    in_container: bool,
) -> Result<ResolvedDb> {
    let mut engine = engine_hint.or_else(|| engine_from_env(app_env));
    let mut service_env = BTreeMap::new();
    let mut service_name = compose_service.map(str::to_string);

    if let Some(yaml) = compose_yaml {
        let services = parse_compose_services(yaml, app_env)?;
        let chosen = service_name
            .clone()
            .or_else(|| detect_service(&services, engine));
        if let Some(name) = chosen {
            let info = services
                .get(&name)
                .ok_or_else(|| anyhow!("compose service {name:?} not found"))?;
            if engine.is_none() {
                engine = engine_from_env(&info.env).or_else(|| {
                    let image = info.image.to_ascii_lowercase();
                    if image.contains("postgres") || image.contains("pgvector") {
                        Some(Engine::Postgres)
                    } else if image.contains("mysql") || image.contains("mariadb") {
                        Some(Engine::Mysql)
                    } else if image.contains("mongo") {
                        Some(Engine::Mongo)
                    } else {
                        None
                    }
                });
            }
            service_env = info.env.clone();
            service_name = Some(name);
        }
    }

    let engine = engine
        .or(engine_hint)
        .ok_or_else(|| anyhow!("cannot determine database engine; set database.engine"))?;

    // Priority: explicit override > compose service environment > env file >
    // application env.
    let merged = {
        let mut m = env_file_env.clone();
        m.extend(app_env.clone());
        m.extend(service_env);
        m
    };

    let database = overrides
        .database
        .clone()
        .or_else(|| match engine {
            Engine::Postgres => pick(
                &merged,
                &["POSTGRES_DB", "PGDATABASE", "DB_NAME", "DB_DATABASE"],
            ),
            Engine::Mysql => pick(
                &merged,
                &[
                    "MYSQL_DATABASE",
                    "MARIADB_DATABASE",
                    "DB_NAME",
                    "DB_DATABASE",
                ],
            ),
            Engine::Mongo => pick(
                &merged,
                &["MONGO_INITDB_DATABASE", "DB_NAME", "DB_DATABASE"],
            ),
        })
        .ok_or_else(|| anyhow!("cannot determine database name; set database.<side>.database"))?;

    let user = overrides
        .user
        .clone()
        .or_else(|| match engine {
            Engine::Postgres => pick(
                &merged,
                &["POSTGRES_USER", "PGUSER", "DB_USER", "DB_USERNAME"],
            ),
            Engine::Mysql => pick(
                &merged,
                &["MYSQL_USER", "MARIADB_USER", "DB_USER", "DB_USERNAME"],
            ),
            Engine::Mongo => pick(
                &merged,
                &["MONGO_INITDB_ROOT_USERNAME", "DB_USER", "DB_USERNAME"],
            ),
        })
        .unwrap_or_default();

    let password = overrides
        .password
        .clone()
        .or_else(|| match engine {
            Engine::Postgres => pick(&merged, &["POSTGRES_PASSWORD", "PGPASSWORD", "DB_PASSWORD"]),
            Engine::Mysql => pick(
                &merged,
                &[
                    "MYSQL_PASSWORD",
                    "MYSQL_ROOT_PASSWORD",
                    "MARIADB_PASSWORD",
                    "DB_PASSWORD",
                ],
            ),
            Engine::Mongo => pick(&merged, &["MONGO_INITDB_ROOT_PASSWORD", "DB_PASSWORD"]),
        })
        .unwrap_or_default();

    let host = overrides.host.clone().unwrap_or_else(|| {
        if in_container {
            "localhost".to_string()
        } else {
            pick(&merged, &["DB_HOST", "PGHOST", "MYSQL_HOST", "MONGO_HOST"])
                .unwrap_or_else(|| "localhost".to_string())
        }
    });
    let port = overrides.port.unwrap_or_else(|| {
        if in_container {
            engine.default_port()
        } else {
            pick(&merged, &["DB_PORT", "PGPORT", "MYSQL_PORT", "MONGO_PORT"])
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| engine.default_port())
        }
    });

    Ok(ResolvedDb {
        engine,
        host,
        port,
        database,
        user,
        password,
        service: service_name,
        in_container,
    })
}
