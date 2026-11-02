use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use cloner::dsl::{self, RunMode, RunOptions};
use cloner::{admin, config};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "cloner",
    version,
    about = "Clone production databases and files using a JavaScript DSL"
)]
struct Cli {
    /// Path to the DSL file (default: dl.js)
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,

    /// Enable verbose logging
    #[arg(long, short = 'v', global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Clone database and files (default)
    Run(RunArgs),
    /// Alias for run
    Download(RunArgs),
    /// Dump database and sync files into a backup directory
    Backup(BackupArgs),
    /// Parse the DSL, resolve compose/env and print the effective config
    Validate(RunArgs),
    /// Write a starter dl.js
    Init(InitArgs),
    /// pg_dev-style database create/reset/drop/migrate/seed
    Db(Box<DbArgs>),
    /// Restore a stored dump (backup directory or dump file)
    Restore(RestoreArgs),
    /// Verify a stored backup directory and its manifest
    Verify(VerifyArgs),
    /// Kubernetes support (CronJob manifest generation)
    K8s(Box<K8sArgs>),
}

#[derive(Args, Debug)]
struct RestoreArgs {
    /// Backup directory (with manifest.json) or a dump file
    #[arg(long, short = 'f')]
    from: PathBuf,
    /// Do not reset the local database before restoring
    #[arg(long = "no-reset")]
    no_reset: bool,
    #[arg(long)]
    dry_run: bool,
    /// Allow restoring with env=production
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug)]
struct VerifyArgs {
    /// Backup directory (with manifest.json) or a dump file
    #[arg(long, short = 'd')]
    dir: PathBuf,
}

#[derive(Args, Debug)]
struct K8sArgs {
    #[command(subcommand)]
    action: K8sAction,
}

#[derive(Subcommand, Debug)]
enum K8sAction {
    /// Render a nightly pg_dump + object-storage upload CronJob
    Cronjob(CronJobArgs),
}

#[derive(Args, Debug)]
struct CronJobArgs {
    /// CronJob name
    #[arg(long)]
    name: Option<String>,
    /// Cron schedule (default: 30 3 * * *)
    #[arg(long)]
    schedule: Option<String>,
    /// IANA time zone (default: Europe/Minsk)
    #[arg(long = "time-zone")]
    time_zone: Option<String>,
    /// rclone destination, e.g. yc:blog-db-backups/postgres
    #[arg(long)]
    bucket: Option<String>,
    /// Secret holding PGPASSWORD, S3_ACCESS_KEY and S3_SECRET_KEY
    #[arg(long)]
    secret: Option<String>,
    /// rclone remote name (default: yc)
    #[arg(long = "remote-name")]
    remote_name: Option<String>,
    /// S3 endpoint
    #[arg(long = "s3-endpoint")]
    s3_endpoint: Option<String>,
    /// S3 region
    #[arg(long = "s3-region")]
    s3_region: Option<String>,
    /// Dump image (default: postgres:18-alpine)
    #[arg(long = "dump-image")]
    dump_image: Option<String>,
    /// Upload image (default: rclone/rclone:1.75.1)
    #[arg(long = "upload-image")]
    upload_image: Option<String>,
    /// emptyDir size limit (default: 512Mi)
    #[arg(long = "size-limit")]
    size_limit: Option<String>,
    /// Database engine override: postgres, mysql or mongo
    #[arg(long)]
    engine: Option<String>,
    /// Database name override
    #[arg(long)]
    database: Option<String>,
    /// Database user override
    #[arg(long)]
    user: Option<String>,
    /// Database host override
    #[arg(long)]
    host: Option<String>,
    /// Database port override
    #[arg(long)]
    port: Option<u16>,
    /// Write the manifest to a file instead of stdout
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Overwrite an existing output file
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug, Clone)]
struct RunArgs {
    /// Skip the database step
    #[arg(long = "skip-database", short = 'D')]
    skip_database: bool,
    /// Skip the files step
    #[arg(long = "skip-files", short = 'F')]
    skip_files: bool,
    /// Only dump/plan, do not modify local services or files
    #[arg(long)]
    dry_run: bool,
    /// Environment name exposed as CLONE_FROM to the DSL
    #[arg(long)]
    from: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct BackupArgs {
    /// Backup destination directory (default: backups/<timestamp>)
    #[arg(long, short = 't')]
    to: Option<PathBuf>,
    /// Skip the database step
    #[arg(long = "skip-database", short = 'D')]
    skip_database: bool,
    /// Skip the files step
    #[arg(long = "skip-files", short = 'F')]
    skip_files: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    from: Option<String>,
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Overwrite an existing file
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug)]
struct DbArgs {
    #[command(subcommand)]
    action: DbAction,
}

#[derive(Subcommand, Debug)]
enum DbAction {
    /// Create the role/user and database
    #[command(alias = "c")]
    Create(DbOptions),
    /// Drop and recreate the schema/database (destroys local data)
    #[command(alias = "r")]
    Reset(DbOptions),
    /// Drop the database
    #[command(alias = "d")]
    Drop(DbOptions),
    /// Run bundle exec rake db:migrate
    Migrate(DbOptions),
    /// Run bundle exec rake db:seed
    Seed(DbOptions),
}

#[derive(Args, Debug, Clone)]
struct DbOptions {
    /// User/owner name (positional, like pg_dev)
    name: Option<String>,
    /// Database engine: postgres, mysql or mongo
    #[arg(long)]
    engine: Option<String>,
    /// Database host (default: localhost or PGHOST)
    #[arg(long)]
    host: Option<String>,
    /// Database port (default: engine port or PGPORT)
    #[arg(long)]
    port: Option<u16>,
    /// Admin connection user (default: postgres or PGUSER)
    #[arg(long)]
    user: Option<String>,
    /// Admin connection password (default: PGPASSWORD)
    #[arg(long)]
    password: Option<String>,
    /// Authentication/maintenance database
    #[arg(long = "auth-db")]
    auth_db: Option<String>,
    /// Environment suffix, e.g. development/test (default: RAILS_ENV)
    #[arg(long)]
    env: Option<String>,
    /// Database name (default: <owner>_<env> or the DSL's local database)
    #[arg(long)]
    dbname: Option<String>,
    /// Database owner
    #[arg(long)]
    owner: Option<String>,
    /// Schema to reset (PostgreSQL only)
    #[arg(long, default_value = "public")]
    schema: String,
    /// Do not drop the schema/database before recreating
    #[arg(long = "no-drop")]
    no_drop: bool,
    /// Do not recreate the schema/database
    #[arg(long = "no-create")]
    no_create: bool,
    /// Set the user password (defaults to the user name)
    #[arg(long = "set-password")]
    set_password: Option<String>,
    /// Create the hstore extension (PostgreSQL)
    #[arg(long)]
    hstore: bool,
    /// Create the postgis extension (PostgreSQL)
    #[arg(long)]
    postgis: bool,
    /// Run bundle exec rake db:migrate afterwards
    #[arg(long)]
    migrate: bool,
    /// Run bundle exec rake db:seed afterwards
    #[arg(long)]
    seed: bool,
    /// Allow running with env=production
    #[arg(long)]
    force: bool,
    /// Print commands without executing them
    #[arg(long)]
    dry_run: bool,
}

fn main() {
    if let Err(error) = real_main() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let default_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
        .format_timestamp(None)
        .init();

    match cli.command.unwrap_or(Command::Run(RunArgs {
        skip_database: false,
        skip_files: false,
        dry_run: false,
        from: None,
    })) {
        Command::Run(args) | Command::Download(args) => {
            let path = find_config(cli.config.as_deref())?;
            log::info!("using {}", path.display());
            dsl::execute(&path, run_options(&args, RunMode::Clone))?;
        }
        Command::Backup(args) => {
            let path = find_config(cli.config.as_deref())?;
            log::info!("using {}", path.display());
            let mut options = run_options(
                &RunArgs {
                    skip_database: args.skip_database,
                    skip_files: args.skip_files,
                    dry_run: args.dry_run,
                    from: args.from,
                },
                RunMode::Backup,
            );
            options.backup_dir = args.to;
            dsl::execute(&path, options)?;
        }
        Command::Validate(args) => {
            let path = find_config(cli.config.as_deref())?;
            dsl::execute(&path, run_options(&args, RunMode::Validate))?;
        }
        Command::Db(args) => {
            let config = find_config_optional(cli.config.as_deref())?;
            let options = admin_options(&args.action, &config)?;
            match args.action {
                DbAction::Create(_) => admin::create(&options)?,
                DbAction::Reset(_) => admin::reset(&options)?,
                DbAction::Drop(_) => admin::drop_database(&options)?,
                DbAction::Migrate(_) => admin::run_migrations(&options)?,
                DbAction::Seed(_) => admin::run_seeds(&options)?,
            }
        }
        Command::Restore(args) => {
            let path = find_config(cli.config.as_deref())?;
            let context = dsl::load_local_context(&path)?;
            let project_env = context.resolved.project_env.clone();
            if project_env == "production" && !args.force {
                bail!("refusing to restore into a production database (use --force to override)");
            }
            let cfg = context.resolved.database.clone().ok_or_else(|| {
                anyhow::anyhow!("{} does not configure a database", path.display())
            })?;
            let stored = cloner::restore::restore_local_backup(
                &context.resolved.local,
                &cfg,
                &context.db,
                &args.from,
                &context.resolved.project_root,
                cloner::restore::RestoreOptions {
                    reset: !args.no_reset,
                    dry_run: args.dry_run,
                },
            )?;
            log::info!("restored {}", stored.dump.display());
        }
        Command::Verify(args) => {
            let stored = cloner::restore::StoredBackup::resolve(&args.dir)?;
            let report = stored.verify()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ok": report.ok(),
                    "dump": stored.dump.display().to_string(),
                    "manifestOk": report.manifest_ok,
                    "dumpPresent": report.dump_present,
                    "sizeOk": report.size_ok,
                    "sha256Ok": report.sha256_ok,
                }))?
            );
            if !report.ok() {
                bail!("backup {} failed verification", args.dir.display());
            }
        }
        Command::K8s(args) => match args.action {
            K8sAction::Cronjob(args) => k8s_cronjob(&args, cli.config.as_deref())?,
        },
        Command::Init(args) => {
            let path = cli.config.clone().unwrap_or_else(|| PathBuf::from("dl.js"));
            if path.exists() && !args.force {
                bail!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            std::fs::write(&path, TEMPLATE).with_context(|| format!("write {}", path.display()))?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}

fn k8s_cronjob(args: &CronJobArgs, explicit_config: Option<&Path>) -> Result<()> {
    use cloner::engine::{Engine, ResolvedDb};
    use cloner::k8s::BackupCronJob;

    let config = find_config_optional(explicit_config)?;
    let (engine, db) = match config {
        Some(path) => {
            let context = dsl::load_local_context(&path)?;
            (context.db.engine, context.db)
        }
        None => (
            Engine::Postgres,
            ResolvedDb {
                engine: Engine::Postgres,
                host: "localhost".into(),
                port: 5432,
                database: String::new(),
                user: String::new(),
                password: String::new(),
                service: None,
                in_container: false,
            },
        ),
    };
    let database = ResolvedDb {
        engine: args
            .engine
            .as_deref()
            .map(parse_engine)
            .transpose()?
            .unwrap_or(engine),
        host: args.host.clone().unwrap_or(db.host),
        port: args.port.unwrap_or(db.port),
        database: args.database.clone().unwrap_or(db.database),
        user: args.user.clone().unwrap_or(db.user),
        password: String::new(),
        service: None,
        in_container: false,
    };
    let bucket = args.bucket.clone().ok_or_else(|| {
        anyhow::anyhow!("--bucket is required (for example yc:blog-db-backups/postgres)")
    })?;
    let name = args.name.clone().unwrap_or_else(|| {
        let base = database.database.trim();
        if base.is_empty() {
            "db-backup".to_string()
        } else {
            format!("{base}-backup")
        }
    });

    let mut job = BackupCronJob::for_database(name, database, bucket);
    if let Some(schedule) = &args.schedule {
        job.schedule = schedule.clone();
    }
    if let Some(zone) = &args.time_zone {
        job.time_zone = zone.clone();
    }
    if let Some(secret) = &args.secret {
        job.secret_name = secret.clone();
    }
    if let Some(remote_name) = &args.remote_name {
        job.remote_name = remote_name.clone();
    }
    if let Some(endpoint) = &args.s3_endpoint {
        job.s3_endpoint = endpoint.clone();
    }
    if let Some(region) = &args.s3_region {
        job.s3_region = region.clone();
    }
    if let Some(image) = &args.dump_image {
        job.dump_image = image.clone();
    }
    if let Some(image) = &args.upload_image {
        job.upload_image = image.clone();
    }
    if let Some(limit) = &args.size_limit {
        job.size_limit = limit.clone();
    }

    let yaml = job.render()?;
    match &args.out {
        Some(path) => {
            if path.exists() && !args.force {
                bail!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            std::fs::write(path, yaml).with_context(|| format!("write {}", path.display()))?;
            println!("wrote {}", path.display());
        }
        None => print!("{yaml}"),
    }
    Ok(())
}

fn run_options(args: &RunArgs, mode: RunMode) -> RunOptions {
    RunOptions {
        skip_database: args.skip_database,
        skip_files: args.skip_files,
        dry_run: args.dry_run || mode == RunMode::Validate,
        mode,
        backup_dir: None,
        from: args.from.clone(),
    }
}

fn find_config(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if !path.exists() {
            bail!("config file {} does not exist", path.display());
        }
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().context("get current directory")?;
    let names = [
        "dl.js",
        "cloner.js",
        "cloner.config.js",
        "dl.config.js",
        ".cloner.js",
    ];
    for dir in cwd.ancestors() {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "no DSL file found in {} or its parents (expected one of: {})",
        cwd.display(),
        names.join(", ")
    )
}

fn find_config_optional(explicit: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit {
        if !path.exists() {
            bail!("config file {} does not exist", path.display());
        }
        return Ok(Some(path.to_path_buf()));
    }
    let cwd = std::env::current_dir().context("get current directory")?;
    let names = [
        "dl.js",
        "cloner.js",
        "cloner.config.js",
        "dl.config.js",
        ".cloner.js",
    ];
    for dir in cwd.ancestors() {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

fn admin_options(action: &DbAction, config: &Option<PathBuf>) -> Result<admin::AdminOptions> {
    let opts = match action {
        DbAction::Create(o)
        | DbAction::Reset(o)
        | DbAction::Drop(o)
        | DbAction::Migrate(o)
        | DbAction::Seed(o) => o,
    };
    let context = match config {
        Some(path) => Some(dsl::load_local_context(path)?),
        None => None,
    };

    let engine = match opts.engine.as_deref() {
        Some(value) => parse_engine(value)?,
        None => context
            .as_ref()
            .map(|c| c.db.engine)
            .unwrap_or(config::Engine::Postgres),
    };
    let project_env = context
        .as_ref()
        .map(|c| c.resolved.project_env.clone())
        .or_else(|| opts.env.clone())
        .or_else(|| std::env::var("RAILS_ENV").ok())
        .unwrap_or_else(|| "development".to_string());
    if project_env == "production" && !opts.force {
        bail!("refusing to modify a production database (use --force to override)");
    }

    let owner = opts
        .name
        .clone()
        .or_else(|| opts.owner.clone())
        .or_else(|| context.as_ref().map(|c| c.db.user.clone()))
        .or_else(|| opts.user.clone())
        .unwrap_or_default();
    let dbname = opts
        .dbname
        .clone()
        .or_else(|| context.as_ref().map(|c| c.db.database.clone()))
        .unwrap_or_else(|| {
            if owner.is_empty() {
                String::new()
            } else {
                format!("{owner}_{project_env}")
            }
        });
    if dbname.is_empty() {
        bail!("database name is required (pass --dbname or an owner name)");
    }
    let user = opts
        .user
        .clone()
        .or_else(|| context.as_ref().map(|c| c.db.user.clone()))
        .unwrap_or_else(|| match engine {
            config::Engine::Postgres => "postgres".to_string(),
            config::Engine::Mysql => "root".to_string(),
            config::Engine::Mongo => String::new(),
        });
    let password = opts
        .password
        .clone()
        .or_else(|| context.as_ref().map(|c| c.db.password.clone()))
        .or_else(|| std::env::var("PGPASSWORD").ok())
        .unwrap_or_default();
    let host = opts
        .host
        .clone()
        .or_else(|| context.as_ref().map(|c| c.db.host.clone()))
        .or_else(|| std::env::var("PGHOST").ok())
        .unwrap_or_else(|| "localhost".to_string());
    let port = opts
        .port
        .or_else(|| context.as_ref().map(|c| c.db.port))
        .or_else(|| std::env::var("PGPORT").ok().and_then(|v| v.parse().ok()))
        .unwrap_or_else(|| engine.default_port());
    let auth_db = opts.auth_db.clone().unwrap_or_else(|| match engine {
        config::Engine::Postgres => "postgres".to_string(),
        config::Engine::Mysql => String::new(),
        config::Engine::Mongo => "admin".to_string(),
    });
    let project_root = context
        .as_ref()
        .map(|c| c.resolved.project_root.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut migrate = opts.migrate;
    let mut seed = opts.seed;
    if matches!(action, DbAction::Migrate(_)) {
        migrate = true;
        seed = false;
    }
    if matches!(action, DbAction::Seed(_)) {
        seed = true;
        migrate = false;
    }

    Ok(admin::AdminOptions {
        engine,
        host,
        port,
        user,
        password,
        auth_db,
        dbname,
        owner,
        schema: opts.schema.clone(),
        no_drop: opts.no_drop,
        no_create: opts.no_create,
        set_password: opts.set_password.clone(),
        target: context
            .as_ref()
            .map(|c| c.resolved.local.command_target())
            .unwrap_or_default(),
        service: context.as_ref().and_then(|c| c.db.service.clone()),
        project_root,
        hstore: opts.hstore,
        postgis: opts.postgis,
        migrate,
        seed,
        dry_run: opts.dry_run,
    })
}

fn parse_engine(value: &str) -> Result<config::Engine> {
    match value.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" | "pg" => Ok(config::Engine::Postgres),
        "mysql" | "mariadb" => Ok(config::Engine::Mysql),
        "mongo" | "mongodb" => Ok(config::Engine::Mongo),
        other => bail!("unknown engine {other:?}; expected postgres, mysql or mongo"),
    }
}

const TEMPLATE: &str = r#"// cloner DSL — https://github.com/rs-pro/cloner
// Run with: cloner run
// Validate with: cloner validate

module.exports = {
  local: {
    root: __dirname,
    env: "development",
    compose: {
      path: "dev-docker",
      file: "compose.yml",
      service: "db",
    },
    // Kubernetes alternative (wins over compose when set):
    // kubernetes: {
    //   namespace: "app",
    //   selector: "app.kubernetes.io/name=postgres",
    //   container: "postgres",
    // },
  },

  remote: {
    host: "example.com",
    user: "root",
    port: 22,
    appPath: "/srv/app",
    compose: {
      file: "compose.yml",
      // service: "db",                    // auto-detected from the image/env
    },
    // envFile: "/srv/app/.env",
    // identityFile: "~/.ssh/id_ed25519",
    // passwordEnv: "PROD_SSH_PASSWORD",
    strictHostKeyChecking: true,
  },

  database: {
    engine: "postgres",                  // postgres | mysql | mongo
    // remoteDumpPath: "/srv/app/tmp_dump",
    // reset: "drop-create",             // none | drop-create | volume
    // volume: "myapp_postgres",
    restoreArgs: ["--no-owner", "-Fc", "-c", "--if-exists"],
    // dumpArgs: ["-Fc"],
  },

  files: [
    // { from: "public/uploads", to: "public/uploads" },
    // { from: "storage", to: "storage", delete: true, exclude: ["**/*.tmp"] },
  ],

  hooks: {
    // beforeDatabase() {},
    // beforeRestore() {},
    // afterRestore() {
    //   shell("bin/rails db:migrate");
    //   db.query("UPDATE settings SET host = 'http://localhost:3000'");
    // },
    // afterAll() {},
  },

  // Optional: replace the default clone action entirely.
  // execute() {
  //   shell("echo custom maintenance instead of cloning");
  //   // clone_db();
  //   // clone_files();
  // },
};
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    fn db_args(cli: &Cli) -> (&DbOptions, &DbAction) {
        match cli.command.as_ref().unwrap() {
            Command::Db(args) => match &args.action {
                action @ DbAction::Create(opts)
                | action @ DbAction::Reset(opts)
                | action @ DbAction::Drop(opts)
                | action @ DbAction::Migrate(opts)
                | action @ DbAction::Seed(opts) => (opts, action),
            },
            other => panic!("expected db command, got {other:?}"),
        }
    }

    #[test]
    fn parses_default_run_command() {
        let cli = parse(&["cloner"]);
        assert!(cli.command.is_none());
        assert!(cli.config.is_none());
        assert!(!cli.verbose);
    }

    #[test]
    fn parses_run_flags() {
        let cli = parse(&[
            "cloner",
            "-c",
            "custom.js",
            "run",
            "-D",
            "--skip-files",
            "--dry-run",
            "--from",
            "staging",
            "-v",
        ]);
        assert_eq!(cli.config.as_deref(), Some(Path::new("custom.js")));
        assert!(cli.verbose);
        match cli.command.unwrap() {
            Command::Run(args) => {
                assert!(args.skip_database);
                assert!(args.skip_files);
                assert!(args.dry_run);
                assert_eq!(args.from.as_deref(), Some("staging"));
            }
            other => panic!("expected run, got {other:?}"),
        }
    }

    #[test]
    fn parses_backup_flags() {
        let cli = parse(&["cloner", "backup", "-t", "/backups/app", "-F"]);
        match cli.command.unwrap() {
            Command::Backup(args) => {
                assert_eq!(args.to.as_deref(), Some(Path::new("/backups/app")));
                assert!(args.skip_files);
                assert!(!args.skip_database);
            }
            other => panic!("expected backup, got {other:?}"),
        }
    }

    #[test]
    fn parses_database_commands_and_aliases() {
        for alias in ["create", "c"] {
            let cli = parse(&[
                "cloner",
                "db",
                alias,
                "app",
                "--engine",
                "mysql",
                "--port",
                "3307",
                "--dbname",
                "app_test",
                "--schema",
                "tenant",
                "--no-drop",
                "--hstore",
            ]);
            let (opts, action) = db_args(&cli);
            assert!(matches!(action, DbAction::Create(_)));
            assert_eq!(opts.name.as_deref(), Some("app"));
            assert_eq!(opts.engine.as_deref(), Some("mysql"));
            assert_eq!(opts.port, Some(3307));
            assert_eq!(opts.dbname.as_deref(), Some("app_test"));
            assert_eq!(opts.schema, "tenant");
            assert!(opts.no_drop);
            assert!(opts.hstore);
        }

        for (name, expected) in [
            ("r", "reset"),
            ("d", "drop"),
            ("migrate", "migrate"),
            ("seed", "seed"),
        ] {
            let cli = parse(&["cloner", "db", name]);
            let (_, action) = db_args(&cli);
            let actual = match action {
                DbAction::Create(_) => "create",
                DbAction::Reset(_) => "reset",
                DbAction::Drop(_) => "drop",
                DbAction::Migrate(_) => "migrate",
                DbAction::Seed(_) => "seed",
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn parses_engines() {
        assert_eq!(parse_engine("postgres").unwrap(), config::Engine::Postgres);
        assert_eq!(parse_engine("POSTGRES").unwrap(), config::Engine::Postgres);
        assert_eq!(parse_engine("pg").unwrap(), config::Engine::Postgres);
        assert_eq!(parse_engine("mysql").unwrap(), config::Engine::Mysql);
        assert_eq!(parse_engine("mariadb").unwrap(), config::Engine::Mysql);
        assert_eq!(parse_engine("mongo").unwrap(), config::Engine::Mongo);
        assert_eq!(parse_engine("mongodb").unwrap(), config::Engine::Mongo);
        assert!(parse_engine("sqlite").is_err());
    }

    #[test]
    fn validate_mode_forces_dry_run() {
        let args = RunArgs {
            skip_database: true,
            skip_files: false,
            dry_run: false,
            from: Some("test".into()),
        };
        let options = run_options(&args, RunMode::Validate);
        assert!(options.dry_run);
        assert!(options.skip_database);
        assert_eq!(options.from.as_deref(), Some("test"));

        let options = run_options(&args, RunMode::Clone);
        assert!(!options.dry_run);
    }

    #[test]
    fn explicit_config_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.js");
        let error = find_config(Some(&missing)).unwrap_err();
        assert!(error.to_string().contains("does not exist"));

        let existing = dir.path().join("dl.js");
        std::fs::write(&existing, "module.exports = {}").unwrap();
        assert_eq!(find_config(Some(&existing)).unwrap(), existing);
        assert_eq!(
            find_config_optional(Some(&existing)).unwrap().as_deref(),
            Some(existing.as_path())
        );
    }

    #[test]
    fn starter_template_is_valid_javascript_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dl.js");
        std::fs::write(&path, TEMPLATE).unwrap();
        let config = dsl::eval_config(&path).unwrap();
        let root = cloner::util::clean_path(dir.path().canonicalize().unwrap());
        assert_eq!(config.local.root.as_deref(), Some(root.to_str().unwrap()));
        assert_eq!(config.remote.unwrap().host, "example.com");
        assert_eq!(config.files.len(), 0);
        assert!(TEMPLATE.contains("execute()"));
        assert!(TEMPLATE.contains("local:"));
        assert!(TEMPLATE.contains("remoteDumpPath"));
    }
}
