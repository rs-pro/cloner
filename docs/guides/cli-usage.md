# CLI usage after `thor dl`

This page is for projects moving from the Ruby/Thor command:

```bash
bundle exec thor dl
```

to the Rust binary:

```bash
cloner
```

The clone behaviour is the same, but the binary is standalone: no Bundler, no
Ruby, no `thor`, no `ssh`/`rsync`/`scp` binaries. Run `cloner --help` for the
built-in reference.

Installation: [installer guide](installer.md).

## Quick command mapping

| Old command | New command |
|---|---|
| `bundle exec thor dl` | `cloner` or `cloner run` |
| `bundle exec thor dl download` | `cloner run` |
| `bundle exec thor dl help` | `cloner --help` |
| `bundle exec thor dl download --from staging` | `cloner run --from staging` |
| `bundle exec thor dl download -D` | `cloner run -D` / `cloner run --skip-database` |
| `bundle exec thor dl download -F` | `cloner run -F` / `cloner run --skip-files` |
| `CLONE_FROM=staging bundle exec thor dl` | `CLONE_FROM=staging cloner` |
| `bundle exec thor dl` with `verbose?` overridden | `cloner -v run` |

`download` is kept as an alias for `run`, so scripts that used the explicit
Thor task name can also use `cloner download`.

## Before you start

The project needs a `dl.js` file at the root (or one of the accepted names
`cloner.js`, `cloner.config.js`, `dl.config.js`, `.cloner.js`). Convert
`dl.thor` with the [conversion guide](thor-to-js-conversion.md), then check it:

```bash
cloner validate
```

`validate` parses the DSL, connects to the remote host, reads Compose and
`.env`, resolves both databases, and prints the effective config with passwords
redacted. Run it before the first real clone.

## Daily commands

```bash
# Clone database and configured files
cloner

# Explicit form
cloner run

# Only the database
cloner run --skip-files

# Only files
cloner run --skip-database

# Print commands without changing local services or files
cloner run --dry-run

# Debug SSH, Compose and database resolution
cloner -v run
```

The short flags `-D` and `-F` match the old Thor options.

## Selecting an environment / stage

Legacy configs often use a stage hash and `--from`:

```bash
bundle exec thor dl download --from staging
```

The new equivalent is:

```bash
cloner run --from staging
```

`--from` is exposed to the DSL as `CLONE_FROM`, so the config can pick a stage:

```js
const stage = (process.env.CLONE_FROM || "production");
```

The same works through the environment, without the flag:

```bash
CLONE_FROM=staging cloner run
```

Unlike the old template, there is no hard-coded default of `production` in the
binary. Your `dl.js` decides what the default is.

## Configuration file selection

The old Thor task lived at `lib/tasks/dl.thor` and was found by Bundler/Thor.
The new binary looks for the DSL in the current directory and its parents:

```text
dl.js
cloner.js
cloner.config.js
dl.config.js
.cloner.js
```

Pass an explicit file when needed:

```bash
cloner --config path/to/dl.js run
cloner -c path/to/dl.js validate
```

The `--config` / `-c` option is global and can appear before or after the
subcommand.

## New commands that did not exist in `thor dl`

### Validate

```bash
cloner validate
```

No remote changes, no local changes. Use it after editing `dl.js`, after
changing Compose services, or when a clone picks the wrong database.

### Backup

```bash
cloner backup --to /var/backups/myapp
cloner backup --to /var/backups/myapp --skip-files
```

Writes `dump/`, `files/` and `manifest.json` under the destination. It does not
reset or restore the local database, so it is safe against production.

### Init

```bash
cloner init
cloner init --force
```

Writes a commented starter `dl.js`; `--force` overwrites an existing file.

### Database administration (`pg_dev` replacement)

The old workflow often used `pg_dev` or hand-written `dropdb`/`createdb`
commands. These are now built in and work with or without a DSL file:

```bash
cloner db create app
cloner db reset app --migrate --seed
cloner db drop --dbname app_development
cloner db migrate
cloner db seed
```

Engine selection:

```bash
cloner db create app --engine mysql
cloner db create app --engine mongo
cloner db reset app --engine postgres --dbname app_test
```

With a DSL file, the local connection, Compose project and engine come from
`dl.js`:

```bash
cloner db reset -c dl.js
```

Safety:

- `env=production` is refused unless `--force` is passed.
- `db drop` refuses PostgreSQL system databases.
- Use `--dry-run` to inspect the generated commands.

Full option list: [CLI reference](cli.md).

## Behaviour differences

- **No Bundler.** `bundle exec` is not needed and the `cloner` gem can be
  removed once all projects use `dl.js`.
- **No `thor` task arguments.** `cloner` has subcommands and flags, not Thor
  task names. `download` remains an alias for compatibility.
- **`--from` is not a database selector.** It only sets `CLONE_FROM` for the
  DSL. The config decides which stage/connection to use.
- **File sync is SFTP-based.** The binary does not call the `rsync` executable.
  The same `files` entries are copied incrementally by size and modification
  time.
- **Hooks are synchronous JavaScript.** Long-running app commands should be
  called with `shell("...")`.
- **Exit status is meaningful.** `0` means success; any config, SSH, database,
  hook or file-sync error returns `1` and prints the failing command.

## Automation examples

### Make alias

```bash
alias dl='cloner run'
alias dl-files='cloner run --skip-database'
```

### Nightly backup

```bash
#!/usr/bin/env bash
set -euo pipefail
cd /srv/myapp
cloner backup --to "/var/backups/myapp/$(date +%F)"
```

### CI smoke test (no remote changes)

```bash
cloner validate
cloner run --dry-run
```

### Local reset before tests

```bash
cloner db reset app --migrate --seed
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no DSL file found` | `cd` to the project or pass `-c path/to/dl.js` |
| `refusing to modify a production database` | Check `local.env`/`--env`; pass `--force` only intentionally |
| SSH authentication error | Check `remote.identityFile`, `SSH_AUTH_SOCK`, or `remote.passwordEnv` |
| Wrong DB selected | Run `cloner validate`; set `database.service` or `database.local`/`remote` |
| Files not synced | Add `files` entries; an empty array means no file step |
| `download` command missing | The binary is old; upgrade and run `cloner --help` |

More: [operations and troubleshooting](operations.md).
