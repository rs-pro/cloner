# Operations and troubleshooting

## What the binary needs

`cloner` is a single Rust binary. It does not need Ruby, Node.js, `ssh`,
`rsync`, `scp`, or a JavaScript runtime.

For a Compose-based project it expects:

- Docker and the Compose v2 plugin on the local machine.
- The database image to contain the corresponding dump/restore tools
  (`pg_dump`/`pg_restore`, `mysqldump`/`mysql`, `mongodump`/`mongorestore`).
- `bundle` on the local machine only if `db --migrate` / `db --seed` or a Rails
  hook is used.
- The project's own commands (`go`, `bin/rails`, etc.) only if hooks call them.

Remote file transfer uses SFTP. The remote SSH server must have the SFTP
subsystem enabled (the OpenSSH default).

## Building

```bash
cd rust
cargo build --release
./target/release/cloner --version
```

The release profile uses thin LTO, one codegen unit and strips symbols. The
result links the QuickJS C sources statically; no shared JavaScript runtime is
required.

For a fully static Linux binary, build for the musl target:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

## SSH

- Authentication tries the SSH agent first, then key files, then password.
- On Windows the agent is reached over the OpenSSH named pipe (`SSH_AUTH_SOCK`,
  defaulting to `\\.\pipe\openssh-ssh-agent`) or Pageant.
- Useful environment variables:
  - `SSH_AUTH_SOCK` — agent socket (Unix) or named pipe (Windows)
  - `SSH_KEY_PASSPHRASE` / `CLONER_SSH_KEY_PASSPHRASE` — encrypted key
  - `PROD_SSH_PASSWORD` and `remote.passwordEnv` — password auth
- `strictHostKeyChecking: true` verifies against `known_hosts`; the default
  `false` logs the fingerprint and accepts it. Use `true` in production.
- Remote commands run through `sh -lc`, so `fish` login shells do not break
  `&&`, redirects, or subshells.
- Local commands run through `sh -c` as well. On Windows cloner uses Git for
  Windows' `sh.exe`; set `CLONER_SHELL` to choose another POSIX shell.
- A long-running dump uses one SSH session; SFTP transfers open separate
  channels on the same session.

## Compose and `.env` resolution

Both sides are resolved independently:

1. Read the side's `.env` file.
2. Parse the Compose file and interpolate `${VAR}`, `${VAR:-default}` and
   `${VAR:?message}` from that `.env`.
3. Select the database service (explicit config, then image/env detection).
4. Merge service `environment`, service `env_file` files and the app `.env`.
5. Apply explicit `database.local` / `database.remote` overrides.

Because dumps/restores run inside the DB container, the resolved host is
`localhost` and the port is the container-internal port. Outside Compose the
`DB_HOST` / `DB_PORT` values are used directly.

`cloner validate` prints the resolved values (passwords redacted), which is the
fastest way to debug a wrong service or database name.

## Reset behavior

- `drop-create` starts the local DB service, waits for readiness, then drops and
  recreates the database/schema.
- `volume` runs `docker compose down`, removes and recreates the configured
  volume, then `docker compose up -d` for the **whole** Compose project. This
  matters for apps whose migrations need Redis/OpenSearch/etc.
- After a volume reset the tool waits for the database only. If a hook needs
  another service (for example OpenSearch), add a readiness wait to the hook or
  start that service before the hook.

## Backup mode

`cloner backup --to DIR` is safe to run against production: it only reads the
remote database and files, writes to `DIR`, and never touches the local DB.

Layout:

```text
DIR/
├── dump/<dump-name>
├── files/<sanitized-remote-path>/
└── manifest.json
```

The manifest records the config path, project root, remote host, database
name/user, dump size, and per-source file counts. It does not contain passwords.

## MySQL and MongoDB

`database.engine` accepts `postgres`, `mysql` and `mongo`.

- `mysql` uses `mysqldump` / `mysql` and `database.reset: "drop-create"`.
  MariaDB images (which only ship `mariadb`, `mariadb-dump` and
  `mariadb-admin`) are detected automatically.
- `mongo` uses `mongodump --archive` / `mongorestore --archive --drop`.
- `cloner db create/reset/drop` supports all three engines.
- `hstore` / `postgis` extensions are PostgreSQL-only and are rejected for
  MySQL/Mongo.

The Compose service auto-detection recognizes `mysql`, `mariadb`, `mongo`,
`postgres` and `pgvector` images, plus the usual `POSTGRES_*`, `MYSQL_*` and
`MONGO_INITDB_*` environment keys.

## Security notes

- Database passwords are interpolated into shell commands, just like the old
  Ruby tool. They can briefly appear in the process list of the DB container.
- Passwords are redacted from `validate` output and are not written to the
  backup manifest.
- `cloner db ...` refuses `env=production` unless `--force` is passed.
- `cloner db drop` refuses PostgreSQL system databases.
- The default host-key policy is permissive for developer convenience. Set
  `strictHostKeyChecking: true` for production or shared hosts.
- File sync does not follow symlinks by default; it recreates them.

## Troubleshooting

### `database ... does not exist; run cloner db create first`

`db reset` only resets an existing database. Create it first:

```bash
cloner db create app
```

### `cannot determine database engine; set database.engine`

The Compose service was not detected. Set `database.engine` and/or
`database.service` explicitly, then run `cloner validate`.

### `no usable SSH authentication method`

Check:

- `ssh -p <port> <user>@<host>` works in the same terminal.
- The key is present in the agent (`ssh-add -l`) or `identityFile` is correct.
- An encrypted key needs `SSH_KEY_PASSPHRASE`.
- `remote.passwordEnv` points to a non-empty variable for password auth.

### `remote command failed with exit code 127`

Usually a missing remote binary or a wrong Compose service. Check `validate`
output and run the printed command manually with `--verbose`.

### Local restore is slow

A `pg_restore` of a multi-GB database takes minutes. The dump is downloaded
once; keep it in `tmp/dump` for repeat restores. `--skip-database` skips the
whole DB step when you only need files.

### `db reset` stops my other dev services

That is the `volume` reset mode, which runs `docker compose down` and then
brings the whole project up again. Use `drop-create` if you want to keep the
other services running.

## Tests performed

Validated against the two demo projects:

- `chaos-router`: remote Compose `postgres` service → local `dev-docker` `db`
  service, `drop-create` reset, 361 MiB dump, 6.2 GB database restored.
- `rtrack-go`: remote Compose `rtdb` service → local `dev-docker` `db` service,
  `volume` reset, 471 MiB dump, 2.1 GB database restored, `go run main.go
  migrate`, OAuth domain updates and `go run main.go reindex` via `afterRestore`.
- File sync over SFTP was validated on a remote config directory and on the
  rtrack `uploads` directory.
- `cloner db create/reset/drop` was validated on PostgreSQL, MariaDB 11 and
  MongoDB 7 (including Compose service detection and the MariaDB binary
  fallback).
