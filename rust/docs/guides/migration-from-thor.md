# Migrating from `dl.thor`

> This is the short overview. For the complete conversion procedure, including
> stages, helper methods, real-world examples and a checklist, see
> [Converting a legacy `dl.thor` to `dl.js`](thor-to-js-conversion.md).
> For command-line changes see [CLI usage after `thor dl`](cli-usage.md).

The old `dl.thor` files subclassed `Cloner::Base` and overrode Ruby methods.
The Rust version uses a JavaScript object. Most overrides become plain config
keys; custom Ruby methods become hooks or an `execute()` action.

## Method mapping

| `dl.thor` | `dl.js` |
|---|---|
| `ssh_host` | `remote.host` |
| `ssh_user` | `remote.user` |
| `ssh_opts { port: 63511 }` | `remote.port: 63511` |
| `remote_app_path` | `remote.appPath` |
| `remote_dump_path` | `database.remoteDumpPath` (or `remote.dumpPath`) |
| `remote_docker_compose? true` | `remote.compose: { ... }` |
| `remote_docker_compose_service` | `remote.compose.service` or `database.remoteService` |
| `remote_docker_compose_path` | `remote.compose.path` |
| `remote_docker_compose_file` | `remote.compose.file` |
| `local_docker_compose? true` | `local.compose: { ... }` |
| `local_docker_compose_service` | `local.compose.service` or `database.localService` |
| `local_docker_compose_path` | `local.compose.path` |
| `local_docker_compose_file` | `local.compose.file` |
| `project_root` / `Rails.root` | `local.root: __dirname` |
| `project_env` / `Rails.env` | `local.env: "development"` |
| `local_db_config` / `remote_db_config` | Compose/env auto-detection, or `database.local` / `database.remote` |
| `pg_dump_param` / `pg_dump_extra` | `database.dumpArgs` |
| `pg_restore_param` | `database.restoreArgs` |
| `my_dump_param` / `my_restore_param` | `database.dumpArgs` / `database.restoreArgs` |
| `clone_db` | `clone_db()` |
| `rsync_public("uploads")` | `files: [{ from: "public/uploads", to: "public/uploads" }]` |
| `rsync(from, to)` | `files` entry or `sync(from, to)` |
| custom `reset_development_database` | `database.reset: "drop-create"`, `"volume"`, or `hooks.beforeRestore()` |
| custom post-restore commands | `hooks.afterRestore()` |
| custom `download` action | `execute() { ... }` |

## Example: chaos-router

Old:

```ruby
class Dl < Cloner::Base
  no_commands do
    def ssh_host = "rsxx.ru"
    def ssh_user = "root"
    def ssh_opts = { port: 63511 }
    def remote_app_path = "/compose/aichaos"
    def remote_dump_path = "/compose/aichaos/tmp_dump"
    def remote_docker_compose? = true
    def remote_docker_compose_service = "postgres"
    def local_docker_compose? = true
    def local_docker_compose_service = "db"
    def local_docker_compose_path = project_root.join("dev-docker").to_s
  end

  def download
    load_env
    pg_dump_remote
    pg_dump_copy
    reset_development_database
    pg_dump_restore
  end
end
```

New `dl.js`:

```js
module.exports = {
  local: {
    root: __dirname,
    env: "development",
    compose: { path: "dev-docker", file: "compose.yml", service: "db" },
  },
  remote: {
    host: "rsxx.ru",
    user: "root",
    port: 63511,
    appPath: "/compose/aichaos",
    compose: { file: "compose.yml", service: "postgres" },
  },
  database: {
    engine: "postgres",
    reset: "drop-create",
    restoreArgs: ["--no-owner", "-Fc", "-c", "--if-exists"],
  },
  files: [],
};
```

## Example: rtrack-go

Old `dl.thor` did a volume reset, restored the DB, ran `go run main.go migrate`,
updated OAuth domains, and ran `go run main.go reindex`.

New `dl.js`:

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

## Replacing the action completely

If the old `download` method did something unrelated to cloning, use
`execute()`:

```js
module.exports = {
  // ...
  execute() {
    shell("bin/rails db:prepare");
    db.reset();
    shell("bin/rails db:seed");
  },
};
```

`execute()` replaces the default `clone_db()` + `clone_files()` pipeline
entirely. Call `clone_db()` / `clone_files()` inside it if you still want the
built-in steps.

## Transition plan

1. Keep `dl.thor` for a short while and add `dl.js` next to it.
2. Run `cloner validate` until the resolved DB and file targets are correct.
3. Run `cloner run --dry-run`.
4. Run `cloner backup --to /tmp/cloner-check` for a non-destructive check.
5. Run `cloner run` and verify the local app.
6. Remove `dl.thor` and the Ruby `cloner` gem from the project.
