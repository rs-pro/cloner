use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::{Engine, ResolvedCompose, ResolvedDb, ResolvedDbConfig, ResolvedLocal};
use crate::ssh::Ssh;
use crate::util::{human_size, shq};

#[derive(Debug, Clone)]
pub struct LocalCmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

pub fn compose_prefix(compose: &ResolvedCompose) -> String {
    format!(
        "cd {} && docker compose -f {}",
        shq(&compose.path),
        shq(&compose.file)
    )
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

fn compose_prefix_local(compose: &ResolvedCompose) -> String {
    compose_prefix(compose)
}

pub fn dump_file_name(engine: Engine, custom: Option<&str>) -> String {
    if let Some(name) = custom {
        return name.to_string();
    }
    match engine {
        Engine::Postgres => "cloner.bak".to_string(),
        Engine::Mysql => "cloner.sql".to_string(),
        Engine::Mongo => "cloner.archive".to_string(),
    }
}

pub async fn dump_remote(
    ssh: &Ssh,
    remote: &crate::config::ResolvedRemote,
    cfg: &ResolvedDbConfig,
    db: &ResolvedDb,
    dry_run: bool,
) -> Result<String> {
    let name = dump_file_name(db.engine, cfg.dump_name.as_deref());
    let dump_path = cfg
        .remote_dump_path
        .clone()
        .unwrap_or_else(|| remote.dump_path.clone());
    let remote_file = format!("{}/{}", dump_path.trim_end_matches('/'), name);

    let compose = remote.compose.as_ref();
    let service = db.service.as_deref().or(cfg.remote_service.as_deref());

    let mut args: Vec<String> = cfg.dump_args.clone();
    if args.is_empty() {
        args = match db.engine {
            Engine::Postgres => vec!["-Fc".to_string()],
            Engine::Mysql => vec!["--add-drop-table".into(), "--single-transaction".into()],
            Engine::Mongo => vec!["--archive".into()],
        };
    }

    let inner = match db.engine {
        Engine::Postgres => {
            let mut parts = vec!["pg_dump".to_string()];
            parts.extend(args);
            parts.push("-U".into());
            parts.push(shq(&db.user));
            parts.push("-h".into());
            parts.push(shq(&db.host));
            parts.push("-p".into());
            parts.push(shq(&db.port.to_string()));
            parts.push(shq(&db.database));
            if db.password.is_empty() {
                parts.join(" ")
            } else {
                format!("env PGPASSWORD={} {}", shq(&db.password), parts.join(" "))
            }
        }
        Engine::Mysql => {
            let mut parts = args;
            parts.push("--user".into());
            parts.push(shq(&db.user));
            if !db.password.is_empty() {
                parts.push(format!("--password={}", shq(&db.password)));
            }
            parts.push("--host".into());
            parts.push(shq(&db.host));
            parts.push("--port".into());
            parts.push(shq(&db.port.to_string()));
            parts.push(shq(&db.database));
            mysql_shell("mysqldump", "mariadb-dump", &parts)
        }
        Engine::Mongo => {
            let uri = if db.user.is_empty() {
                format!("mongodb://{}:{}/{}", db.host, db.port, db.database)
            } else {
                format!(
                    "mongodb://{}:{}@{}:{}/{}?authSource=admin",
                    db.user, db.password, db.host, db.port, db.database
                )
            };
            format!("mongodump --uri {} --archive", shq(&uri))
        }
    };

    let body = if let Some(compose) = compose {
        let service = service.ok_or_else(|| {
            anyhow!("remote compose is configured but no database service was resolved")
        })?;
        format!(
            "{} exec -T {} {}",
            compose_prefix(compose),
            shq(service),
            inner
        )
    } else {
        inner
    };

    let command = format!(
        "mkdir -p {dump} && rm -f {file} && ({body}) > {file} && test -s {file} && wc -c < {file}",
        dump = shq(&dump_path),
        file = shq(&remote_file),
        body = body,
    );
    log::info!(
        "dumping {} database {} on {}",
        db.engine.as_str(),
        db.database,
        remote.host
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
    let compose = local.compose.as_ref();
    let service = cfg.local_service.as_deref().or(db.service.as_deref());

    let mut restore_args: Vec<String> = cfg.restore_args.clone();
    if restore_args.is_empty() {
        restore_args = match db.engine {
            Engine::Postgres => vec![
                "--no-owner".into(),
                "-Fc".into(),
                "-c".into(),
                "--if-exists".into(),
            ],
            Engine::Mysql => vec![],
            Engine::Mongo => vec!["--archive".into(), "--drop".into()],
        };
    }

    let inner = match db.engine {
        Engine::Postgres => {
            let mut parts = vec!["pg_restore".to_string()];
            parts.extend(restore_args);
            parts.push("-U".into());
            parts.push(shq(&db.user));
            parts.push("-h".into());
            parts.push(shq(&db.host));
            parts.push("-p".into());
            parts.push(shq(&db.port.to_string()));
            parts.push("-d".into());
            parts.push(shq(&db.database));
            if db.password.is_empty() {
                parts.join(" ")
            } else {
                format!("env PGPASSWORD={} {}", shq(&db.password), parts.join(" "))
            }
        }
        Engine::Mysql => {
            let mut parts = restore_args;
            parts.push("--user".into());
            parts.push(shq(&db.user));
            if !db.password.is_empty() {
                parts.push(format!("--password={}", shq(&db.password)));
            }
            parts.push("--host".into());
            parts.push(shq(&db.host));
            parts.push("--port".into());
            parts.push(shq(&db.port.to_string()));
            parts.push(shq(&db.database));
            mysql_shell("mysql", "mariadb", &parts)
        }
        Engine::Mongo => {
            let mut parts = vec!["mongorestore".to_string()];
            parts.extend(restore_args);
            parts.push("--nsInclude".into());
            parts.push(shq(&format!("{}.*", db.database)));
            parts.join(" ")
        }
    };

    let command = if let Some(compose) = compose {
        let service = service.ok_or_else(|| {
            anyhow!("local compose is configured but no database service was resolved")
        })?;
        format!(
            "{} exec -T {} {} < {}",
            compose_prefix_local(compose),
            shq(service),
            inner,
            shq(&dump_file.to_string_lossy())
        )
    } else {
        format!("{} < {}", inner, shq(&dump_file.to_string_lossy()))
    };

    log::info!(
        "restoring {} database {} locally",
        db.engine.as_str(),
        db.database
    );
    run_local(&command, None, &[], None, dry_run).map(|_| ())
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
    let compose = local.compose.as_ref();
    if let Some(compose) = compose {
        let service = db.service.as_deref().unwrap_or("db");
        log::info!("starting local compose service {service}");
        run_local(
            &format!("{} up -d {}", compose_prefix_local(compose), shq(service)),
            None,
            &[],
            None,
            dry_run,
        )?;
        wait_ready(local, db, dry_run)?;
    }
    match db.engine {
        Engine::Postgres => {
            let script = "dropdb --if-exists --force \"$PGDATABASE\" && createdb \"$PGDATABASE\"";
            let command = if let Some(compose) = compose {
                format!(
                    "{} exec -T {} env PGHOST={} PGPORT={} PGUSER={} PGDATABASE={} PGPASSWORD={} sh -eu -c {}",
                    compose_prefix_local(compose),
                    shq(db.service.as_deref().unwrap_or("db")),
                    shq(&db.host),
                    shq(&db.port.to_string()),
                    shq(&db.user),
                    shq(&db.database),
                    shq(&db.password),
                    shq(script)
                )
            } else {
                format!(
                    "PGHOST={} PGPORT={} PGUSER={} PGDATABASE={} PGPASSWORD={} sh -eu -c {}",
                    shq(&db.host),
                    shq(&db.port.to_string()),
                    shq(&db.user),
                    shq(&db.database),
                    shq(&db.password),
                    shq(script)
                )
            };
            run_local(&command, None, &[], None, dry_run).map(|_| ())
        }
        Engine::Mysql => {
            let ident = db.database.replace('`', "``");
            let script = format!("DROP DATABASE IF EXISTS `{ident}`; CREATE DATABASE `{ident}`;");
            let args = vec![
                format!("--user={}", shq(&db.user)),
                format!("--password={}", shq(&db.password)),
                format!("--host={}", shq(&db.host)),
                format!("--port={}", shq(&db.port.to_string())),
                format!("-e {}", shq(&script)),
            ];
            let client = mysql_shell("mysql", "mariadb", &args);
            let command = if let Some(compose) = compose {
                format!(
                    "{} exec -T {} {}",
                    compose_prefix_local(compose),
                    shq(db.service.as_deref().unwrap_or("db")),
                    client
                )
            } else {
                client
            };
            run_local(&command, None, &[], None, dry_run).map(|_| ())
        }
        Engine::Mongo => {
            let script = format!(
                "db.getSiblingDB({}).dropDatabase()",
                serde_json::to_string(&db.database)?
            );
            let command = if let Some(compose) = compose {
                format!(
                    "{} exec -T {} mongosh --quiet --eval {}",
                    compose_prefix_local(compose),
                    shq(db.service.as_deref().unwrap_or("db")),
                    shq(&script)
                )
            } else {
                format!("mongosh --quiet --eval {}", shq(&script))
            };
            run_local(&command, None, &[], None, dry_run).map(|_| ())
        }
    }
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

    run_local(
        &format!("{} down", compose_prefix_local(compose)),
        None,
        &[],
        None,
        dry_run,
    )?;
    run_local(
        &format!("docker volume rm -f {}", shq(volume)),
        None,
        &[],
        None,
        dry_run,
    )?;
    run_local(
        &format!("docker volume create {}", shq(volume)),
        None,
        &[],
        None,
        dry_run,
    )?;
    // Match the original cloner behavior: after recreating the volume, bring
    // the whole compose project up (not just the DB), because migrations and
    // the app may need search/redis/etc. too.
    run_local(
        &format!("{} up -d", compose_prefix_local(compose)),
        None,
        &[],
        None,
        dry_run,
    )?;
    wait_ready(local, db, dry_run)
}

pub fn wait_ready(local: &ResolvedLocal, db: &ResolvedDb, dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    let compose = local.compose.as_ref();
    let service = db.service.as_deref().unwrap_or("db");
    for attempt in 1..=60 {
        let check = match (db.engine, compose) {
            (Engine::Postgres, Some(compose)) => format!(
                "{} exec -T {} pg_isready -U {} -d {}",
                compose_prefix_local(compose),
                shq(service),
                shq(&db.user),
                shq(&db.database)
            ),
            (Engine::Postgres, None) => format!(
                "pg_isready -h {} -p {} -U {} -d {}",
                shq(&db.host),
                shq(&db.port.to_string()),
                shq(&db.user),
                shq(&db.database)
            ),
            (Engine::Mysql, Some(compose)) => format!(
                "{} exec -T {} {}",
                compose_prefix_local(compose),
                shq(service),
                mysql_shell(
                    "mysqladmin",
                    "mariadb-admin",
                    &[
                        "ping".to_string(),
                        format!("--user={}", shq(&db.user)),
                        format!("--password={}", shq(&db.password)),
                        format!("--host={}", shq(&db.host)),
                    ]
                )
            ),
            (Engine::Mysql, None) => mysql_shell(
                "mysqladmin",
                "mariadb-admin",
                &[
                    "ping".to_string(),
                    format!("--user={}", shq(&db.user)),
                    format!("--password={}", shq(&db.password)),
                    format!("--host={}", shq(&db.host)),
                    format!("--port={}", shq(&db.port.to_string())),
                ],
            ),
            (Engine::Mongo, Some(compose)) => format!(
                "{} exec -T {} mongosh --quiet --eval 'db.runCommand({{ping:1}}).ok'",
                compose_prefix_local(compose),
                shq(service)
            ),
            (Engine::Mongo, None) => format!(
                "mongosh --quiet --host {} --port {} --eval 'db.runCommand({{ping:1}}).ok'",
                shq(&db.host),
                shq(&db.port.to_string())
            ),
        };
        if run_local(&check, None, &[], None, false).is_ok() {
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
    let compose = local.compose.as_ref();
    let command = match db.engine {
        Engine::Postgres => {
            let inner = format!(
                "env PGPASSWORD={} psql -v ON_ERROR_STOP=1 -U {} -h {} -p {} -d {} -c {}",
                shq(&db.password),
                shq(&db.user),
                shq(&db.host),
                shq(&db.port.to_string()),
                shq(&db.database),
                shq(sql)
            );
            if let Some(compose) = compose {
                format!(
                    "{} exec -T {} {}",
                    compose_prefix_local(compose),
                    shq(db.service.as_deref().unwrap_or("db")),
                    inner
                )
            } else {
                inner
            }
        }
        Engine::Mysql => {
            let inner = mysql_shell(
                "mysql",
                "mariadb",
                &[
                    format!("--user={}", shq(&db.user)),
                    format!("--password={}", shq(&db.password)),
                    format!("--host={}", shq(&db.host)),
                    format!("--port={}", shq(&db.port.to_string())),
                    shq(&db.database),
                    format!("-e {}", shq(sql)),
                ],
            );
            if let Some(compose) = compose {
                format!(
                    "{} exec -T {} {}",
                    compose_prefix_local(compose),
                    shq(db.service.as_deref().unwrap_or("db")),
                    inner
                )
            } else {
                inner
            }
        }
        Engine::Mongo => {
            let script = format!(
                "db.getSiblingDB({}).runCommand({{ ping: 1 }})",
                serde_json::to_string(&db.database)?
            );
            let inner = format!("mongosh --quiet --eval {}", shq(&script));
            if let Some(compose) = compose {
                format!(
                    "{} exec -T {} {}",
                    compose_prefix_local(compose),
                    shq(db.service.as_deref().unwrap_or("db")),
                    inner
                )
            } else {
                inner
            }
        }
    };
    let output = run_local(&command, None, &[], None, dry_run)?;
    Ok(output.stdout)
}

pub fn run_local_capture(
    command: &str,
    cwd: Option<&Path>,
    envs: &[(String, String)],
    stdin_path: Option<&Path>,
) -> Result<LocalCmdOutput> {
    let shell = crate::util::local_shell()?;
    let mut process = Command::new(&shell.program);
    process.arg("-c").arg(command);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    for (key, value) in envs {
        process.env(key, value);
    }
    if let Some(stdin_path) = stdin_path {
        let file = std::fs::File::open(stdin_path)
            .with_context(|| format!("open stdin {}", stdin_path.display()))?;
        process.stdin(Stdio::from(file));
    }
    let output = process
        .output()
        .with_context(|| format!("run local command: {command}"))?;
    Ok(LocalCmdOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    })
}

pub fn run_local(
    command: &str,
    cwd: Option<&Path>,
    envs: &[(String, String)],
    stdin_path: Option<&Path>,
    dry_run: bool,
) -> Result<LocalCmdOutput> {
    log::debug!("local: {command}");
    if dry_run {
        println!("DRY RUN local: {command}");
        return Ok(LocalCmdOutput {
            stdout: String::new(),
            stderr: String::new(),
            code: 0,
        });
    }
    let result = run_local_capture(command, cwd, envs, stdin_path)?;
    if result.code != 0 {
        let detail = if !result.stderr.trim().is_empty() {
            result.stderr.trim()
        } else {
            result.stdout.trim()
        };
        bail!(
            "local command failed with exit code {}: {command}\n{}",
            result.code,
            tail(detail, 4000)
        );
    }
    Ok(result)
}

fn tail(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn builds_compose_prefix_with_quoting() {
        let compose = ResolvedCompose {
            path: "/srv/my app".into(),
            file: "compose.yml".into(),
            service: None,
        };
        assert_eq!(
            compose_prefix(&compose),
            "cd '/srv/my app' && docker compose -f compose.yml"
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

    #[test]
    fn tail_keeps_utf8_boundaries() {
        assert_eq!(tail("short", 100), "short");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("русский", 8), "ский");
    }
}
