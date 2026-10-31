# Converting a legacy `dl.thor` to `dl.js`

This guide is a complete, mechanical conversion reference for projects that
still use the Ruby/Thor `cloner` gem (`bundle exec thor dl`). It covers every
override used by the generated templates and the real-world `dl.thor` files in
`chaos-router` and `rtrack-go`, plus the behavioural differences to watch for.

The new tool is the Rust binary in [`rust/`](../../README.md). It reads a normal
JavaScript file, so the conversion is mostly replacing Ruby method overrides
with config keys and replacing custom `download` code with hooks or `execute()`.

See also:

- [DSL reference](dsl.md) — every config key, hook and global
- [CLI usage for Thor users](cli-usage.md) — command-line mapping
- [Installation](installer.md) — building the binary
- [Migrating from `dl.thor`](migration-from-thor.md) — short overview

## 1. How the old files are structured

A legacy `dl.thor` always looks like this:

```ruby
require "cloner"

class Dl < Cloner::Base
  no_commands do
    # configuration overrides, helper methods
  end

  desc "download", "clone files and DB from production"
  option :from, default: "production"

  def download
    # the action: dump, copy, reset, restore, rsync, post-process
  end
end
```

The important parts:

- `no_commands` methods configure the clone: SSH, paths, Compose, DB config.
- `download` is the action.
- `options[:from]` / `ENV["CLONE_FROM"]` select an environment.
- The base class already implements dump, download, reset, restore and rsync.
- Projects override only the pieces they need. Anything not overridden (for
  example internal `pg_dump` command construction) should **not** be converted;
  the Rust tool implements the equivalent internally.

The new file is data plus optional functions:

```js
module.exports = {
  local: { ... },
  remote: { ... },
  database: { ... },
  files: [ ... ],
  hooks: { ... },
  // optional: execute() { ... }
};
```

## 2. Conversion procedure

1. **Copy the SSH settings** — `ssh_host`, `ssh_user`, `ssh_opts[:port]`.
2. **Copy the remote paths** — `remote_app_path`, `remote_dump_path`.
3. **Copy the Compose settings** — remote/local Compose path, file and service.
4. **Copy the local project settings** — `project_root`/`Rails.root` →
   `local.root`, `project_env`/`Rails.env` → `local.env`.
5. **Delete the explicit DB config** (`local_db_config`, `remote_db_config`) and
   let Compose/`.env` resolution find the values. Add overrides only for values
   that cannot be inferred.
6. **Map the reset method** to `database.reset`: `"none"`, `"drop-create"` or
   `"volume"`.
7. **Map rsync calls** to `files` entries.
8. **Map the body of `download`**:
   - calls to built-ins (`pg_dump_remote`, `pg_dump_copy`, `pg_dump_restore`,
     `clone_db`) disappear;
   - post-restore commands move to `hooks.afterRestore()`;
   - anything that is not a normal clone becomes `execute()`.
9. **Validate**: `cloner validate`, then `cloner run --dry-run`, then a real run.
10. **Remove `dl.thor`** and the `cloner` gem once the new file is verified.

## 3. Complete method mapping

### SSH and remote paths

| Legacy Ruby | New JavaScript |
|---|---|
| `ssh_host` | `remote.host` |
| `ssh_user` | `remote.user` (default `root`) |
| `ssh_opts { port: 63511 }` | `remote.port: 63511` |
| `remote_app_path` | `remote.appPath` |
| `remote_dump_path` | `database.remoteDumpPath` or `remote.dumpPath` |
| `ssh_opts` with `keys:` / `password:` | `remote.identityFile` / `remote.passwordEnv` |
| custom `do_ssh` wrapper | not needed; the binary connects itself |

Relative `remote.dumpPath` values are resolved against `remote.appPath`.
A relative `database.remoteDumpPath` is resolved the same way.

### Compose

| Legacy Ruby | New JavaScript |
|---|---|
| `remote_docker_compose? true` | `remote.compose: { ... }` |
| `remote_docker_compose_service` | `remote.compose.service` or `database.remoteService` |
| `remote_docker_compose_path` | `remote.compose.path` (default `remote.appPath`) |
| `remote_docker_compose_file` | `remote.compose.file` (default `compose.yml`) |
| `local_docker_compose? true` | `local.compose: { ... }` |
| `local_docker_compose_service` | `local.compose.service` or `database.localService` |
| `local_docker_compose_path` | `local.compose.path` (relative to `local.root`) |
| `local_docker_compose_file` | `local.compose.file` (default `compose.yml`) |
| `compose_cmd` helper | not needed; the tool builds the command |

If `service` is omitted, the tool detects the database service from the image
(`postgres`, `pgvector`, `mysql`, `mariadb`, `mongo`) or from environment keys
(`POSTGRES_DB`, `MYSQL_DATABASE`, `MONGO_INITDB_DATABASE`). When detection is
ambiguous, set it explicitly.

### Project and database configuration

| Legacy Ruby | New JavaScript |
|---|---|
| `project_root` / `Rails.root` | `local.root: __dirname` |
| `project_env` / `Rails.env` | `local.env: "development"` |
| `Rails` shim class (`self.root`, `self.env`) | remove it |
| `local_db_config` | auto-detected, or `database.local: { ... }` |
| `remote_db_config` | auto-detected, or `database.remote: { ... }` |
| `read_local_env("DB_NAME")` | `env.local("DB_NAME")`, or use auto-detection |
| `read_remote_env("DB_NAME")` | `env.remote("DB_NAME")`, or use auto-detection |
| `load_env` | remove; `.env` and Compose are loaded automatically |
| `verbose? true` | run with `-v` / `--verbose` |

For each side the resolver priority is:

1. explicit `database.local.*` / `database.remote.*`
2. selected Compose service `environment`
3. service `env_file` files
4. the side's `.env`
5. engine defaults

Explicit `database.local` / `database.remote` fields are `host`, `port`,
`database`, `user`, `password`.

### Dump, restore, reset

| Legacy Ruby | New JavaScript |
|---|---|
| `pg_dump_param`, `pg_dump_extra` | `database.dumpArgs: ["-Fc", ...]` |
| `pg_restore_param` | `database.restoreArgs: ["--no-owner", "-Fc", "-c", "--if-exists"]` |
| `my_dump_param`, `my_restore_param` | `database.dumpArgs` / `database.restoreArgs` |
| `reset_development_database` | `database.reset: "drop-create"` |
| `reset_db_volume` | `database.reset: "volume"`, `database.volume: "<name>"` |
| custom pre-restore commands | `hooks.beforeRestore()` |
| custom post-restore commands | `hooks.afterRestore()` |
| `clone_db` inside `download` | remove; it is the default, or call `clone_db()` in `execute()` |

Reset modes:

- `none` — keep the local database and restore into it.
- `drop-create` — start the local Compose DB service, then drop/recreate the
  database (or the PostgreSQL schema).
- `volume` — `docker compose down`, remove and recreate `database.volume`, then
  `docker compose up -d` for the whole project. This is the replacement for a
  hand-written `reset_db_volume` and is required when migrations need other
  project services.

### Files

| Legacy Ruby | New JavaScript |
|---|---|
| `rsync_public("uploads")` | `files: [{ from: "public/uploads", to: "public/uploads" }]` |
| `rsync_public("ckeditor_assets")` | `files: [{ from: "public/ckeditor_assets", to: "public/ckeditor_assets" }]` |
| `rsync("remote/path", "local/path")` | `files` entry, or `sync("remote/path", "local/path")` |
| excludes / extra options | `exclude`, `include`, `delete`, `maxSize`, `followSymlinks` |
| no file sync | `files: []` |

`files[].from` is remote and relative to `remote.appPath` unless absolute.
`files[].to` is local and relative to `local.root`.

### Control flow and hooks

| Legacy Ruby | New JavaScript |
|---|---|
| body of `download` | default pipeline |
| custom code before everything | `hooks.beforeAll()` |
| custom code before DB work | `hooks.beforeDatabase()` |
| custom code before restore | `hooks.beforeRestore()` |
| custom code after restore | `hooks.afterRestore()` |
| custom code after DB work | `hooks.afterDatabase()` |
| custom code around files | `hooks.beforeFiles()` / `hooks.afterFiles()` |
| custom code at the end | `hooks.afterAll()` |
| a completely custom `download` | `execute() { ... }` |
| helper Ruby methods | ordinary JavaScript functions |

Lifecycle order in the default action:

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

`execute()` replaces the entire default action. Hooks are **not** called
automatically from `execute()`.

### Environments / stages

Legacy files often keep a hash of stages and use `options[:from]`:

```ruby
def stages
  {
    production: { ssh_host: "prod.example.com", ssh_user: "deploy" },
    staging:    { ssh_host: "staging.example.com", ssh_user: "deploy" },
  }
end
```

In `dl.js`, use the `CLONE_FROM` value (set by `--from`) as a normal JS lookup:

```js
const stages = {
  production: { host: "prod.example.com", user: "deploy" },
  staging: { host: "staging.example.com", user: "deploy" },
};

const from = process.env.CLONE_FROM || "production";
const stage = stages[from];
if (!stage) throw new Error(`unknown stage: ${from}`);

module.exports = {
  local: { root: __dirname, env: "development" },
  remote: {
    host: stage.host,
    user: stage.user,
    port: 22,
    appPath: "/srv/app",
  },
  database: { engine: "postgres", reset: "drop-create" },
  files: [],
};
```

Then run `cloner run --from staging`.

## 4. Before/after examples

### Minimal PostgreSQL project

Legacy:

```ruby
class Dl < Cloner::Base
  no_commands do
    def ssh_host = "example.com"
    def ssh_user = "deploy"
    def remote_app_path = "/srv/app"
    def remote_dump_path = "/srv/app/tmp_dump"
    def remote_docker_compose? = true
    def remote_docker_compose_service = "postgres"
    def local_docker_compose? = true
    def local_docker_compose_service = "db"
    def local_docker_compose_path = project_root.join("dev-docker").to_s
    def reset_development_database
      # dropdb/createdb
    end
  end

  def download
    pg_dump_remote
    pg_dump_copy
    reset_development_database
    pg_dump_restore
  end
end
```

New:

```js
module.exports = {
  local: {
    root: __dirname,
    env: "development",
    compose: { path: "dev-docker", file: "compose.yml", service: "db" },
  },
  remote: {
    host: "example.com",
    user: "deploy",
    appPath: "/srv/app",
    compose: { file: "compose.yml", service: "postgres" },
  },
  database: {
    engine: "postgres",
    reset: "drop-create",
    restoreArgs: ["--no-owner", "-Fc", "-c", "--if-exists"],
  },
  files: [
    { from: "public/uploads", to: "public/uploads" },
  ],
};
```

### Volume reset plus post-processing (`rtrack-go`)

Legacy `download` did a volume reset, `clone_db`, a Go migration, four SQL
updates and a reindex.

New:

```js
module.exports = {
  local: {
    root: __dirname,
    env: "development",
    compose: { path: "dev-docker", file: "compose.yml", service: "db" },
  },
  remote: {
    host: "rtrack.ru",
    user: "root",
    port: 63511,
    appPath: "/root/compose/rtrack",
    compose: { file: "compose.yml", service: "rtdb" },
  },
  database: {
    engine: "postgres",
    reset: "volume",
    volume: "rt_postgres",
    restoreArgs: ["--no-owner", "-Fc", "-c", "--if-exists"],
  },
  files: [{ from: "uploads", to: "uploads" }],
  hooks: {
    afterRestore() {
      shell("go run main.go migrate");

      db.query("UPDATE oauth_apps SET domain='http://localhost:8065' where id = '1'");
      db.query("UPDATE oauth_apps SET domain='http://localhost:7321' where id = '2'");
      db.query("UPDATE oauth_apps SET domain='http://localhost:1220' where id = '3'");
      db.query("UPDATE oauth_apps SET domain='http://localhost:8610' where id = '4'");

      shell("go run main.go reindex");
    },
  },
};
```

The four `psql -c` calls become `db.query()`. `shell()` runs in `local.root`
and echoes output.

### Custom action

Legacy `download` that did not clone at all:

```js
module.exports = {
  // ... connection/config only if needed ...
  execute() {
    shell("bin/rails db:prepare");
    db.reset();
    shell("bin/rails db:seed");
  },
};
```

Call `clone_db()` and/or `clone_files()` inside `execute()` if the custom action
should still include the normal steps.

## 5. Behaviour differences to expect

- **No Ruby or Bundler.** Remove `require "cloner"`, the `Gemfile` entry and the
  `Dl < Cloner::Base` class. `cloner` is a single binary.
- **No Rails boot.** `Rails.root`/`Rails.env` do not exist. Use `__dirname` and
  `local.env`.
- **Hooks are JavaScript.** They run synchronously; returning a Promise is an
  error. Use `shell()`, `sh.local()`, `sh.remote()`, `db.query()`, `db.reset()`.
- **No `rsync` at runtime.** File sync is an SFTP-based incremental copy
  (size + mtime, atomic rename, permissions, times, symlinks, globs). It has no
  delta algorithm; very large repeatedly-synced trees may re-copy changed files
  whole.
- **Compose resolution replaces hand-written DB configs.** If auto-detection
  picks the wrong service or database, set `database.service` /
  `database.local` / `database.remote` explicitly.
- **Reset is explicit.** Nothing is dropped unless `database.reset` is set.
- **Hooks are not run from `execute()`.** Add them manually if needed.
- **Old internal helper overrides are not portable.** Overrides of
  `pg_remote_bin_path`, `pg_dump_remote` or similar have no direct config key.
  Use `database.dumpArgs`/`restoreArgs`, or run a custom command from a hook.
- **`verbose?` is now a CLI flag**, not a method: `cloner -v run`.

## 6. Validation checklist

```bash
# 1. Does the DSL parse and resolve the right services/databases?
cloner validate

# 2. Are the planned remote and local commands what you expect?
cloner run --dry-run

# 3. Does a non-destructive dump + file copy work?
cloner backup --to /tmp/cloner-check

# 4. Real clone
cloner run
```

Check in `validate` output:

- `projectRoot` and `projectEnv`
- `remote.host`, `remote.appPath`, `remote.dumpPath`
- `database.remote.database/user/service`
- `database.local.database/user/service`
- `files[].from/to`

Passwords are redacted in `validate` output.

## 7. Common conversion mistakes

| Symptom | Cause | Fix |
|---|---|---|
| `no DSL file found` | Running outside the project | `cd` to the project or pass `-c path/to/dl.js` |
| `cannot determine database engine` | No `database.engine`, no detectable Compose service | Set `database.engine` and `database.service` |
| wrong database restored | Explicit `local_db_config` was dropped without checking `.env` | Run `cloner validate`; add `database.local.database/user` |
| `remote host key` warning | Default permissive host-key policy | Set `remote.strictHostKeyChecking: true` |
| files not synced | Rsync calls were not converted | Add `files` entries; `files: []` means "no file step" |
| hooks never run | `execute()` replaces the pipeline | Call the code from `execute()` or remove `execute()` |
| volume reset stops other services | `reset: "volume"` restarts the whole Compose project | Intended behavior; use `drop-create` otherwise |
| `database ... does not exist` during reset | Local DB was never created | `cloner db create <owner>` first |
| `bundle exec thor dl` still used | Old workflow | Replace with `cloner run`; see [CLI usage](cli-usage.md) |

## 8. Final state

Keep the new file at the project root as `dl.js` (other accepted names are
`cloner.js`, `cloner.config.js`, `dl.config.js`, `.cloner.js`), remove
`dl.thor`, and update scripts, aliases and CI to call `cloner` instead of
`bundle exec thor dl`.
