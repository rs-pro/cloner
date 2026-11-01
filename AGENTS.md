# AGENTS.md

## Repository layout

- Root: legacy Ruby/Thor gem (`lib/cloner/`, generator under `lib/generators/`, tests under `test/`).
- `rust/`: active single-binary rewrite with a JavaScript DSL (`dl.js`). Prefer changing this unless the task is explicitly about the legacy gem.
- User docs live in `rust/docs/guides/`; the `migrate-dl-thor-to-dl-js` skill is `.opencode/skills/migrate-dl-thor-to-dl-js/SKILL.md`.
- New installs use `rust/install.sh` on Linux/macOS and `rust/install.ps1` on Windows (or `cargo build --release` from `rust/`).

## Rust commands

```bash
cd rust
cargo fmt --all
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

Run one test by path/name:

```bash
cargo test config::tests::resolves_compose_service_from_env
cargo test --test cli prints_version
```

- CI (`.github/workflows/rust.yml`) runs fmt, clippy with `-D warnings`, unit tests, CLI integration tests and MSRV 1.89 on Linux/macOS/Windows, plus release builds for Linux and Windows.
- CI paths are only `rust/**` and the workflow file; root-only changes do not trigger Rust CI.
- Rust edition is 2024, MSRV 1.89. Do not raise either without updating CI and `Cargo.toml`.
- Keep dependencies from crates.io only, permissively licensed. Do not add local path dependencies; `/data/_swap/rsync` is GPL-3.0-or-later and deliberately rejected.

## Legacy Ruby

- Install with `bundle install`.
- There is no `rake test` task; run tests directly:
  `bundle exec ruby test/test_without_rails.rb` and `bundle exec ruby test/test_docker_compose.rb`.
- Generated `dl.thor` templates are in `lib/generators/templates/`; changes there affect new Ruby users only.

## Rust architecture

- `src/main.rs`: clap CLI, config discovery, generated `dl.js` template, `db` option mapping.
- `src/dsl.rs`: QuickJS runtime, config evaluation, lifecycle hooks, `execute()` override, built-ins.
- `src/config.rs`: config schema, Compose/`.env` parsing, service detection, DB resolution.
- `src/ssh.rs`: `russh` client. Remote commands always go through `sh -lc` because demo hosts use `fish`.
- `src/sync.rs`: SFTP incremental sync (size+mtime, temp file+rename). It does not call the `rsync` binary and has no delta algorithm.
- `src/db.rs` / `src/admin.rs`: dump/restore/reset/query and `pg_dev`-style create/reset/drop. DB tools run inside Compose containers; MariaDB falls back from `mysql*` to `mariadb*`.
- Only `cloner` binary target exists; modules are declared in `main.rs`. Unit tests are inline `#[cfg(test)]`; CLI integration tests spawn `CARGO_BIN_EXE_cloner` from `rust/tests/cli.rs`.

## Behavioural gotchas

- `cloner validate` **connects to the remote host** and reads Compose/`.env`; it is not an offline parser. `cloner db ... -c dl.js` resolves only the local side.
- DB resolution priority: explicit `database.local/remote` > Compose service env > service `env_file` > side `.env` > engine defaults. In-container host is forced to `localhost`.
- Service selection: explicit config, then image/env detection, then names like `db`/`postgres`/`rtdb`.
- `execute()` replaces the entire default `clone_db()` + `clone_files()` pipeline and suppresses automatic hooks.
- `database.reset: "drop-create"` deletes local data; `"volume"` runs `docker compose down`, removes/recreates the volume, then brings up the whole Compose project.
- `files: []` means no file sync. Hooks run with cwd `local.root`.
- `db reset` requires the database to exist; `db create` first.
- The starter template in `main.rs` is covered by a test and must remain valid JavaScript.
- Local commands run through a POSIX shell (`sh -c`) resolved by `util::local_shell()`: `CLONER_SHELL` > `sh`/`bash` on PATH (Windows App Execution Aliases skipped) > Git for Windows `sh.exe`. Remote commands use `sh -lc`.
- Platform differences stay behind `#[cfg(unix)]`/`#[cfg(windows)]`: SSH agent transport (Unix socket vs OpenSSH named pipe/Pageant), POSIX mode bits, symlink creation, `HOME` vs `USERPROFILE`. Keep the crate warning-free on Windows as well.

## Docs

- `rust/docs/guides/`: `installer.md`, `dsl.md`, `cli.md`, `cli-usage.md`, `operations.md`, `thor-to-js-conversion.md`, `migration-from-thor.md`.
- Keep docs aligned with executable sources (`src/main.rs`, `src/config.rs`, `Cargo.toml`, CI) rather than duplicating stale prose.
- Installer validation: `shellcheck rust/install.sh` and `cd rust && ./install.sh --no-deps --dry-run`; on Windows `pwsh -NoProfile -File rust/install.ps1 -NoDeps -DryRun`.
