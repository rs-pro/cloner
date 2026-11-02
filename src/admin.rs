//! `cloner db create/reset/drop/migrate/seed` administration.
//!
//! The SQL and the execution target are separate: [`crate::engine`] builds the
//! statements, [`crate::runtime`] decides whether they run on the host, in a
//! Compose service or in a Kubernetes pod.

use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};

use crate::engine::{
    Engine, ResolvedDb, mongo_string, mysql_ident, mysql_literal, pg_ident, pg_literal,
};
use crate::runtime::CommandTarget;
use crate::shell::{self, shq};

#[derive(Clone, Debug)]
pub struct AdminOptions {
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub auth_db: String,
    pub dbname: String,
    pub owner: String,
    pub schema: String,
    pub no_drop: bool,
    pub no_create: bool,
    pub set_password: Option<String>,
    pub target: CommandTarget,
    pub service: Option<String>,
    pub project_root: PathBuf,
    pub hstore: bool,
    pub postgis: bool,
    pub migrate: bool,
    pub seed: bool,
    pub dry_run: bool,
}

impl AdminOptions {
    fn service(&self) -> Option<&str> {
        self.service.as_deref().filter(|s| !s.is_empty())
    }

    fn owner(&self) -> Result<&str> {
        if self.owner.trim().is_empty() {
            bail!("owner/user name is required (pass it as an argument or use --owner)");
        }
        Ok(&self.owner)
    }

    /// Connection info for the maintenance database or the target database.
    fn db(&self, database: &str) -> ResolvedDb {
        ResolvedDb {
            engine: self.engine,
            host: self.host.clone(),
            port: self.port,
            database: database.to_string(),
            user: self.user.clone(),
            password: self.password.clone(),
            service: self.service.clone(),
            in_container: !self.target.is_host(),
        }
    }

    fn auth_db(&self) -> ResolvedDb {
        self.db(&self.auth_db)
    }
}

pub fn create(opts: &AdminOptions) -> Result<()> {
    ensure_started(opts)?;
    match opts.engine {
        Engine::Postgres => pg_create(opts)?,
        Engine::Mysql => mysql_create(opts)?,
        Engine::Mongo => mongo_create(opts)?,
    }
    finish(opts)
}

pub fn reset(opts: &AdminOptions) -> Result<()> {
    ensure_started(opts)?;
    match opts.engine {
        Engine::Postgres => pg_reset(opts)?,
        Engine::Mysql => mysql_reset(opts)?,
        Engine::Mongo => mongo_reset(opts)?,
    }
    finish(opts)
}

pub fn drop_database(opts: &AdminOptions) -> Result<()> {
    ensure_started(opts)?;
    match opts.engine {
        Engine::Postgres => pg_drop(opts)?,
        Engine::Mysql => mysql_drop(opts)?,
        Engine::Mongo => mongo_drop(opts)?,
    }
    Ok(())
}

pub fn run_migrations(opts: &AdminOptions) -> Result<()> {
    run_rails(opts, "db:migrate")
}

pub fn run_seeds(opts: &AdminOptions) -> Result<()> {
    run_rails(opts, "db:seed")
}

fn finish(opts: &AdminOptions) -> Result<()> {
    if opts.hstore || opts.postgis {
        extensions(opts)?;
    }
    if opts.migrate {
        run_rails(opts, "db:migrate")?;
    }
    if opts.seed {
        run_rails(opts, "db:seed")?;
    }
    Ok(())
}

fn run_rails(opts: &AdminOptions, task: &str) -> Result<()> {
    log::info!("running bundle exec rake {task}");
    shell::run(
        &format!("bundle exec rake {task}"),
        Some(&opts.project_root),
        &[],
        None,
        opts.dry_run,
    )
    .map(|_| ())
    .map_err(|error| anyhow!("{error:#}"))
}

fn ensure_started(opts: &AdminOptions) -> Result<()> {
    let Some(prefix) = opts.target.compose_prefix() else {
        return Ok(());
    };
    let service = opts.service().unwrap_or("db");
    log::info!("starting local compose service {service}");
    shell::run(
        &format!("{prefix} up -d {}", shq(service)),
        None,
        &[],
        None,
        opts.dry_run,
    )?;
    if opts.dry_run {
        return Ok(());
    }
    for attempt in 1..=60 {
        if ping(opts).is_ok() {
            log::info!("local database is ready");
            return Ok(());
        }
        log::debug!("waiting for local database ({attempt}/60)");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    bail!("local database did not become ready within 60 seconds")
}

fn ping(opts: &AdminOptions) -> Result<()> {
    let db = opts.auth_db();
    let command = opts.target.wrap(opts.service(), &db.ping_command())?;
    shell::run(&command, None, &[], None, false).map(|_| ())
}

// ---------------------------------------------------------------------------
// Statement execution

fn run_sql(opts: &AdminOptions, db: &ResolvedDb, sql: &str, scalar: bool) -> Result<String> {
    let command = if scalar {
        db.scalar_command(sql)
    } else {
        db.query_command(sql)
    };
    let command = opts.target.wrap(opts.service(), &command)?;
    let output = shell::run(&command, None, &[], None, opts.dry_run)?;
    Ok(output.stdout.trim().to_string())
}

fn pg_exec(opts: &AdminOptions, dbname: &str, sql: &str) -> Result<String> {
    run_sql(opts, &opts.db(dbname), sql, true)
}

fn pg_script(opts: &AdminOptions, dbname: &str, sql: &str) -> Result<()> {
    run_sql(opts, &opts.db(dbname), sql, false).map(|_| ())
}

fn mysql_exec(opts: &AdminOptions, sql: &str) -> Result<String> {
    run_sql(opts, &opts.db(""), sql, false)
}

fn mongo_eval(opts: &AdminOptions, script: &str) -> Result<String> {
    let db = opts.db("");
    let command = db.mongo_eval_command_auth(script, &opts.auth_db);
    let command = opts.target.wrap(opts.service(), &command)?;
    let output = shell::run(&command, None, &[], None, opts.dry_run)?;
    Ok(output.stdout.trim().to_string())
}

// ---------------------------------------------------------------------------
// PostgreSQL

fn protect_postgres_db(opts: &AdminOptions) -> Result<()> {
    if matches!(opts.dbname.as_str(), "postgres" | "template0" | "template1") {
        bail!("refusing to modify system database {}", opts.dbname);
    }
    Ok(())
}

fn pg_exists(opts: &AdminOptions, sql: &str) -> Result<bool> {
    if opts.dry_run {
        // Do not let dry runs stop at "database does not exist"; the caller
        // prints the commands that a real run would execute instead.
        return Ok(true);
    }
    Ok(pg_exec(opts, &opts.auth_db, sql)? == "1")
}

fn pg_create(opts: &AdminOptions) -> Result<()> {
    let owner = opts.owner()?;
    if opts.dbname == "postgres" {
        bail!("refusing to create system database postgres");
    }
    let password = opts.set_password.as_deref().unwrap_or(owner);
    // A dry run cannot query existence, so assume the objects are missing and
    // print the full CREATE/ALTER set.
    let role_exists = !opts.dry_run
        && pg_exists(
            opts,
            &format!(
                "SELECT 1 FROM pg_roles WHERE rolname = {}",
                pg_literal(owner)
            ),
        )?;
    if !role_exists {
        log::info!("creating role {owner}");
        pg_script(
            opts,
            &opts.auth_db,
            &format!(
                "CREATE ROLE {} LOGIN PASSWORD {}",
                pg_ident(owner),
                pg_literal(password)
            ),
        )?;
    } else if opts.set_password.is_some() {
        log::info!("updating password for role {owner}");
        pg_script(
            opts,
            &opts.auth_db,
            &format!(
                "ALTER ROLE {} WITH PASSWORD {}",
                pg_ident(owner),
                pg_literal(password)
            ),
        )?;
    }

    let db_exists = !opts.dry_run
        && pg_exists(
            opts,
            &format!(
                "SELECT 1 FROM pg_database WHERE datname = {}",
                pg_literal(&opts.dbname)
            ),
        )?;
    if !db_exists {
        log::info!("creating database {} owned by {owner}", opts.dbname);
        pg_script(
            opts,
            &opts.auth_db,
            &format!(
                "CREATE DATABASE {} OWNER {}",
                pg_ident(&opts.dbname),
                pg_ident(owner)
            ),
        )?;
    }
    pg_script(
        opts,
        &opts.auth_db,
        &format!(
            "GRANT ALL PRIVILEGES ON DATABASE {} TO {}",
            pg_ident(&opts.dbname),
            pg_ident(owner)
        ),
    )?;
    pg_script(
        opts,
        &opts.dbname,
        &format!("GRANT ALL ON SCHEMA public TO {}", pg_ident(owner)),
    )?;
    Ok(())
}

fn pg_reset(opts: &AdminOptions) -> Result<()> {
    let owner = opts.owner()?;
    protect_postgres_db(opts)?;
    let exists = pg_exists(
        opts,
        &format!(
            "SELECT 1 FROM pg_database WHERE datname = {}",
            pg_literal(&opts.dbname)
        ),
    )?;
    if !exists {
        bail!(
            "database {} does not exist; run `cloner db create` first",
            opts.dbname
        );
    }
    if !opts.no_drop {
        log::info!("dropping schema {} in {}", opts.schema, opts.dbname);
        pg_script(
            opts,
            &opts.dbname,
            &format!("DROP SCHEMA IF EXISTS {} CASCADE", pg_ident(&opts.schema)),
        )?;
    }
    if !opts.no_create {
        log::info!(
            "creating schema {} in {} owned by {owner}",
            opts.schema,
            opts.dbname
        );
        pg_script(
            opts,
            &opts.dbname,
            &format!(
                "CREATE SCHEMA {} AUTHORIZATION {}",
                pg_ident(&opts.schema),
                pg_ident(owner)
            ),
        )?;
        if opts.schema != "public" {
            pg_script(
                opts,
                &opts.dbname,
                &format!(
                    "GRANT ALL ON SCHEMA {} TO {}",
                    pg_ident(&opts.schema),
                    pg_ident(owner)
                ),
            )?;
        }
    }
    Ok(())
}

fn pg_drop(opts: &AdminOptions) -> Result<()> {
    protect_postgres_db(opts)?;
    log::info!("dropping database {}", opts.dbname);
    pg_script(
        opts,
        &opts.auth_db,
        &format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            pg_ident(&opts.dbname)
        ),
    )
}

// ---------------------------------------------------------------------------
// MySQL

fn mysql_create(opts: &AdminOptions) -> Result<()> {
    let owner = opts.owner()?;
    let password = opts.set_password.as_deref().unwrap_or(owner);
    let user_literal = owner.replace('\'', "''");
    log::info!("creating mysql database {} and user {owner}", opts.dbname);
    mysql_exec(
        opts,
        &format!(
            "CREATE DATABASE IF NOT EXISTS {}; CREATE USER IF NOT EXISTS '{}'@'%' IDENTIFIED BY {}; ALTER USER '{}'@'%' IDENTIFIED BY {}; GRANT ALL PRIVILEGES ON {}.* TO '{}'@'%'; FLUSH PRIVILEGES;",
            mysql_ident(&opts.dbname),
            user_literal,
            mysql_literal(password),
            user_literal,
            mysql_literal(password),
            mysql_ident(&opts.dbname),
            user_literal,
        ),
    )?;
    Ok(())
}

fn mysql_reset(opts: &AdminOptions) -> Result<()> {
    if opts.hstore || opts.postgis {
        bail!("hstore/postgis extensions are PostgreSQL-only");
    }
    log::info!("resetting mysql database {}", opts.dbname);
    mysql_exec(
        opts,
        &format!(
            "DROP DATABASE IF EXISTS {}; CREATE DATABASE {};",
            mysql_ident(&opts.dbname),
            mysql_ident(&opts.dbname)
        ),
    )?;
    Ok(())
}

fn mysql_drop(opts: &AdminOptions) -> Result<()> {
    log::info!("dropping mysql database {}", opts.dbname);
    mysql_exec(
        opts,
        &format!("DROP DATABASE IF EXISTS {}", mysql_ident(&opts.dbname)),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// MongoDB

fn mongo_create(opts: &AdminOptions) -> Result<()> {
    let db = mongo_string(&opts.dbname);
    let mut script = format!(
        "const d = db.getSiblingDB({db}); if (!d.getCollectionNames().includes('_cloner_init')) {{ d.createCollection('_cloner_init'); }}"
    );
    if !opts.owner.is_empty() {
        let password = opts.set_password.as_deref().unwrap_or(&opts.owner);
        script.push_str(&format!(
            " try {{ d.createUser({{user: {}, pwd: {}, roles: [{{role: 'readWrite', db: {db}}}]}}); }} catch (e) {{ if (e.code !== 51003) throw e; }}",
            mongo_string(&opts.owner),
            mongo_string(password)
        ));
    }
    log::info!("creating mongo database {}", opts.dbname);
    mongo_eval(opts, &script)?;
    Ok(())
}

fn mongo_reset(opts: &AdminOptions) -> Result<()> {
    if opts.hstore || opts.postgis {
        bail!("hstore/postgis extensions are PostgreSQL-only");
    }
    let db = mongo_string(&opts.dbname);
    log::info!("resetting mongo database {}", opts.dbname);
    mongo_eval(opts, &format!("db.getSiblingDB({db}).dropDatabase()"))?;
    Ok(())
}

fn mongo_drop(opts: &AdminOptions) -> Result<()> {
    let db = mongo_string(&opts.dbname);
    log::info!("dropping mongo database {}", opts.dbname);
    mongo_eval(opts, &format!("db.getSiblingDB({db}).dropDatabase()"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Extensions

fn extensions(opts: &AdminOptions) -> Result<()> {
    if opts.engine != Engine::Postgres {
        bail!("hstore/postgis extensions are PostgreSQL-only");
    }
    if opts.hstore {
        log::info!("creating hstore extension");
        pg_script(opts, &opts.dbname, "CREATE EXTENSION IF NOT EXISTS hstore")?;
    }
    if opts.postgis {
        log::info!("creating postgis extension");
        pg_script(opts, &opts.dbname, "CREATE EXTENSION IF NOT EXISTS postgis")?;
    }
    Ok(())
}

#[allow(dead_code)]
fn project_root_or_cwd(path: Option<&Path>) -> PathBuf {
    path.map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(engine: Engine) -> AdminOptions {
        AdminOptions {
            engine,
            host: "localhost".into(),
            port: engine.default_port(),
            user: "postgres".into(),
            password: "secret".into(),
            auth_db: "postgres".into(),
            dbname: "app_development".into(),
            owner: "app".into(),
            schema: "public".into(),
            no_drop: false,
            no_create: false,
            set_password: None,
            target: CommandTarget::Host,
            service: None,
            project_root: PathBuf::from("/project"),
            hstore: false,
            postgis: false,
            migrate: false,
            seed: false,
            dry_run: true,
        }
    }

    #[test]
    fn service_and_owner_defaults() {
        let mut opts = options(Engine::Postgres);
        assert_eq!(opts.service(), None);
        assert_eq!(opts.owner().unwrap(), "app");

        opts.service = Some("rtdb".into());
        assert_eq!(opts.service(), Some("rtdb"));

        opts.owner = "  ".into();
        assert!(opts.owner().is_err());
    }

    #[test]
    fn builds_postgres_commands() {
        let opts = options(Engine::Postgres);
        let command = opts.auth_db().query_command("SELECT 1");
        assert!(command.contains("PGPASSWORD=secret"));
        assert!(command.contains("psql -v ON_ERROR_STOP=1"));
        assert!(command.contains("-h localhost"));
        assert!(command.contains("-p 5432"));
        assert!(command.contains("-U postgres"));
        assert!(command.contains("-d postgres"));
    }

    #[test]
    fn builds_postgres_compose_commands() {
        let mut opts = options(Engine::Postgres);
        opts.target = CommandTarget::compose("/project/dev-docker", "compose.yml");
        opts.service = Some("db".into());
        let command = opts
            .target
            .wrap(opts.service(), &opts.auth_db().query_command("SELECT 1"))
            .unwrap();
        assert!(command.contains("docker compose -f compose.yml exec -T db"));
        assert!(command.contains("PGPASSWORD=secret"));
        assert!(command.contains("-d postgres"));
    }

    #[test]
    fn builds_kubernetes_commands() {
        let mut opts = options(Engine::Postgres);
        opts.target = CommandTarget::kubernetes(crate::k8s::KubernetesTarget {
            namespace: Some("app".into()),
            pod: Some("pg-0".into()),
            ..Default::default()
        });
        let command = opts
            .target
            .wrap(opts.service(), &opts.auth_db().query_command("SELECT 1"))
            .unwrap();
        assert!(command.starts_with("kubectl --namespace app exec pg-0"));
        assert!(command.contains("psql"));
    }

    #[test]
    fn builds_mysql_commands_with_mariadb_fallback() {
        let opts = options(Engine::Mysql);
        let command = opts.db("").query_command("SELECT 1");
        assert!(command.starts_with("sh -lc 'if command -v mysql"));
        assert!(command.contains("mariadb"));
        assert!(command.contains("--user postgres"));
        assert!(command.contains("--password=secret"));
        assert!(command.contains("--host localhost"));
        assert!(command.contains("--port 3306"));
        assert!(command.contains("-e 'SELECT 1'"));
    }

    #[test]
    fn omits_empty_mysql_password() {
        let mut opts = options(Engine::Mysql);
        opts.password.clear();
        let command = opts.db("").query_command("SELECT 1");
        assert!(!command.contains("--password"));
    }

    #[test]
    fn builds_mongo_commands_with_and_without_auth() {
        let opts = options(Engine::Mongo);
        let command = opts
            .db("")
            .mongo_eval_command_auth("db.ping()", &opts.auth_db);
        assert!(command.contains("mongosh --quiet"));
        assert!(command.contains("--host localhost"));
        assert!(command.contains("--port 27017"));
        assert!(command.contains("--username postgres"));
        assert!(command.contains("--password secret"));
        assert!(command.contains("--authenticationDatabase postgres"));

        let mut anonymous = options(Engine::Mongo);
        anonymous.user.clear();
        let command = anonymous
            .db("")
            .mongo_eval_command_auth("db.ping()", "admin");
        assert!(!command.contains("--username"));
        assert!(!command.contains("--password"));
    }

    #[test]
    fn refuses_system_databases() {
        for name in ["postgres", "template0", "template1"] {
            let mut opts = options(Engine::Postgres);
            opts.dbname = name.into();
            assert!(protect_postgres_db(&opts).is_err());
        }
        assert!(protect_postgres_db(&options(Engine::Postgres)).is_ok());
    }

    #[test]
    fn project_root_defaults_to_cwd() {
        assert_eq!(
            project_root_or_cwd(Some(Path::new("/project"))),
            PathBuf::from("/project")
        );
        assert_eq!(project_root_or_cwd(None), std::env::current_dir().unwrap());
    }
}
