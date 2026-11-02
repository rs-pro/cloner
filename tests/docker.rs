//! End-to-end database tests against real Compose containers.
//!
//! These are opt-in because they pull/start Docker images:
//!
//! ```bash
//! CLONER_DOCKER_TESTS=1 cargo test --test docker -- --nocapture
//! ```
//!
//! The test starts PostgreSQL, MariaDB and MongoDB with Compose, then exercises
//! dump → reset → restore → query through the same command path the CLI uses.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cloner::compose::infer_db;
use cloner::config::{ResetMode, ResolvedCompose, ResolvedDbConfig, ResolvedLocal};
use cloner::db;
use cloner::engine::{Engine, ResolvedDb};
use cloner::runtime::CommandTarget;
use cloner::shell;

fn enabled() -> bool {
    std::env::var("CLONER_DOCKER_TESTS").is_ok_and(|value| value != "0" && !value.is_empty())
}

fn docker(args: &[&str], cwd: &Path) -> (bool, String) {
    let output = Command::new("docker")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("run docker {args:?}: {error}"));
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

struct ComposeProject {
    dir: tempfile::TempDir,
}

impl ComposeProject {
    fn new(yaml: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("compose.yml"), yaml).unwrap();
        let (ok, text) = docker(&["compose", "-f", "compose.yml", "up", "-d"], dir.path());
        assert!(ok, "docker compose up failed:\n{text}");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for ComposeProject {
    fn drop(&mut self) {
        let _ = docker(
            &[
                "compose",
                "-f",
                "compose.yml",
                "down",
                "-v",
                "--remove-orphans",
            ],
            self.dir.path(),
        );
    }
}

fn wait_ready(target: &CommandTarget, service: Option<&str>, db: &ResolvedDb) {
    let command = target.wrap(service, &db.ping_command()).unwrap();
    let mut last = String::new();
    for _ in 1..=90 {
        match shell::run_capture(&command, None, &[], None) {
            Ok(output) if output.code == 0 => return,
            Ok(output) => last = format!("stdout: {}\nstderr: {}", output.stdout, output.stderr),
            Err(error) => last = format!("{error:#}"),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    panic!("database did not become ready via {command}\n{last}");
}

fn local_config(dir: &Path, db: &ResolvedDb) -> ResolvedLocal {
    let _ = db;
    ResolvedLocal {
        compose: Some(ResolvedCompose {
            path: dir.display().to_string(),
            file: "compose.yml".into(),
            service: db.service.clone(),
        }),
        env_file: None,
        kubernetes: None,
    }
}

fn db_config(engine: Engine) -> ResolvedDbConfig {
    ResolvedDbConfig {
        engine: Some(engine),
        remote: Default::default(),
        local: Default::default(),
        remote_service: None,
        local_service: None,
        reset: ResetMode::DropCreate,
        volume: None,
        dump_name: None,
        dump_args: Vec::new(),
        restore_args: Vec::new(),
        remote_dump_path: None,
    }
}

fn resolve(yaml: &str) -> ResolvedDb {
    infer_db(
        None,
        &Default::default(),
        &BTreeMap::new(),
        Some(yaml),
        None,
        &BTreeMap::new(),
        true,
    )
    .expect("resolve database from compose")
}

fn assert_size(path: &Path) {
    let size = std::fs::metadata(path).expect("dump exists").len();
    assert!(size > 0, "dump {} is empty", path.display());
}

fn round_trip(engine: Engine, compose: &str, setup_sql: &str, verify_sql: &str, expected: &str) {
    let project = ComposeProject::new(compose);
    let mut db = resolve(compose);
    assert_eq!(db.engine, engine, "engine detection");
    if engine == Engine::Mysql {
        // DROP/CREATE DATABASE needs the administrative account.
        db.user = "root".into();
        db.password = "rootsecret".into();
    }
    let target = CommandTarget::compose(project.path().display().to_string(), "compose.yml");
    let service = db.service.as_deref();
    wait_ready(&target, service, &db);

    let run_sql = |sql: &str| {
        let command = target.wrap(service, &db.query_command(sql)).unwrap();
        shell::run(&command, None, &[], None, false)
            .unwrap_or_else(|error| panic!("{engine:?} query failed:\n{error:#}"))
    };
    run_sql(setup_sql);

    let local = local_config(project.path(), &db);
    let cfg = db_config(engine);
    let dump = project.path().join("dump").join("cloner.dump");
    let size = db::dump_to_path(&target, service, &db, &[], &dump, false)
        .unwrap_or_else(|error| panic!("{engine:?} dump failed:\n{error:#}"));
    assert!(size > 0);
    assert_size(&dump);

    db::reset_local(&local, &cfg, &db, project.path(), false)
        .unwrap_or_else(|error| panic!("{engine:?} reset failed:\n{error:#}"));
    wait_ready(&target, service, &db);

    db::restore_local(&local, &cfg, &db, &dump, false)
        .unwrap_or_else(|error| panic!("{engine:?} restore failed:\n{error:#}"));

    let output = run_sql(verify_sql);
    assert!(
        output.stdout.contains(expected),
        "{engine:?} verification failed: expected {expected:?} in\nstdout: {}\nstderr: {}",
        output.stdout,
        output.stderr
    );
}

#[test]
fn compose_databases_round_trip() {
    if !enabled() {
        eprintln!("skipping Docker tests; set CLONER_DOCKER_TESTS=1 to run them");
        return;
    }

    round_trip(
        Engine::Postgres,
        r#"
services:
  db:
    image: postgres:17-alpine
    environment:
      POSTGRES_DB: app_test
      POSTGRES_USER: app
      POSTGRES_PASSWORD: secret
"#,
        "CREATE TABLE items (id serial primary key, name text); INSERT INTO items (name) VALUES ('alpha'), ('beta');",
        "SELECT count(*) FROM items;",
        "2",
    );

    round_trip(
        Engine::Mysql,
        r#"
services:
  db:
    image: mariadb:11
    environment:
      MYSQL_DATABASE: app_test
      MYSQL_USER: app
      MYSQL_PASSWORD: secret
      MYSQL_ROOT_PASSWORD: rootsecret
"#,
        "CREATE TABLE items (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(64)); INSERT INTO items (name) VALUES ('alpha'), ('beta');",
        "SELECT COUNT(*) FROM items;",
        "2",
    );

    round_trip(
        Engine::Mongo,
        r#"
services:
  db:
    image: mongo:7
    environment:
      MONGO_INITDB_DATABASE: app_test
"#,
        "db.getSiblingDB('app_test').items.insertMany([{name:'alpha'},{name:'beta'}]);",
        "db.getSiblingDB('app_test').items.countDocuments();",
        "2",
    );
}

#[test]
fn compose_admin_commands_create_and_drop() {
    if !enabled() {
        eprintln!("skipping Docker tests; set CLONER_DOCKER_TESTS=1 to run them");
        return;
    }
    use cloner::admin::{self, AdminOptions};

    let compose = r#"
services:
  db:
    image: postgres:17-alpine
    environment:
      POSTGRES_DB: app_test
      POSTGRES_USER: app
      POSTGRES_PASSWORD: secret
"#;
    let project = ComposeProject::new(compose);
    let db = resolve(compose);
    let target = CommandTarget::compose(project.path().display().to_string(), "compose.yml");
    wait_ready(&target, db.service.as_deref(), &db);

    let opts = AdminOptions {
        engine: Engine::Postgres,
        host: "localhost".into(),
        port: 5432,
        user: "app".into(),
        password: "secret".into(),
        auth_db: "app_test".into(),
        dbname: "scratch_test".into(),
        owner: "app".into(),
        schema: "public".into(),
        no_drop: false,
        no_create: false,
        set_password: None,
        target: target.clone(),
        service: db.service.clone(),
        project_root: PathBuf::from(project.path()),
        hstore: false,
        postgis: false,
        migrate: false,
        seed: false,
        dry_run: false,
    };

    admin::create(&opts).unwrap();
    let query = opts
        .target
        .wrap(
            opts.service.as_deref(),
            &opts_auth(&opts).query_command("SELECT 1"),
        )
        .unwrap();
    shell::run(&query, None, &[], None, false).unwrap();
    admin::drop_database(&opts).unwrap();
}

fn opts_auth(opts: &cloner::admin::AdminOptions) -> ResolvedDb {
    ResolvedDb {
        engine: opts.engine,
        host: opts.host.clone(),
        port: opts.port,
        database: opts.auth_db.clone(),
        user: opts.user.clone(),
        password: opts.password.clone(),
        service: opts.service.clone(),
        in_container: true,
    }
}
