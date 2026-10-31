# Cloner (Rust)

A single-binary replacement for the Ruby/Thor `cloner` workflow. It clones a
remote database and files into a local development environment, and doubles as a
standalone backup and database-reset tool.

- JavaScript DSL (`dl.js`) similar in spirit to `dl.thor`
- SSH via [`russh`](https://github.com/warp-tech/russh) (Apache-2.0)
- Incremental file sync over SFTP (`russh-sftp`, MIT/Apache-2.0)
- Embedded QuickJS via `rquickjs` (MIT) — no Node.js dependency
- PostgreSQL, MySQL and MongoDB
- Reads Docker Compose and `.env` files; no need to repeat DB credentials
- `pg_dev`-style `cloner db create/reset/drop/migrate/seed`
- Backup mode with a JSON manifest
- No Ruby, `ssh`, `rsync`, `scp` or JavaScript runtime at runtime

> The local `/data/_swap/rsync` crate is GPL-3.0-or-later. It is deliberately not
> linked into this MIT-licensed binary; file sync is implemented on top of
> `russh-sftp`.

## Quick start

```bash
# install to ~/.local/bin (installs build deps + Rust if needed)
./rust/install.sh

# create a starter config
cloner init

# inspect the resolved config (passwords redacted)
cloner validate

# clone database + files
cloner run

# non-destructive backup
cloner backup --to /var/backups/myapp
```

Or build manually:

```bash
cd rust
cargo build --release
./target/release/cloner --help
```

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

## Commands

```text
cloner run [--skip-database] [--skip-files] [--dry-run] [--from ENV]
cloner backup --to DIR
cloner validate
cloner init
cloner db create|reset|drop|migrate|seed ...
```

See the full [CLI reference](docs/guides/cli.md).

## Testing

```bash
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
```

The suite contains unit tests for config/Compose resolution, dotenv and
interpolation handling, shell quoting, database command construction, sync
filters, the JavaScript DSL evaluator and the starter template, plus CLI
integration tests for `init`, config discovery, `db` actions and dry runs.
GitHub Actions runs the same tests on Linux and macOS, checks the MSRV (1.89),
and builds a release binary.

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

## Design

```text
dl.js (QuickJS)
   │  config + hooks + optional execute()
   ▼
cloner
   ├─ SSH (russh) ── remote compose/env, pg_dump/mysqldump/mongodump
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
