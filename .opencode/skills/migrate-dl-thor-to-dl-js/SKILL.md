---
name: migrate-dl-thor-to-dl-js
description: >
  Convert a legacy Ruby/Thor cloner config (dl.thor, `bundle exec thor dl`)
  into the Rust cloner JavaScript DSL (dl.js). Use when a user asks to migrate,
  port, rewrite or modernize dl.thor, replace `bundle exec thor dl`, set up
  dl.js for an existing cloner project, or debug a conversion where the wrong
  database/files are cloned.
---

# Migrate `dl.thor` → `dl.js`

Full reference: [`rust/docs/guides/thor-to-js-conversion.md`](../../../rust/docs/guides/thor-to-js-conversion.md).
Also read [`dsl.md`](../../../rust/docs/guides/dsl.md) and
[`cli-usage.md`](../../../rust/docs/guides/cli-usage.md) before editing.

## Workflow

1. **Read the old file completely.** Note every method in `no_commands`, the
   `download` body, `option :from`, and any `Rails` shim or helper methods.
2. **Generate `dl.js` next to `dl.thor`.** Do not delete `dl.thor` until the new
   config is validated.
3. **Convert config first**, then hooks, then `execute()` if needed.
4. **Validate without touching anything:**
   ```bash
   cloner validate
   cloner run --dry-run
   ```
5. **Verify a real clone**, then remove `dl.thor` and the Ruby gem reference.

## Conversion map

| Ruby / Thor | JavaScript |
|---|---|
| `ssh_host` | `remote.host` |
| `ssh_user` | `remote.user` |
| `ssh_opts { port: N }` | `remote.port: N` |
| `remote_app_path` | `remote.appPath` |
| `remote_dump_path` | `database.remoteDumpPath` or `remote.dumpPath` |
| `remote_docker_compose?` / `_service` / `_path` / `_file` | `remote.compose: { path, file, service }` |
| `local_docker_compose?` / `_service` / `_path` / `_file` | `local.compose: { path, file, service }` |
| `project_root` / `Rails.root` | `local.root: __dirname` |
| `project_env` / `Rails.env` | `local.env: "development"` |
| `local_db_config` / `remote_db_config` | delete; Compose/`.env` auto-detection, or `database.local` / `database.remote` |
| `read_local_env(k)` / `read_remote_env(k)` | `env.local(k)` / `env.remote(k)` |
| `load_env` | delete; `.env` files are loaded automatically |
| `pg_dump_param`, `pg_dump_extra` | `database.dumpArgs: [...]` |
| `pg_restore_param` | `database.restoreArgs: [...]` |
| `my_dump_param`, `my_restore_param` | `database.dumpArgs` / `database.restoreArgs` |
| `reset_development_database` | `database.reset: "drop-create"` |
| `reset_db_volume` | `database.reset: "volume"`, `database.volume: "<name>"` |
| `rsync_public("uploads")` | `files: [{ from: "public/uploads", to: "public/uploads" }]` |
| `rsync(from, to)` | `files` entry or `sync(from, to)` |
| `clone_db` | remove (default), or `clone_db()` in `execute()` |
| `pg_dump_remote` / `pg_dump_copy` / `pg_dump_restore` | remove; built into `run` |
| post-restore shell/SQL | `hooks.afterRestore()` with `shell()` / `db.query()` |
| custom `download` body | `execute() { ... }` |
| `verbose?` | CLI flag `-v` |
| stages hash + `options[:from]` | `process.env.CLONE_FROM` selected by `--from` |

## Skeleton

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
    engine: "postgres", // postgres | mysql | mongo
    reset: "drop-create", // none | drop-create | volume
    restoreArgs: ["--no-owner", "-Fc", "-c", "--if-exists"],
  },
  files: [{ from: "uploads", to: "uploads" }],
  hooks: {
    afterRestore() {
      shell("bin/rails db:migrate");
      db.query("UPDATE settings SET host = 'http://localhost:3000'");
    },
  },
};
```

For an unusual old `download`, replace the pipeline:

```js
execute() {
  shell("...");
  // clone_db();
  // clone_files();
}
```

`execute()` replaces the default `clone_db()` + `clone_files()` action, and
lifecycle hooks do **not** run automatically inside it.

## Rules

- Use `local.root: __dirname`; do not port `Rails.root`.
- Trust Compose/`.env` resolution before adding `database.remote/local`.
  Add explicit fields only when `cloner validate` shows they are wrong.
- Use `database.engine` and `database.service` when auto-detection is ambiguous.
- `database.reset` is opt-in. Never add a destructive reset just because the old
  file had one unless the behavior is still wanted.
- Port shell commands to `shell("...")` (runs in `local.root`, echoes output) or
  `sh.local(...)` (returns stdout). Remote: `shell.remote(...)`.
- Port `psql -c` / `mysql -e` post-processing to `db.query(sql)`.
- Keep hooks synchronous. Returned Promises are rejected.
- File sync is SFTP-based, not the `rsync` binary. Map paths into `files`.
- Do not invent config keys; check `dsl.md`.

## Validation checklist

```bash
cloner validate        # services, DB names, users, paths; passwords redacted
cloner run --dry-run   # planned commands
cloner backup --to /tmp/cloner-check
cloner run             # real clone
```

After validation, remove `dl.thor` and update scripts from
`bundle exec thor dl` to `cloner` / `cloner run`.

## Common failures

| Failure | Fix |
|---|---|
| `cannot determine database engine` | set `database.engine` + `database.service` |
| wrong DB restored | inspect `cloner validate`; set `database.local.database/user` |
| no files copied | add `files` entries; `files: []` disables the step |
| hook code never runs | it is inside `hooks` or remove `execute()` |
| `database ... does not exist` | run `cloner db create <owner>` first |
| volume reset restarts everything | expected for `reset: "volume"`; use `drop-create` |
