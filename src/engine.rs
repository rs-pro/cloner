//! Database engines, connection resolution and command construction.
//!
//! Everything here is pure: it builds shell command strings but never executes
//! them. The caller decides whether the command runs on the host, inside a
//! Compose service, inside a Kubernetes pod, or over SSH.

use serde::{Deserialize, Serialize};

use crate::shell::shq;

/// Explicit connection overrides from the config schema.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DbConn {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    Postgres,
    Mysql,
    Mongo,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Postgres => "postgres",
            Engine::Mysql => "mysql",
            Engine::Mongo => "mongo",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Engine::Postgres => 5432,
            Engine::Mysql => 3306,
            Engine::Mongo => 27017,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Some(Engine::Postgres),
            "mysql" | "mariadb" => Some(Engine::Mysql),
            "mongo" | "mongodb" => Some(Engine::Mongo),
            _ => None,
        }
    }

    pub fn from_image(image: &str) -> Option<Self> {
        let image = image.to_ascii_lowercase();
        if image.contains("postgres") || image.contains("pgvector") {
            Some(Engine::Postgres)
        } else if image.contains("mysql") || image.contains("mariadb") {
            Some(Engine::Mysql)
        } else if image.contains("mongo") {
            Some(Engine::Mongo)
        } else {
            None
        }
    }

    /// Default dump file name per engine, used when `dump.name` is absent.
    pub fn default_dump_file(self) -> &'static str {
        match self {
            Engine::Postgres => "cloner.bak",
            Engine::Mysql => "cloner.sql",
            Engine::Mongo => "cloner.archive",
        }
    }

    pub fn default_dump_args(self) -> Vec<String> {
        match self {
            Engine::Postgres => vec!["-Fc".to_string()],
            Engine::Mysql => vec![
                "--add-drop-table".to_string(),
                "--single-transaction".to_string(),
            ],
            Engine::Mongo => vec!["--archive".to_string()],
        }
    }

    pub fn default_restore_args(self) -> Vec<String> {
        match self {
            Engine::Postgres => vec![
                "--no-owner".to_string(),
                "-Fc".to_string(),
                "-c".to_string(),
                "--if-exists".to_string(),
            ],
            Engine::Mysql => Vec::new(),
            Engine::Mongo => vec!["--archive".to_string(), "--drop".to_string()],
        }
    }
}

/// One side's resolved database connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDb {
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    /// Compose/Kubernetes service or pod that runs the database, if any.
    pub service: Option<String>,
    /// True when the command runs inside the database container, so `host`
    /// must be `localhost`.
    pub in_container: bool,
}

impl ResolvedDb {
    /// Reuse this connection with another database on the same server.
    pub fn with_database(&self, database: impl Into<String>) -> Self {
        let mut db = self.clone();
        db.database = database.into();
        db
    }

    pub fn dump_file_name(&self, custom: Option<&str>) -> String {
        custom
            .map(str::to_string)
            .unwrap_or_else(|| self.engine.default_dump_file().to_string())
    }

    /// `pg_dump`/`mysqldump`/`mongodump` command, without any container or
    /// SSH wrapper.
    pub fn dump_command(&self, args: &[String]) -> String {
        let args = if args.is_empty() {
            self.engine.default_dump_args()
        } else {
            args.to_vec()
        };
        match self.engine {
            Engine::Postgres => {
                let mut parts = vec!["pg_dump".to_string()];
                parts.extend(args);
                parts.extend(self.pg_connection_args());
                parts.push(shq(&self.database));
                self.with_pg_password(parts.join(" "))
            }
            Engine::Mysql => {
                let parts = self.mysql_args(&args, true);
                mysql_shell("mysqldump", "mariadb-dump", &parts)
            }
            Engine::Mongo => {
                let mut parts = vec!["mongodump".to_string()];
                parts.extend(args);
                parts.push("--uri".to_string());
                parts.push(shq(&self.mongo_uri()));
                parts.join(" ")
            }
        }
    }

    /// `pg_restore`/`mysql`/`mongorestore` command reading a dump from stdin.
    pub fn restore_command(&self, args: &[String]) -> String {
        let args = if args.is_empty() {
            self.engine.default_restore_args()
        } else {
            args.to_vec()
        };
        match self.engine {
            Engine::Postgres => {
                let mut parts = vec!["pg_restore".to_string()];
                parts.extend(args);
                parts.extend(self.pg_connection_args());
                parts.push("-d".to_string());
                parts.push(shq(&self.database));
                self.with_pg_password(parts.join(" "))
            }
            Engine::Mysql => {
                let parts = self.mysql_args(&args, true);
                mysql_shell("mysql", "mariadb", &parts)
            }
            Engine::Mongo => {
                let mut parts = vec!["mongorestore".to_string()];
                parts.extend(args);
                parts.push("--nsInclude".to_string());
                parts.push(shq(&format!("{}.*", self.database)));
                parts.join(" ")
            }
        }
    }

    /// Run SQL (or a JavaScript snippet for Mongo) and return its output.
    pub fn query_command(&self, sql: &str) -> String {
        match self.engine {
            Engine::Postgres => {
                let mut parts = vec![
                    "psql".to_string(),
                    "-v".to_string(),
                    "ON_ERROR_STOP=1".to_string(),
                ];
                parts.extend(self.pg_connection_args());
                parts.push("-d".to_string());
                parts.push(shq(&self.database));
                parts.push("-c".to_string());
                parts.push(shq(sql));
                self.with_pg_password(parts.join(" "))
            }
            Engine::Mysql => {
                let mut args = self.mysql_args(&[], true);
                args.push("-e".to_string());
                args.push(shq(sql));
                mysql_shell("mysql", "mariadb", &args)
            }
            Engine::Mongo => self.mongo_eval_command(sql),
        }
    }

    /// Like [`Self::query_command`] but with psql's tuple-only output, which is
    /// what existence checks need.
    pub fn scalar_command(&self, sql: &str) -> String {
        match self.engine {
            Engine::Postgres => {
                let mut parts = vec![
                    "psql".to_string(),
                    "-v".to_string(),
                    "ON_ERROR_STOP=1".to_string(),
                ];
                parts.extend(self.pg_connection_args());
                parts.push("-d".to_string());
                parts.push(shq(&self.database));
                parts.push("-tAc".to_string());
                parts.push(shq(sql));
                self.with_pg_password(parts.join(" "))
            }
            Engine::Mysql => self.query_command(sql),
            Engine::Mongo => self.mongo_eval_command(sql),
        }
    }

    pub fn mongo_eval_command(&self, script: &str) -> String {
        self.mongo_eval_command_auth(script, "admin")
    }

    pub fn mongo_eval_command_auth(&self, script: &str, auth_db: &str) -> String {
        let mut parts = vec!["mongosh".to_string(), "--quiet".to_string()];
        parts.push("--host".to_string());
        parts.push(shq(&self.host));
        parts.push("--port".to_string());
        parts.push(shq(&self.port.to_string()));
        if !self.user.is_empty() {
            parts.push("--username".to_string());
            parts.push(shq(&self.user));
            if !self.password.is_empty() {
                parts.push("--password".to_string());
                parts.push(shq(&self.password));
            }
            parts.push("--authenticationDatabase".to_string());
            parts.push(shq(if auth_db.is_empty() { "admin" } else { auth_db }));
        }
        parts.push("--eval".to_string());
        parts.push(shq(script));
        parts.join(" ")
    }

    /// Health check used while waiting for a container to start.
    pub fn ping_command(&self) -> String {
        match self.engine {
            Engine::Postgres => format!(
                "pg_isready -h {} -p {} -U {} -d {}",
                shq(&self.host),
                shq(&self.port.to_string()),
                shq(&self.user),
                shq(&self.database)
            ),
            Engine::Mysql => mysql_shell(
                "mysqladmin",
                "mariadb-admin",
                &[
                    "ping".to_string(),
                    format!("--user={}", shq(&self.user)),
                    format!("--password={}", shq(&self.password)),
                    format!("--host={}", shq(&self.host)),
                    format!("--port={}", shq(&self.port.to_string())),
                ],
            ),
            Engine::Mongo => self.mongo_eval_command("db.runCommand({ping:1}).ok"),
        }
    }

    /// Drop and recreate the database (used by `reset: drop-create`).
    pub fn drop_create_command(&self) -> String {
        match self.engine {
            Engine::Postgres => {
                let script =
                    "dropdb --if-exists --force \"$PGDATABASE\" && createdb \"$PGDATABASE\"";
                format!(
                    "env PGHOST={} PGPORT={} PGUSER={} PGDATABASE={} PGPASSWORD={} sh -eu -c {}",
                    shq(&self.host),
                    shq(&self.port.to_string()),
                    shq(&self.user),
                    shq(&self.database),
                    shq(&self.password),
                    shq(script)
                )
            }
            Engine::Mysql => {
                let ident = mysql_ident(&self.database);
                let script = format!("DROP DATABASE IF EXISTS {ident}; CREATE DATABASE {ident};");
                self.query_command(&script)
            }
            Engine::Mongo => self.mongo_eval_command(&format!(
                "db.getSiblingDB({}).dropDatabase()",
                mongo_string(&self.database)
            )),
        }
    }

    fn pg_connection_args(&self) -> Vec<String> {
        vec![
            "-U".to_string(),
            shq(&self.user),
            "-h".to_string(),
            shq(&self.host),
            "-p".to_string(),
            shq(&self.port.to_string()),
        ]
    }

    fn with_pg_password(&self, command: String) -> String {
        if self.password.is_empty() {
            command
        } else {
            format!("env PGPASSWORD={} {}", shq(&self.password), command)
        }
    }

    fn mongo_uri(&self) -> String {
        if self.user.is_empty() {
            format!("mongodb://{}:{}/{}", self.host, self.port, self.database)
        } else {
            format!(
                "mongodb://{}:{}@{}:{}/{}?authSource=admin",
                self.user, self.password, self.host, self.port, self.database
            )
        }
    }

    /// Connection flags shared by the mysql client tools.
    fn mysql_args(&self, args: &[String], with_database: bool) -> Vec<String> {
        let mut parts = args.to_vec();
        parts.push("--user".to_string());
        parts.push(shq(&self.user));
        if !self.password.is_empty() {
            parts.push(format!("--password={}", shq(&self.password)));
        }
        parts.push("--host".to_string());
        parts.push(shq(&self.host));
        parts.push("--port".to_string());
        parts.push(shq(&self.port.to_string()));
        if with_database && !self.database.is_empty() {
            parts.push(shq(&self.database));
        }
        parts
    }
}

/// MariaDB images ship `mariadb`/`mariadb-dump`/`mariadb-admin` instead of the
/// MySQL-named tools. Use the MySQL name when present, otherwise the MariaDB
/// name, without requiring the caller to know which image is running.
pub fn mysql_shell(primary: &str, fallback: &str, args: &[String]) -> String {
    format!(
        "sh -lc 'if command -v {primary} >/dev/null 2>&1; then exec {primary} \"$@\"; else exec {fallback} \"$@\"; fi' sh {}",
        args.join(" ")
    )
}

// ---------------------------------------------------------------------------
// Identifier / literal quoting helpers for admin SQL

pub fn pg_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub fn pg_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn mysql_ident(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

pub fn mysql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub fn mongo_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// Build the connection described in the config schema (explicit overrides).
pub fn db_from_conn(engine: Engine, conn: &DbConn) -> ResolvedDb {
    ResolvedDb {
        engine,
        host: conn.host.clone().unwrap_or_else(|| "localhost".to_string()),
        port: conn.port.unwrap_or_else(|| engine.default_port()),
        database: conn.database.clone().unwrap_or_default(),
        user: conn.user.clone().unwrap_or_default(),
        password: conn.password.clone().unwrap_or_default(),
        service: None,
        in_container: false,
    }
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
            password: "s3cret".into(),
            service: Some("db".into()),
            in_container: true,
        }
    }

    #[test]
    fn engine_metadata_is_stable() {
        assert_eq!(Engine::Postgres.as_str(), "postgres");
        assert_eq!(Engine::Mysql.as_str(), "mysql");
        assert_eq!(Engine::Mongo.as_str(), "mongo");
        assert_eq!(Engine::Postgres.default_port(), 5432);
        assert_eq!(Engine::Mysql.default_port(), 3306);
        assert_eq!(Engine::Mongo.default_port(), 27017);
        assert_eq!(Engine::parse("pg"), Some(Engine::Postgres));
        assert_eq!(Engine::parse("mariadb"), Some(Engine::Mysql));
        assert_eq!(Engine::parse("mongodb"), Some(Engine::Mongo));
        assert_eq!(Engine::parse("sqlite"), None);
        assert_eq!(
            Engine::from_image("pgvector/pgvector:pg17"),
            Some(Engine::Postgres)
        );
        assert_eq!(Engine::from_image("redis:7"), None);
    }

    #[test]
    fn chooses_dump_file_names() {
        assert_eq!(db(Engine::Postgres).dump_file_name(None), "cloner.bak");
        assert_eq!(db(Engine::Mysql).dump_file_name(None), "cloner.sql");
        assert_eq!(db(Engine::Mongo).dump_file_name(None), "cloner.archive");
        assert_eq!(
            db(Engine::Postgres).dump_file_name(Some("custom.dump")),
            "custom.dump"
        );
    }

    #[test]
    fn builds_postgres_dump_and_restore() {
        let dump = db(Engine::Postgres).dump_command(&[]);
        assert!(dump.starts_with("env PGPASSWORD=s3cret pg_dump -Fc"));
        assert!(dump.contains("-U app -h localhost -p 5432 app_development"));

        let restore = db(Engine::Postgres).restore_command(&[]);
        assert!(restore.contains("pg_restore --no-owner -Fc -c --if-exists"));
        assert!(restore.contains("-d app_development"));
    }

    #[test]
    fn builds_mysql_commands_with_mariadb_fallback() {
        let dump = db(Engine::Mysql).dump_command(&[]);
        assert!(dump.starts_with("sh -lc 'if command -v mysqldump"));
        assert!(dump.contains("else exec mariadb-dump"));
        assert!(dump.contains("--user app"));
        assert!(dump.contains("--password=s3cret"));

        let restore = db(Engine::Mysql).restore_command(&[]);
        assert!(restore.contains("command -v mysql"));
        assert!(restore.contains("else exec mariadb"));
    }

    #[test]
    fn builds_mongo_commands_with_uri_and_ns_include() {
        let dump = db(Engine::Mongo).dump_command(&[]);
        assert!(dump.starts_with("mongodump --archive --uri"));
        assert!(
            dump.contains("mongodb://app:s3cret@localhost:27017/app_development?authSource=admin")
        );

        let restore = db(Engine::Mongo).restore_command(&[]);
        assert!(restore.contains("mongorestore --archive --drop"));
        assert!(restore.contains("--nsInclude 'app_development.*'"));
    }

    #[test]
    fn builds_query_commands() {
        let pg = db(Engine::Postgres).query_command("SELECT 1");
        assert!(pg.contains("psql -v ON_ERROR_STOP=1"));
        assert!(pg.ends_with("-c 'SELECT 1'"));

        let mysql = db(Engine::Mysql).query_command("SELECT 1");
        assert!(mysql.contains("--user app"));
        assert!(mysql.contains("-e 'SELECT 1'"));

        let mongo = db(Engine::Mongo).query_command("db.ping()");
        assert!(mongo.contains("mongosh --quiet"));
        assert!(mongo.contains("--eval 'db.ping()'"));
    }

    #[test]
    fn scalar_command_uses_tuples_only_for_postgres() {
        let pg = db(Engine::Postgres).scalar_command("SELECT 1");
        assert!(pg.contains("-tAc"));
        assert!(pg.ends_with("-tAc 'SELECT 1'"));
        let mysql = db(Engine::Mysql).scalar_command("SELECT 1");
        assert!(mysql.contains("-e 'SELECT 1'"));
    }

    #[test]
    fn builds_ping_and_drop_create_commands() {
        let pg = db(Engine::Postgres);
        assert!(pg.ping_command().contains("pg_isready"));
        assert!(pg.drop_create_command().contains("env PGHOST=localhost"));
        assert!(
            pg.drop_create_command()
                .contains("dropdb --if-exists --force")
        );

        let mysql = db(Engine::Mysql);
        assert!(mysql.ping_command().contains("mysqladmin"));
        let drop_create = mysql.drop_create_command();
        assert!(drop_create.contains("DROP DATABASE IF EXISTS"));
        assert!(drop_create.contains("app_development"));

        let mongo = db(Engine::Mongo);
        assert!(mongo.ping_command().contains("runCommand({ping:1})"));
        assert!(
            mongo
                .drop_create_command()
                .contains("db.getSiblingDB(\"app_development\").dropDatabase()")
        );
    }

    #[test]
    fn quotes_identifiers_and_literals() {
        assert_eq!(pg_ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(pg_literal("it's"), "'it''s'");
        assert_eq!(mysql_ident("we`ird"), "`we``ird`");
        assert_eq!(mysql_literal("it's"), "'it\\'s'");
        assert_eq!(mongo_string("line\nbreak"), "\"line\\nbreak\"");
    }

    #[test]
    fn empty_password_is_omitted() {
        let mut pg = db(Engine::Postgres);
        pg.password.clear();
        assert!(!pg.dump_command(&[]).contains("PGPASSWORD"));

        let mut mysql = db(Engine::Mysql);
        mysql.password.clear();
        assert!(!mysql.query_command("SELECT 1").contains("--password"));
    }
}
