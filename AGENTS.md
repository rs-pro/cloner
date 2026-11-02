# AGENTS.md

## Repository layout

- Root (main): Rust single-binary implementation with a JavaScript DSL (`dl.js`): `Cargo.toml`, `src/`, `tests/`, `install.sh`, `install.ps1`. Prefer changing this unless the task is explicitly about the legacy gem.
- Legacy Ruby/Thor gem (still supported): `lib/cloner/`, generator under `lib/generators/`, tests under `test/`, `cloner.gemspec`, `Gemfile`.
- User docs live in `docs/guides/`; `README.md` is the main entry point and `docs/ruby-readme.md` documents the legacy Ruby option. The `migrate-dl-thor-to-dl-js` skill is `.opencode/skills/migrate-dl-thor-to-dl-js/SKILL.md`.
- New installs use `install.sh` on Linux/macOS and `install.ps1` on Windows (or `cargo build --release` from the repository root).

## Rust commands

```bash
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
- CI paths are `Cargo.toml`, `Cargo.lock`, `src/**`, `tests/**`, `install.sh`, `install.ps1` and the workflow file; Ruby/docs-only changes do not trigger Rust CI.
- Rust edition is 2024, MSRV 1.89. Do not raise either without updating CI and `Cargo.toml`.
- Keep dependencies from crates.io only, permissively licensed. Do not add local path dependencies; `/data/_swap/rsync` is GPL-3.0-or-later and deliberately rejected.

## Legacy Ruby

- Install with `bundle install`.
- There is no `rake test` task; run tests directly:
  `bundle exec ruby test/test_without_rails.rb` and `bundle exec ruby test/test_docker_compose.rb`.
- Generated `dl.thor` templates are in `lib/generators/templates/`; changes there affect new Ruby users only.

## Rust architecture

- `src/lib.rs` exposes reusable modules; `src/main.rs` is the thin clap CLI. Keep primitives usable without the DSL/SSH so the internal backup app can reuse them.
- `src/main.rs`: CLI, config discovery, generated `dl.js` template, `db`/`k8s` option mapping.
- `src/dsl.rs`: QuickJS runtime, config evaluation, lifecycle hooks, `execute()` override, built-ins, and the `DslBuilder`/`DslExtension` embedding API (`DslEnvironment`, `DslCommandRunner`). Downstream products add variables, namespaces/commands and extra hooks through extensions instead of forking the DSL.
- `src/config.rs`: config schema, path/env resolution, `describe()` for `validate`.
- `src/compose.rs`: Compose parsing, service/engine detection, connection inference.
- `src/engine.rs`: database engines and pure dump/restore/query/ping/admin command builders. No execution here.
- `src/runtime.rs`: `CommandTarget` (host / Compose / Kubernetes) that wraps commands for the right place.
- `src/k8s.rs`: `kubectl exec` target, pod/selector resolution types, and the nightly dump CronJob renderer.
- `src/backup.rs`: backup directory layout, versioned JSON manifest, SHA-256, retention/prune.
- `src/shell.rs`: POSIX quoting and local command execution.
- `src/ssh.rs`: `russh` client. Remote commands always go through `sh -lc` because demo hosts use `fish`.
- `src/sync.rs`: SFTP incremental sync (size+mtime, temp file+rename). It does not call the `rsync` binary and has no delta algorithm.
- `src/db.rs` / `src/admin.rs`: orchestration of dump/restore/reset/query and `pg_dev`-style create/reset/drop. MariaDB falls back from `mysql*` to `mariadb*`.
- Unit tests are inline `#[cfg(test)]`; CLI integration tests are `tests/cli.rs`; Docker round trips are `tests/docker.rs` behind `CLONER_DOCKER_TESTS=1`.

## Behavioural gotchas

- `cloner validate` **connects to the remote host** and reads Compose/`.env`; it is not an offline parser. `cloner db ... -c dl.js` resolves only the local side.
- DB resolution priority: explicit `database.local/remote` > Compose service env > service `env_file` > side `.env` > engine defaults. In-container host is forced to `localhost`.
- Service selection: explicit config, then image/env detection, then names like `db`/`postgres`/`rtdb`.
- `execute()` replaces the entire default `clone_db()` + `clone_files()` pipeline and suppresses automatic hooks.
- `database.reset: "drop-create"` deletes local data; `"volume"` runs `docker compose down`, removes/recreates the volume, then brings up the whole Compose project.
- `files: []` means no file sync. Hooks run with cwd `local.root`.
- `cloner restore --from` reuses the local database path: it verifies the manifest (size + SHA-256), optionally runs `database.reset`, then streams `database.restore_args` into the target. It never contacts the remote host.
- `db reset` requires the database to exist; `db create` first.
- The starter template in `main.rs` is covered by a test and must remain valid JavaScript.
- Local commands run through a POSIX shell (`sh -c`) resolved by `util::local_shell()`: `CLONER_SHELL` > `sh`/`bash` on PATH (Windows App Execution Aliases skipped) > Git for Windows `sh.exe`. Remote commands use `sh -lc`.
- Platform differences stay behind `#[cfg(unix)]`/`#[cfg(windows)]`: SSH agent transport (Unix socket vs OpenSSH named pipe/Pageant), POSIX mode bits, symlink creation, `HOME` vs `USERPROFILE`. Keep the crate warning-free on Windows as well.

## Docs

- `docs/guides/`: `installer.md`, `dsl.md`, `cli.md`, `cli-usage.md`, `operations.md`, `thor-to-js-conversion.md`, `migration-from-thor.md`.
- Kubernetes support: `local.kubernetes` / `remote.kubernetes` (`context`, `namespace`, `pod`, `selector`, `container`) and `cloner k8s cronjob`. `compose` is ignored when `kubernetes` is set.
- Keep docs aligned with executable sources (`src/main.rs`, `src/config.rs`, `Cargo.toml`, CI) rather than duplicating stale prose.
- Installer validation: `shellcheck install.sh` and `./install.sh --no-deps --dry-run`; on Windows `pwsh -NoProfile -File install.ps1 -NoDeps -DryRun`.
