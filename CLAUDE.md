# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## About Cloner

Cloner clones production databases and files into local development or staging
environments. It supports MongoDB, PostgreSQL, and MySQL.

The main implementation is the Rust single binary at the repository root. It
uses a JavaScript DSL (`dl.js`), SSH/SFTP file sync, Compose/`.env` resolution,
backup mode, and `pg_dev`-style `cloner db create/reset/drop/migrate/seed`
commands. Build and test it with:

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

On Windows use `install.ps1` (installs Build Tools/Rust if needed and copies
`cloner.exe` to `%USERPROFILE%\.local\bin`); the same cargo commands apply and
the binary is `target\release\cloner.exe`.

The legacy Ruby/Thor gem remains supported in the same repository (`lib/`,
`test/`, `cloner.gemspec`, `Gemfile`) and is documented in
[`docs/ruby-readme.md`](docs/ruby-readme.md). Its `dl.thor` syntax maps closely
to the Rust `dl.js` DSL.

Rust user docs: `docs/guides/installer.md`, `dsl.md`, `cli.md`, `cli-usage.md`,
`operations.md`, `thor-to-js-conversion.md`, `migration-from-thor.md`. The
`migrate-dl-thor-to-dl-js` skill lives at
`.opencode/skills/migrate-dl-thor-to-dl-js/SKILL.md`.

## Key Commands

```bash
# Main (Rust)
cloner init
cloner validate
cloner run
cloner backup --to /var/backups/myapp

# Legacy Ruby/Thor: install dependencies
bundle install

# Generate cloner template (basic)
bundle exec rails generate cloner

# Generate extended template with more options
bundle exec rails generate cloner -e

# Run cloning (after generating template)
thor dl

# Clone only database (skip files)
thor dl -F

# Clone only files (skip database)
thor dl -D
```

## Legacy Ruby Architecture Overview

The gem is organized as a modular Thor-based CLI tool:

1. **Core Module Structure** (`lib/cloner/`):
   - `base.rb`: Thor command base class that users extend
   - `internal.rb`: Core functionality shared across all adapters
   - Database adapters: `mongodb.rb`, `mysql.rb`, `postgres.rb`, `ar.rb`
   - `rsync.rb`: File synchronization logic
   - `ssh.rb`: SSH connection handling

2. **Rails Integration**:
   - Generator in `lib/generators/cloner_generator.rb` creates `lib/tasks/dl.thor`
   - Templates in `lib/generators/templates/` provide base and extended configurations

3. **Key Design Patterns**:
   - Uses Thor for CLI interface
   - Modular adapters for different database types
   - Override methods for customization (e.g., `ssh_host`, `ssh_user`, `remote_dump_path`)
   - Leverages native database tools (pg_dump, mysqldump, mongodump)

## Development Notes

- The gem version is defined in `lib/cloner/version.rb`
- Uses `fileutils` for file operations (ensure compatibility)
- Supports Rails 6+ multi-database configurations
- SSH connections use net-ssh gem
- Database detection automatically chooses appropriate adapter based on Rails configuration