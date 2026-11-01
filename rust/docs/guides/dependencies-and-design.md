# Dependencies and design notes

This document records the crate/platform choices and the licensing constraints
that shaped them.

## Chosen crates

| Area | Crate | License | Why |
|---|---|---|---|
| SSH | `russh` | Apache-2.0 | Pure-Rust SSH client, agent/key/password auth, async |
| SFTP | `russh-sftp` | MIT OR Apache-2.0 | File transfer over the same SSH session |
| DSL | `rquickjs` | MIT | Embedded QuickJS; full JavaScript, no Node.js runtime |
| CLI | `clap` | MIT OR Apache-2.0 | Derive-based subcommands and help |
| Async | `tokio` | MIT | Runtime used by `russh` |
| Config | `serde`, `serde_json`, `serde_yaml` | MIT OR Apache-2.0 | Config and Compose parsing |
| Globs | `globset` | MIT OR Apache-2.0 | Include/exclude patterns |
| Hashing | `sha2`, `hex` | MIT OR Apache-2.0 | Backup manifest checksums |
| Time | `chrono` | MIT OR Apache-2.0 | Timestamps and backup directory names |
| Shell quoting | `shell-words` | MIT OR Apache-2.0 | Safe command construction |

The release binary statically embeds QuickJS and the Rust crates. It has no
runtime dependency on Ruby, Node.js, `ssh`, `rsync`, `scp` or a system
JavaScript engine.

## Rejected: the local `/data/_swap/rsync` crate

`/data/_swap/rsync` (“oc-rsync”) is a capable pure-Rust rsync implementation,
but its workspace declares:

```text
license = "GPL-3.0-or-later"
```

Linking it into this binary would make the combined work GPL-3.0-or-later,
which conflicts with the permissive-license constraint for the new code. It was
therefore not used.

Permissive rsync-related alternatives reviewed:

| Crate | License | Assessment |
|---|---|---|
| `fast_rsync` | Apache-2.0 | Delta/signature algorithm only; not a full remote client |
| `rsyn` | Apache-2.0 | Wire-compatible client, but pre-alpha (2020) |
| `ripsync` | MIT OR Apache-2.0 | New, small adoption |
| `barqsync` | MIT OR Apache-2.0 | New, rsync-compatible, larger scope than needed |
| `sy` | MIT | New, alternative design |
| `librsync` bindings | LGPL / mixed | Native library dependency, not desired |

Instead, file sync is implemented directly on `russh-sftp`:

- recursive remote walk with SFTP metadata
- size + mtime comparison (rsync-like incremental behavior)
- temporary file + atomic rename on download
- permissions, modification times, directories and symlinks
- include/exclude globs, max size, optional delete

This loses rsync's delta algorithm, but keeps the binary permissive and
dependency-free. For large, repeatedly-synced trees a delta engine can be added
later behind a permissive crate or an external-`rsync` opt-in.

## DSL choice

| Option | License | Result |
|---|---|---|
| `rquickjs` (QuickJS) | MIT | **Chosen**: real JavaScript, hooks, async-free but rich, small embedded engine |
| `rhai` | MIT OR Apache-2.0 | Good pure-Rust scripting, but a separate language and less familiar syntax |
| `rune` | MIT OR Apache-2.0 | Good Rust-native scripting, smaller ecosystem |
| `kdl` / TOML / YAML | MIT | Declarative only; poor fit for hooks and custom actions |
| Lua (`mlua`) | MIT | Requires a Lua runtime/C library; JS was preferred |

`rquickjs` is built from the crates.io package (which vendors QuickJS-NG). The
local `/data/_js/rquickjs` checkout needs its QuickJS submodule initialized, so
the crates.io release is used for reproducible builds.

The DSL intentionally mirrors the old Thor style:

```js
hooks: {
  afterRestore() {
    shell("bin/rails db:migrate");
    db.query("UPDATE settings SET host = 'http://localhost:3000'");
  },
}
```

and supports a full action override:

```js
execute() {
  clone_db();
  clone_files();
}
```

## Database tools

Database dumps and restores intentionally use the native tools already present
in the database containers:

- PostgreSQL: `pg_dump`, `pg_restore`, `psql`
- MySQL/MariaDB: `mysqldump`/`mariadb-dump`, `mysql`/`mariadb`
- MongoDB: `mongodump`, `mongorestore`, `mongosh`

Reimplementing the PostgreSQL/MySQL/Mongo wire protocols and dump formats in
Rust would add large dependencies without improving the developer workflow. The
remote host already runs the database image, so the tools are available there.
Local Compose mode runs them inside the container; non-Compose mode expects the
client tools on the local machine.

## Backup direction

This tool's `backup` command produces plain dumps plus a manifest. It is not a
deduplicating repository format. If repository-style backups are needed later,
the existing RSBackup plan (Rustic / `rustic-core` + `rustic-backend`,
Apache-2.0 OR MIT) is the intended engine; it is not a dependency of this
cloner binary and does not affect the single-binary goal.
