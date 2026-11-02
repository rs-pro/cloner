//! End-to-end CLI tests. These only exercise commands that do not need a
//! network connection or a database server.

use std::path::Path;
use std::process::{Command, Output};

fn cloner(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cloner"))
        .args(args)
        .current_dir(dir)
        .env_remove("RAILS_ENV")
        .env_remove("PGHOST")
        .env_remove("PGPORT")
        .env_remove("PGUSER")
        .env_remove("PGPASSWORD")
        .output()
        .expect("run cloner binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    output
}

#[test]
fn prints_version() {
    let dir = tempfile::tempdir().unwrap();
    let output = success(cloner(dir.path(), &["--version"]));
    assert!(stdout(&output).contains("cloner"));
}

#[test]
fn help_lists_all_commands() {
    let dir = tempfile::tempdir().unwrap();
    let output = success(cloner(dir.path(), &["--help"]));
    let text = stdout(&output);
    for command in ["run", "backup", "validate", "init", "db"] {
        assert!(text.contains(command), "missing {command} in help:\n{text}");
    }
}

#[test]
fn rejects_unknown_commands() {
    let dir = tempfile::tempdir().unwrap();
    let output = cloner(dir.path(), &["definitely-not-a-command"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unrecognized subcommand"));
}

#[test]
fn reports_missing_config() {
    let dir = tempfile::tempdir().unwrap();
    let output = cloner(dir.path(), &["validate"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("no DSL file found"));

    let output = cloner(dir.path(), &["--config", "missing.js", "validate"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("does not exist"));
}

#[test]
fn init_writes_a_starter_config_and_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let output = success(cloner(dir.path(), &["init"]));
    assert!(stdout(&output).contains("wrote dl.js"));
    let config = std::fs::read_to_string(dir.path().join("dl.js")).unwrap();
    assert!(config.contains("module.exports"));
    assert!(config.contains("remote:"));
    assert!(config.contains("execute()"));

    let output = cloner(dir.path(), &["init"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("already exists"));

    let output = success(cloner(dir.path(), &["init", "--force"]));
    assert!(stdout(&output).contains("wrote dl.js"));
}

#[test]
fn db_create_dry_run_prints_queries() {
    let dir = tempfile::tempdir().unwrap();
    let output = success(cloner(dir.path(), &["db", "create", "app", "--dry-run"]));
    let text = stdout(&output);
    assert!(text.contains("DRY RUN local:"));
    assert!(text.contains("CREATE DATABASE \"app_development\""));
    assert!(text.contains("CREATE ROLE \"app\""));
    assert!(text.contains("psql"));
}

#[test]
fn db_reset_and_drop_dry_run_are_engine_aware() {
    let dir = tempfile::tempdir().unwrap();

    let output = success(cloner(
        dir.path(),
        &["db", "reset", "app", "--engine", "mysql", "--dry-run"],
    ));
    let text = stdout(&output);
    assert!(text.contains("DROP DATABASE IF EXISTS `app_development`"));
    assert!(text.contains("mariadb"));

    let output = success(cloner(
        dir.path(),
        &[
            "db",
            "drop",
            "--dbname",
            "app_test",
            "--engine",
            "mongo",
            "--dry-run",
        ],
    ));
    let text = stdout(&output);
    assert!(text.contains("mongosh"));
    assert!(text.contains("dropDatabase"));
}

#[test]
fn db_commands_refuse_production_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let output = cloner(
        dir.path(),
        &["db", "reset", "app", "--env", "production", "--dry-run"],
    );
    assert!(!output.status.success());
    assert!(stderr(&output).contains("refusing to modify a production database"));

    let output = success(cloner(
        dir.path(),
        &[
            "db",
            "reset",
            "app",
            "--env",
            "production",
            "--force",
            "--dry-run",
        ],
    ));
    assert!(stdout(&output).contains("DRY RUN local:"));
}

#[test]
fn db_reads_the_local_database_from_a_dsl_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("dl.js"),
        r#"
        module.exports = {
          local: { root: __dirname, env: "development" },
          database: {
            engine: "postgres",
            local: { database: "from_config", user: "config_user", password: "secret" },
          },
        };
        "#,
    )
    .unwrap();

    let output = success(cloner(
        dir.path(),
        &[
            "db",
            "drop",
            "--dbname",
            "from_config",
            "--dry-run",
            "-c",
            "dl.js",
        ],
    ));
    let text = stdout(&output);
    assert!(text.contains("from_config"));
    assert!(text.contains("DROP DATABASE IF EXISTS"));
}

#[test]
fn db_migrate_and_seed_dry_run_use_rake() {
    let dir = tempfile::tempdir().unwrap();
    for (command, task) in [("migrate", "db:migrate"), ("seed", "db:seed")] {
        let output = success(cloner(
            dir.path(),
            &["db", command, "--dbname", "app_development", "--dry-run"],
        ));
        assert!(stdout(&output).contains(&format!("bundle exec rake {task}")));
    }
}
