# Cloner

Clone production databases and files into local development or staging
environments.

This repository contains two implementations:

- **Rust (main):** a single binary with a JavaScript DSL (`dl.js`), SSH/SFTP
  sync, Compose/`.env` resolution, backup mode and `pg_dev`-style database
  commands. No Ruby, `ssh`, `rsync`, `scp` or JavaScript runtime is required.
- **Ruby/Thor (legacy, still supported):** the original gem with a `dl.thor`
  config and `bundle exec thor dl`. See the
  [Ruby README](docs/ruby-readme.md). It remains supported for existing
  installations and projects that prefer Ruby, and its configuration syntax maps
  closely to the Rust DSL.

## Quick start

```bash
# install to ~/.local/bin (installs build deps + Rust if needed)
./install.sh

# create a starter config
cloner init

# inspect the resolved config (passwords redacted)
cloner validate

# clone database + files
cloner run

# non-destructive backup
cloner backup --to /var/backups/myapp
```

```powershell
# Windows: installs the C++ Build Tools + Rust if needed, then copies
# cloner.exe to %USERPROFILE%\.local\bin
.\install.ps1 -AddToPath
```

Or build manually from the repository root:

```bash
cargo build --release --locked
./target/release/cloner --help
```

On Windows the binary is `target\release\cloner.exe`.

Example `dl.js`:

```js
module.exports = {
  local: {
    root: __dirname,
    env: "development",
    compose: { path: "dev-docker", file: "compose.yml", service: "db" },
  },

  remote: {
    host: "example.com",
    user: "root",
    port: 22,
    appPath: "/srv/app",
    compose: { file: "compose.yml", service: "postgres" },
  },

  database: {
    engine: "postgres",
    reset: "drop-create",
  },

  files: [
    { from: "public/uploads", to: "public/uploads" },
  ],

  hooks: {
    afterRestore() {
      shell("bin/rails db:migrate");
    },
  },
};
```

`dl.js` is deliberately close to `dl.thor`: `ssh_host` becomes `remote.host`,
`remote_app_path` becomes `remote.appPath`, `remote_docker_compose_*` becomes
`remote.compose`, and `rsync_public(...)` becomes a `files` entry.

## Commands

```text
cloner run [--skip-database] [--skip-files] [--dry-run] [--from ENV]
cloner backup --to DIR
cloner validate
cloner init
cloner restore --from /var/backups/myapp
cloner verify --dir /var/backups/myapp
cloner db create|reset|drop|migrate|seed ...
cloner k8s cronjob --bucket yc:my-bucket/postgres
```

See the full [CLI reference](docs/guides/cli.md).

## Features

- JavaScript DSL (`dl.js`) similar in spirit to `dl.thor`
- SSH via [`russh`](https://github.com/warp-tech/russh) (Apache-2.0)
- Incremental file sync over SFTP (`russh-sftp`, MIT/Apache-2.0)
- Embedded QuickJS via `rquickjs` (MIT) — no Node.js dependency
- PostgreSQL, MySQL and MongoDB
- Runs against Docker Compose services or Kubernetes pods (`kubectl exec`)
- Reads Docker Compose, `.env` files and pod environments; no need to repeat DB
  credentials
- `pg_dev`-style `cloner db create/reset/drop/migrate/seed`
- Backup mode with a JSON manifest (dump size + SHA-256) and retention
- Kubernetes CronJob generator for nightly dumps straight to object storage

> The local `/data/_swap/rsync` crate is GPL-3.0-or-later. It is deliberately not
> linked into this MIT-licensed binary; file sync is implemented on top of
> `russh-sftp`.

## Ruby/Thor (legacy)

The original Ruby gem lives in the same repository (`lib/`, `test/`,
`cloner.gemspec`, `Gemfile`). It is still supported and documented in the
[Ruby README](docs/ruby-readme.md), including Docker Compose support, `thor dl`
options and the changelog.

For migration guidance, see
[Converting `dl.thor` to `dl.js`](docs/guides/thor-to-js-conversion.md).

## Testing

```bash
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
```

The suite contains unit tests for config/Compose resolution, dotenv and
interpolation handling, shell quoting, database command construction, sync
filters, the JavaScript DSL evaluator, backup manifests/retention, Kubernetes
command building and CronJob rendering, plus CLI integration tests for `init`,
config discovery, `db` actions, `k8s cronjob` and dry runs.

Real database round trips (PostgreSQL, MariaDB, MongoDB) run against Compose
containers:

```bash
CLONER_DOCKER_TESTS=1 cargo test --test docker -- --test-threads=1
```
GitHub Actions runs the same tests on Linux, macOS and Windows, checks the
MSRV (1.89), and builds release binaries for Linux and Windows.

The legacy Ruby suite uses:

```bash
bundle install
bundle exec ruby test/test_without_rails.rb
bundle exec ruby test/test_docker_compose.rb
```

## Documentation

- [Installation](docs/guides/installer.md) — installer script, manual build,
  PATH and troubleshooting
- [DSL reference](docs/guides/dsl.md) — every config key, hook and global
- [CLI reference](docs/guides/cli.md) — commands, flags, database admin
- [CLI usage for `thor dl` users](docs/guides/cli-usage.md) — old command →
  new command mapping
- [Converting `dl.thor` to `dl.js`](docs/guides/thor-to-js-conversion.md) —
  complete conversion guide
- [Operations and troubleshooting](docs/guides/operations.md) — SSH, Compose
  resolution, backup, security, tests
- [Migrating from dl.thor](docs/guides/migration-from-thor.md) — short method
  mapping and transition plan
- [Dependencies and design](docs/guides/dependencies-and-design.md) — crate
  choices, licensing and rejected alternatives
- [Ruby/Thor README](docs/ruby-readme.md) — legacy implementation

## Design

```text
dl.js (QuickJS)
   │  config + hooks + optional execute()
   ▼
cloner
   ├─ target ── host | docker compose exec | kubectl exec
   ├─ SSH (russh) ── remote compose/env/pod, pg_dump/mysqldump/mongodump
   ├─ SFTP (russh-sftp) ── dump download + incremental file sync
   ├─ local compose exec ── reset + restore
   └─ hooks ── shell(), db.query(), clone_db(), clone_files(), ...
```

The DSL is evaluated before any action. A user-supplied `execute()` replaces
the default `clone_db()` + `clone_files()` pipeline, which makes the tool
usable for custom maintenance/backup workflows without forking it.

## License

MIT. Third-party crates keep their own licenses; see `Cargo.toml` and the
upstream projects. `rustic` is not a dependency of this tool.
