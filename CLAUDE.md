# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## About Cloner

Cloner is a Ruby gem that simplifies cloning production databases and files to local development or staging environments. It supports MongoDB (via Mongoid), PostgreSQL, and MySQL databases, and uses rsync for file synchronization.

## Key Commands

```bash
# Install dependencies
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

## Architecture Overview

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