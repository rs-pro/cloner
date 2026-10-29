mod admin;
mod config;
mod db;
mod dsl;
mod ssh;
mod sync;
mod util;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use dsl::{RunMode, RunOptions};
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
        compose: context
            .as_ref()
            .and_then(|c| c.resolved.local.compose.clone()),
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
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(config.local.root.as_deref(), Some(root.to_str().unwrap()));
        assert_eq!(config.remote.unwrap().host, "example.com");
        assert_eq!(config.files.len(), 0);
        assert!(TEMPLATE.contains("execute()"));
        assert!(TEMPLATE.contains("local:"));
        assert!(TEMPLATE.contains("remoteDumpPath"));
    }
}
