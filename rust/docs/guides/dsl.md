# Cloner Rust DSL

`cloner` is a single Rust binary that replaces the Ruby/Thor `dl.thor` workflow.
It reads a JavaScript config file (normally `dl.js`), resolves Docker Compose
and `.env` files on both sides, dumps the remote database, syncs files over
SFTP, and restores the database locally.

The DSL is plain JavaScript evaluated by an embedded QuickJS runtime. It has no
Node.js dependency and the binary does not need Ruby, `ssh`, `rsync` or a
JavaScript runtime installed.

- Default config name: `dl.js` (also `cloner.js`, `cloner.config.js`,
  `dl.config.js`, `.cloner.js`)
- Config lookup: current directory and its parents
- Explicit path: `cloner --config path/to/dl.js run`

## Minimal example

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
};
```

Run:

```bash
cloner run
# or simply
cloner
```

## Top-level keys

| Key | Purpose |
|---|---|
| `local` | Local project paths, environment name and local Compose settings |
| `remote` | SSH connection, remote app path and remote Compose settings |
| `database` | Database engine, service selection, dump/restore options, reset mode |
| `files` | Remote → local file sync rules |
| `backup` | Default backup destination |
| `hooks` | Lifecycle hooks |
| `execute` | Optional custom action that replaces the default clone pipeline |

## `local`

```js
local: {
  root: __dirname,          // project root; default: directory containing dl.js
  env: "development",       // local environment name (used by db commands)
  compose: {
    path: "dev-docker",     // relative to local.root
    file: "compose.yml",
    service: "db",          // optional; auto-detected
  },
  envFile: "dev-docker/.env", // optional; default: <compose.path>/.env
}
```

- `local.root` is the working directory for hooks and `shell(...)`, and the
  base for relative `files[].to` paths.
- `local.env` replaces the old `project.env`. `project.root` and `project.env`
  are still accepted as deprecated fallbacks.
- When `local.compose` is present, database commands run inside the database
  container with `docker compose exec -T <service>`.
- If `envFile` is omitted, the file `<compose.path>/.env` is used when it
  exists.

## `remote`

```js
remote: {
  host: "example.com",
  user: "root",             // default: root
  port: 22,                 // default: 22
  appPath: "/srv/app",
  compose: {
    path: "/srv/app",       // default: remote.appPath
    file: "compose.yml",
    service: "postgres",    // optional; auto-detected
  },
  envFile: "/srv/app/.env", // optional; default: <compose.path>/.env
  identityFile: "~/.ssh/id_ed25519",
  passwordEnv: "PROD_SSH_PASSWORD",
  strictHostKeyChecking: true,
  knownHosts: "~/.ssh/known_hosts",
}
```

Authentication order:

1. SSH agent (`SSH_AUTH_SOCK`), if available.
2. Private key: `identityFile`, or `~/.ssh/id_ed25519`, `id_rsa`, `id_ecdsa`.
   Encrypted keys can use `SSH_KEY_PASSPHRASE` or
   `CLONER_SSH_KEY_PASSPHRASE`.
3. Password from `remote.password` or `passwordEnv`.

`strictHostKeyChecking: true` verifies the host key against `knownHosts`
(default `~/.ssh/known_hosts`). With `false` (the default) the fingerprint is
logged and the key is accepted; use `true` for production hosts.

Remote commands are always executed through `sh -lc`, so hosts whose login
shell is `fish` (or another non-POSIX shell) work correctly.

## `database`

```js
database: {
  engine: "postgres",          // postgres | mysql | mongo
  service: "postgres",         // shorthand for both remote/local service
  remoteService: "postgres",   // remote Compose service override
  localService: "db",          // local Compose service override
  remoteDumpPath: "/srv/app/tmp_dump",
  dumpName: "cloner.bak",
  dumpArgs: ["-Fc"],
  restoreArgs: ["--no-owner", "-Fc", "-c", "--if-exists"],
  reset: "drop-create",        // none | drop-create | volume
  volume: "myapp_postgres",    // required for reset: "volume"
  remote: { host, port, database, user, password },
  local:  { host, port, database, user, password },
}
```

### Resolution order

For each side, values are resolved with this priority:

1. Explicit `database.remote.*` / `database.local.*` values.
2. The selected Compose service's `environment` (after `${...}` interpolation).
3. Compose service `env_file` files.
4. The side's `.env` file.
5. Engine defaults (`localhost` and `5432` / `3306` / `27017` inside a
   container).

The database service is selected by:

1. `database.service` / `remoteService` / `localService`.
2. `remote.compose.service` / `local.compose.service`.
3. Auto-detection from the image (`postgres`, `pgvector`, `mysql`, `mariadb`,
   `mongo`) or environment keys (`POSTGRES_DB`, `MYSQL_DATABASE`,
   `MONGO_INITDB_DATABASE`).

Inside a Compose network the host is forced to `localhost` and the container's
internal port is used, because dumps and restores run inside the database
container. Outside Compose, `DB_HOST` / `DB_PORT` (or `PGHOST` / `PGPORT`) are
used.

### Reset modes

| Mode | Behavior |
|---|---|
| `none` | Do not reset; restore into the existing database |
| `drop-create` | Start the local service, drop and recreate the database (PostgreSQL/MySQL) or drop it (Mongo) |
| `volume` | `docker compose down`, remove and recreate `volume`, `docker compose up -d` (all services), wait for the DB |

`reset: "volume"` requires `database.volume`.

### Dump and restore commands

- PostgreSQL: `pg_dump -Fc` and `pg_restore --no-owner -Fc -c --if-exists`
- MySQL: `mysqldump --add-drop-table --single-transaction` and `mysql`
- MongoDB: `mongodump --archive` and `mongorestore --archive --drop`

The dump is written on the remote host, transferred over SFTP, and stored in
`<local.root>/tmp/dump/<name>` (or in the backup directory in backup mode).

## `files`

```js
files: [
  { from: "public/uploads", to: "public/uploads" },
  { from: "storage", to: "storage", delete: true, exclude: ["**/*.tmp"] },
  { from: "/var/log/app", to: "tmp/remote-logs", maxSize: 10485760 },
]
```

| Field | Meaning |
|---|---|
| `from` | Remote path. Relative paths are resolved against `remote.appPath` |
| `to` | Local path. Relative paths are resolved against `local.root` |
| `delete` | Remove local files that no longer exist remotely |
| `exclude` | Glob patterns to skip |
| `include` | If set, only matching paths are copied |
| `maxSize` | Skip files larger than this many bytes |
| `followSymlinks` | Follow symlinks instead of recreating them |

The transfer is an rsync-like incremental sync over SFTP: files are compared by
size and modification time, written to a temporary file, then renamed into
place. Permissions and modification times are preserved. Directories and
symlinks are supported.

> The local `/data/_swap/rsync` crate is GPL-3.0-or-later, so it is not linked
> into this MIT-licensed binary. File sync uses `russh` + `russh-sftp`, both
> permissively licensed.

## `backup`

```js
backup: {
  destination: "backups",  // relative to local.root
  keep: 7,                 // reserved for retention tooling
}
```

Backup mode:

```bash
cloner backup --to /var/backups/myapp
```

This dumps the database to `<to>/dump/`, syncs configured files to
`<to>/files/`, and writes `<to>/manifest.json` with timestamps, sizes and file
counts. It does not reset or restore the local database.

## `hooks`

```js
hooks: {
  beforeAll() {},
  beforeDatabase() {},
  beforeRestore() {},
  afterRestore() {
    shell("bin/rails db:migrate");
    db.query("UPDATE settings SET host = 'http://localhost:3000'");
  },
  afterDatabase() {},
  beforeFiles() {},
  afterFiles() {},
  afterAll() {},
}
```

Order in the default action:

```text
beforeAll
  beforeDatabase
    beforeRestore
    reset + restore
    afterRestore
  afterDatabase
  beforeFiles
    sync files
  afterFiles
afterAll
```

In backup mode the restore-related hooks are skipped; `afterDatabase` runs after
the dump has been downloaded.

Hooks must be synchronous. Returning a Promise is an error.

## `execute()` — overriding the default action

By default `cloner` runs `clone_db()` and then `clone_files()`. If the exported
object contains an `execute` function, it replaces the whole default pipeline.
This lets a project do something completely different, or run only one part:

```js
module.exports = {
  // ... local / remote / database / files ...

  execute() {
    shell("echo custom maintenance");
    // clone_db();
    // clone_files();
  },
};
```

Available actions:

| Function | Effect |
|---|---|
| `clone_db()` | Dump the remote database, download it, reset the local DB and restore |
| `clone_files()` | Sync all configured `files` entries |
| `clone_all()` | `clone_db()` followed by `clone_files()` |
| `db.reset()` | Reset the local database using `database.reset` |
| `db.query(sql)` | Run SQL against the local database |
| `sync(from, to)` | Sync one remote path to one local path |

Hooks are **not** called automatically when `execute()` is defined. If you want
them, call them from your `execute` function or use the default pipeline.

## Globals available in the DSL

### Shell

```js
shell("go run main.go migrate");        // local, cwd = local.root, output echoed
shell.in("/tmp", "ls -la");             // local, explicit cwd
shell.remote("docker compose ps");       // remote, through SSH

sh.local("cmd");                         // local, cwd = local.root, output returned
sh.localIn("/tmp", "cmd");               // local, explicit cwd
sh.remote("cmd");                        // remote
```

`shell(...)` prints command output and returns stdout. `sh.local(...)` only
returns stdout, which is useful when you want to parse it.

### Environment

```js
env.local("DB_NAME");
env.remote("DB_PASSWORD");
process.env.CLONE_FROM;
```

`env.local` reads the resolved local `.env` values; `env.remote` reads the
resolved remote `.env` values.

### Database

```js
db.query("SELECT count(*) FROM users");
db.reset();
db.clone();
```

### Compose

```js
compose.local("ps");
compose.remote("logs", "--tail", "20");
```

### Utilities

```js
log("message");
console.log("message", { any: "value" });
sleep(500);
sync("public/uploads", "public/uploads");
```

`__dirname` and `__filename` are available like in Node.js. `dirname` and
`basename` accept both `/` and `\` separators, so they work with Windows paths
as well as remote POSIX paths.

## `defineConfig`

`defineConfig` is an identity helper for editor autocompletion and readability:

```js
module.exports = defineConfig({
  local: { root: __dirname },
  // ...
});
```

## Environment variables

| Variable | Meaning |
|---|---|
| `CLONE_FROM` | `--from` value exposed to the DSL |
| `SSH_AUTH_SOCK` | SSH agent socket |
| `SSH_KEY_PASSPHRASE` | Passphrase for an encrypted identity file |
| `CLONER_SSH_KEY_PASSPHRASE` | Alternative passphrase variable |
| `CLONER_SHELL` | POSIX shell used for local commands (Windows default: Git for Windows `sh.exe`) |
| `PGHOST` / `PGPORT` / `PGUSER` / `PGPASSWORD` | Defaults for `cloner db` commands |
| `RAILS_ENV` | Default environment for `cloner db` commands |

## See also

- [Installation](installer.md)
- [CLI reference](cli.md)
- [CLI usage for `thor dl` users](cli-usage.md)
- [Operations and troubleshooting](operations.md)
- [Converting `dl.thor` to `dl.js`](thor-to-js-conversion.md)
- [Migrating from dl.thor](migration-from-thor.md)
- [Dependencies and design](dependencies-and-design.md)
