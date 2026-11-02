use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};

use crate::config::{Engine, ResolvedCompose};
use crate::db::{compose_prefix, mysql_shell, run_local};
use crate::util::shq;

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
    pub compose: Option<ResolvedCompose>,
    pub service: Option<String>,
    pub project_root: PathBuf,
    pub hstore: bool,
    pub postgis: bool,
    pub migrate: bool,
    pub seed: bool,
    pub dry_run: bool,
}

impl AdminOptions {
    fn service(&self) -> &str {
        self.service.as_deref().unwrap_or("db")
    }

    fn owner(&self) -> Result<&str> {
        if self.owner.trim().is_empty() {
            bail!("owner/user name is required (pass it as an argument or use --owner)");
        }
        Ok(&self.owner)
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
    run_local(
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
    let Some(compose) = &opts.compose else {
        return Ok(());
    };
    log::info!("starting local compose service {}", opts.service());
    run_local(
        &format!("{} up -d {}", compose_prefix(compose), shq(opts.service())),
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
    match opts.engine {
        Engine::Postgres => {
            let command = format!("{} -tAc 'SELECT 1'", pg_base(opts, &opts.auth_db));
            run_local(&command, None, &[], None, false).map(|_| ())
        }
        Engine::Mysql => {
            let command = format!("{} --execute 'SELECT 1'", mysql_base(opts, None));
            run_local(&command, None, &[], None, false).map(|_| ())
        }
        Engine::Mongo => {
            let command = format!("{} --eval 'db.runCommand({{ping:1}}).ok'", mongo_base(opts));
            run_local(&command, None, &[], None, false).map(|_| ())
        }
    }
}

// ---------------------------------------------------------------------------
// SQL helpers

fn ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn mysql_ident(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

fn mysql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn pg_base(opts: &AdminOptions, dbname: &str) -> String {
    if let Some(compose) = &opts.compose {
        format!(
            "{} exec -T {} env PGHOST=localhost PGPORT=5432 PGUSER={} PGPASSWORD={} psql -v ON_ERROR_STOP=1 -U {} -d {}",
            compose_prefix(compose),
            shq(opts.service()),
            shq(&opts.user),
            shq(&opts.password),
            shq(&opts.user),
            shq(dbname)
        )
    } else {
        format!(
            "PGPASSWORD={} psql -v ON_ERROR_STOP=1 -h {} -p {} -U {} -d {}",
            shq(&opts.password),
            shq(&opts.host),
            shq(&opts.port.to_string()),
            shq(&opts.user),
            shq(dbname)
        )
    }
}

fn pg_exec(opts: &AdminOptions, dbname: &str, sql: &str) -> Result<String> {
    let command = format!("{} -tAc {}", pg_base(opts, dbname), shq(sql));
    let output = run_local(&command, None, &[], None, opts.dry_run)?;
    Ok(output.stdout.trim().to_string())
}

fn pg_script(opts: &AdminOptions, dbname: &str, sql: &str) -> Result<()> {
    let command = format!("{} -c {}", pg_base(opts, dbname), shq(sql));
    run_local(&command, None, &[], None, opts.dry_run).map(|_| ())
}

fn mysql_base(opts: &AdminOptions, dbname: Option<&str>) -> String {
    let mut args = vec![format!("--user={}", shq(&opts.user))];
    if !opts.password.is_empty() {
        args.push(format!("--password={}", shq(&opts.password)));
    }
    args.push(format!("--host={}", shq(&opts.host)));
    args.push(format!("--port={}", shq(&opts.port.to_string())));
    if let Some(dbname) = dbname
        && !dbname.is_empty()
    {
        args.push(shq(dbname));
    }
    let client = mysql_shell("mysql", "mariadb", &args);
    if let Some(compose) = &opts.compose {
        format!(
            "{} exec -T {} {}",
            compose_prefix(compose),
            shq(opts.service()),
            client
        )
    } else {
        client
    }
}

fn mysql_exec(opts: &AdminOptions, dbname: Option<&str>, sql: &str) -> Result<String> {
    let command = format!("{} --execute {}", mysql_base(opts, dbname), shq(sql));
    let output = run_local(&command, None, &[], None, opts.dry_run)?;
    Ok(output.stdout.trim().to_string())
}

fn mongo_base(opts: &AdminOptions) -> String {
    let mut parts = vec!["mongosh".to_string(), "--quiet".to_string()];
    if let Some(compose) = &opts.compose {
        parts = vec![
            format!("{} exec -T", compose_prefix(compose)),
            shq(opts.service()),
            "mongosh".to_string(),
            "--quiet".to_string(),
        ];
    }
    parts.push(format!("--host {}", shq(&opts.host)));
    parts.push(format!("--port {}", shq(&opts.port.to_string())));
    if !opts.user.is_empty() {
        parts.push(format!("--username {}", shq(&opts.user)));
        if !opts.password.is_empty() {
            parts.push(format!("--password {}", shq(&opts.password)));
        }
        if !opts.auth_db.is_empty() {
            parts.push(format!("--authenticationDatabase {}", shq(&opts.auth_db)));
        }
    }
    parts.join(" ")
}

fn mongo_eval(opts: &AdminOptions, script: &str) -> Result<String> {
    let command = format!("{} --eval {}", mongo_base(opts), shq(script));
    let output = run_local(&command, None, &[], None, opts.dry_run)?;
    Ok(output.stdout.trim().to_string())
}

fn mongo_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
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
            &format!("SELECT 1 FROM pg_roles WHERE rolname = {}", literal(owner)),
        )?;
    if !role_exists {
        log::info!("creating role {owner}");
        pg_script(
            opts,
            &opts.auth_db,
            &format!(
                "CREATE ROLE {} LOGIN PASSWORD {}",
                ident(owner),
                literal(password)
            ),
        )?;
    } else if opts.set_password.is_some() {
        log::info!("updating password for role {owner}");
        pg_script(
            opts,
            &opts.auth_db,
            &format!(
                "ALTER ROLE {} WITH PASSWORD {}",
                ident(owner),
                literal(password)
            ),
        )?;
    }

    let db_exists = !opts.dry_run
        && pg_exists(
            opts,
            &format!(
                "SELECT 1 FROM pg_database WHERE datname = {}",
                literal(&opts.dbname)
            ),
        )?;
    if !db_exists {
        log::info!("creating database {} owned by {owner}", opts.dbname);
        pg_script(
            opts,
            &opts.auth_db,
            &format!(
                "CREATE DATABASE {} OWNER {}",
                ident(&opts.dbname),
                ident(owner)
            ),
        )?;
    }
    pg_script(
        opts,
        &opts.auth_db,
        &format!(
            "GRANT ALL PRIVILEGES ON DATABASE {} TO {}",
            ident(&opts.dbname),
            ident(owner)
        ),
    )?;
    pg_script(
        opts,
        &opts.dbname,
        &format!("GRANT ALL ON SCHEMA public TO {}", ident(owner)),
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
            literal(&opts.dbname)
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
            &format!("DROP SCHEMA IF EXISTS {} CASCADE", ident(&opts.schema)),
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
                ident(&opts.schema),
                ident(owner)
            ),
        )?;
        if opts.schema != "public" {
            pg_script(
                opts,
                &opts.dbname,
                &format!(
                    "GRANT ALL ON SCHEMA {} TO {}",
                    ident(&opts.schema),
                    ident(owner)
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
            ident(&opts.dbname)
        ),
    )
}

// ---------------------------------------------------------------------------
// MySQL

fn mysql_create(opts: &AdminOptions) -> Result<()> {
    let owner = opts.owner()?;
    let password = opts.set_password.as_deref().unwrap_or(owner);
    log::info!("creating mysql database {} and user {owner}", opts.dbname);
    mysql_exec(
        opts,
        None,
        &format!(
            "CREATE DATABASE IF NOT EXISTS {}; CREATE USER IF NOT EXISTS '{}'@'%' IDENTIFIED BY {}; ALTER USER '{}'@'%' IDENTIFIED BY {}; GRANT ALL PRIVILEGES ON {}.* TO '{}'@'%'; FLUSH PRIVILEGES;",
            mysql_ident(&opts.dbname),
            owner.replace('\'', "''"),
            mysql_literal(password),
            owner.replace('\'', "''"),
            mysql_literal(password),
            mysql_ident(&opts.dbname),
            owner.replace('\'', "''"),
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
        None,
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
        None,
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
            compose: None,
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
    fn quotes_postgres_identifiers_and_literals() {
        assert_eq!(ident("public"), "\"public\"");
        assert_eq!(ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(literal("plain"), "'plain'");
        assert_eq!(literal("it's"), "'it''s'");
    }

    #[test]
    fn quotes_mysql_identifiers_and_literals() {
        assert_eq!(mysql_ident("app"), "`app`");
        assert_eq!(mysql_ident("we`ird"), "`we``ird`");
        assert_eq!(mysql_literal("plain"), "'plain'");
        assert_eq!(mysql_literal("back\\slash"), "'back\\\\slash'");
        assert_eq!(mysql_literal("it's"), "'it\\'s'");
    }

    #[test]
    fn encodes_mongo_strings_as_json() {
        assert_eq!(mongo_string("plain"), "\"plain\"");
        assert_eq!(mongo_string("it's"), "\"it's\"");
        assert_eq!(mongo_string("line\nbreak"), "\"line\\nbreak\"");
    }

    #[test]
    fn service_and_owner_defaults() {
        let mut opts = options(Engine::Postgres);
        assert_eq!(opts.service(), "db");
        assert_eq!(opts.owner().unwrap(), "app");

        opts.service = Some("rtdb".into());
        assert_eq!(opts.service(), "rtdb");

        opts.owner = "  ".into();
        assert!(opts.owner().is_err());
    }

    #[test]
    fn builds_postgres_commands() {
        let opts = options(Engine::Postgres);
        let command = pg_base(&opts, "app_development");
        assert!(command.contains("PGPASSWORD=secret"));
        assert!(command.contains("psql -v ON_ERROR_STOP=1"));
        assert!(command.contains("-h localhost"));
        assert!(command.contains("-p 5432"));
        assert!(command.contains("-U postgres"));
        assert!(command.contains("-d app_development"));
    }

    #[test]
    fn builds_postgres_compose_commands() {
        let mut opts = options(Engine::Postgres);
        opts.compose = Some(ResolvedCompose {
            path: "/project/dev-docker".into(),
            file: "compose.yml".into(),
            service: None,
        });
        let command = pg_base(&opts, "app_development");
        assert!(command.contains("docker compose -f compose.yml exec -T db"));
        assert!(command.contains("PGHOST=localhost PGPORT=5432"));
        assert!(command.contains("-d app_development"));
    }

    #[test]
    fn builds_mysql_commands_with_mariadb_fallback() {
        let opts = options(Engine::Mysql);
        let command = mysql_base(&opts, Some("app_development"));
        assert!(command.starts_with("sh -lc 'if command -v mysql"));
        assert!(command.contains("mariadb"));
        assert!(command.contains("--user=postgres"));
        assert!(command.contains("--password=secret"));
        assert!(command.contains("--host=localhost"));
        assert!(command.contains("--port=3306"));
        assert!(command.ends_with("app_development"));
    }

    #[test]
    fn omits_empty_mysql_password() {
        let mut opts = options(Engine::Mysql);
        opts.password.clear();
        let command = mysql_base(&opts, None);
        assert!(!command.contains("--password"));
    }

    #[test]
    fn builds_mongo_commands_with_and_without_auth() {
        let opts = options(Engine::Mongo);
        let command = mongo_base(&opts);
        assert!(command.contains("mongosh --quiet"));
        assert!(command.contains("--host localhost"));
        assert!(command.contains("--port 27017"));
        assert!(command.contains("--username postgres"));
        assert!(command.contains("--password secret"));
        assert!(command.contains("--authenticationDatabase postgres"));

        let mut anonymous = options(Engine::Mongo);
        anonymous.user.clear();
        let command = mongo_base(&anonymous);
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
