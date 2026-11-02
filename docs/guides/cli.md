# Cloner CLI reference

```text
cloner [GLOBAL OPTIONS] [COMMAND]
```

If no command is given, `cloner` runs the default `run` (clone) action.

## Global options

| Option | Meaning |
|---|---|
| `-c, --config <FILE>` | DSL file; default lookup: `dl.js` and standard variants in the current directory and parents |
| `-v, --verbose` | Debug logging |

## `run` / `download`

```bash
cloner run
cloner download                 # alias
cloner run --skip-database      # only sync files
cloner run --skip-files         # only clone the database
cloner run --dry-run            # print commands, do not change anything
cloner run --from staging       # exposed to the DSL as CLONE_FROM
```

| Option | Meaning |
|---|---|
| `-D, --skip-database` | Skip the DB dump/restore step |
| `-F, --skip-files` | Skip the file sync step |
| `--dry-run` | Print commands without executing them |
| `--from <NAME>` | Environment name exposed as `CLONE_FROM` |

## `backup`

```bash
cloner backup --to /var/backups/myapp
cloner backup --to /var/backups/myapp --skip-files
```

Writes:

```text
/var/backups/myapp/
├── dump/
│   └── cloner.bak
├── files/
│   └── <sanitized-remote-path>/
└── manifest.json
```

The backup action does not reset or restore the local database.

| Option | Meaning |
|---|---|
| `-t, --to <DIR>` | Destination directory; default `backup.destination` or `backups/<timestamp>` |
| `-D, --skip-database` | Skip the dump |
| `-F, --skip-files` | Skip file sync |
| `--dry-run` | Print commands only |
| `--from <NAME>` | Environment name exposed as `CLONE_FROM` |

## `validate`

```bash
cloner validate
cloner validate --config /path/to/dl.js
```

Parses the DSL, connects to the remote host, reads Compose/`.env`, resolves the
database connections and prints the effective config as JSON. Passwords are
redacted. This is the quickest way to check service auto-detection.

## `init`

```bash
cloner init
cloner init --force
```

Writes a commented `dl.js` starter template.

## `db` — pg_dev-style database administration

The `db` commands merge the old `pg_dev` tool into `cloner`. They work with or
without a DSL file:

- With `--config`, the local database, Compose project and engine are taken
  from the DSL.
- Without a config, connection defaults come from CLI flags and `PG*` /
  `RAILS_ENV` environment variables.
- `--engine postgres|mysql|mongo` selects the engine explicitly.

```bash
cloner db create app
cloner db reset app --migrate --seed
cloner db reset --dbname app_development --owner app
cloner db drop --dbname app_development
cloner db migrate
cloner db seed
```

### Shared `db` options

| Option | Meaning |
|---|---|
| `[NAME]` | Owner/user name (positional, like `pg_dev`) |
| `--engine <ENGINE>` | `postgres`, `mysql` or `mongo` |
| `--host <HOST>` | Database host; default `localhost` or `PGHOST` |
| `--port <PORT>` | Database port; default engine port or `PGPORT` |
| `--user <USER>` | Admin connection user; default `postgres` or `PGUSER` |
| `--password <PASS>` | Admin password; default `PGPASSWORD` |
| `--auth-db <DB>` | Maintenance database; default `postgres` (PostgreSQL) or `admin` (Mongo) |
| `--env <ENV>` | Environment suffix; default `local.env`, `RAILS_ENV` or `development` |
| `--dbname <DB>` | Database name; default `<owner>_<env>` or the DSL local database |
| `--owner <OWNER>` | Database owner |
| `--schema <SCHEMA>` | Schema for PostgreSQL reset; default `public` |
| `--no-drop` | Do not drop the schema/database first |
| `--no-create` | Do not recreate the schema/database |
| `--set-password <PASS>` | Set the user password (create defaults to the user name) |
| `--hstore` | Create the `hstore` extension after the action |
| `--postgis` | Create the `postgis` extension after the action |
| `--migrate` | Run `bundle exec rake db:migrate` afterwards |
| `--seed` | Run `bundle exec rake db:seed` afterwards |
| `--force` | Allow `env=production` |
| `--dry-run` | Print commands without executing them |

### `db create` (`c`)

Creates the role/user and database, grants privileges, and optionally the
`public` schema privileges. Safe to re-run: existing roles/databases are kept.

```bash
# PostgreSQL
cloner db create app

# with a non-default password
cloner db create app --set-password s3cret

# MySQL
cloner db create app --engine mysql

# MongoDB
cloner db create app --engine mongo
```

### `db reset` (`r`)

Drops and recreates the local schema/database. **This deletes data.**

```bash
# PostgreSQL: DROP SCHEMA public CASCADE; CREATE SCHEMA public AUTHORIZATION app
cloner db reset app

# only drop
cloner db reset app --no-create

# only create
cloner db reset app --no-drop

# a non-public schema
cloner db reset app --schema tenant_1

# use the local DB from dl.js
cloner db reset --config /path/to/dl.js

# reset plus extensions and Rails tasks
cloner db reset app --hstore --postgis --migrate --seed
```

MySQL reset uses `DROP DATABASE` / `CREATE DATABASE`; the `--schema` option is
PostgreSQL-only. MongoDB reset drops the database.

### `db drop` (`d`)

Drops the database. PostgreSQL system databases (`postgres`, `template0`,
`template1`) are refused.

### `db migrate` / `db seed`

Runs `bundle exec rake db:migrate` or `bundle exec rake db:seed` in the project
root (the DSL root when a config is used).

## Safety

- `env=production` is refused by `db` commands unless `--force` is passed.
- `drop-database` refuses PostgreSQL system databases.
- `run` with `database.reset` can delete local data. Use `--dry-run` first.
- Remote commands and local shell commands are logged with `--verbose`.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Any error (config, SSH, database, file sync, hook) |

Errors include the failing command and command output where available.

## Examples

```bash
# Clone both DB and files using ./dl.js
cloner

# Inspect resolution without touching anything
cloner validate

# Non-destructive backup of database + files
cloner backup --to /data/backups/myapp

# Only refresh local uploads
cloner run --skip-database

# Create/reset a scratch PostgreSQL database on the local dev stack
cloner db create scratch --dbname scratch
cloner db reset scratch --dbname scratch
cloner db drop --dbname scratch --force
```
