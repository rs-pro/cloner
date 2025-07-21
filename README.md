# Cloner

Easily clone your production Mongoid or PostgreSQL / MySQL database and files for local development or staging area.

Uses rsync and database-specific default dump/restore tools (pg_dump/pg_restore, mysqldump/mysql, mongodump/mongorestore)


## Installation

Add this line to your application's Gemfile:

```ruby
gem 'cloner'
```

And then execute:

    $ bundle

Or install it yourself as:

    $ gem install cloner

## Usage

For generate cloner base template, run:

```
bundle exec rails generate cloner
```

This is create `lib/tasks/dl.thor` file with following content:
```ruby
require 'cloner'

class Dl < Cloner::Base
  no_commands do
    def rails_path
      File.expand_path("../../../config/environment", __FILE__)
    end
    def ssh_host
      'hottea.ru'
    end
    def ssh_user
      'tea'
    end
    def remote_dump_path
      '/data/tea/dump'
    end
    def remote_app_path
      "/data/tea/app/current"
    end
  end

  desc "download", "clone files and DB from production"
  def download
    load_env
    clone_db
    rsync_public("ckeditor_assets")
    rsync_public("uploads")
  end
end
```

Adjust it to your project and deployment.

Run it:

    thor dl


If you generate extended cloner template as: `rails g cloner -e`,
you can run `thor dl` with additional parameters, for example:
```
bundle exec thor dl -D # For skip clone database
bundle exec thor dl -F # For skip clone files
```

For details see help:
```
bundle exec thor help dl:download

Usage:
  thor dl:download

Options:
      [--from=FROM]                            # stage name where cloner get data
                                               # Default: production
  -D, [--skip-database], [--no-skip-database]  # skip clone database
  -F, [--skip-files], [--no-skip-files]        # skip clone files

clone files and DB from production
```

## Additional

All functions from cloner/internal.rb can be overriden, for example:


    def verbose?
      false
    end
    def env_from
      'production'
    end
    def ssh_opts
      {}
    end

## Docker Compose Support

Cloner now supports Docker Compose for both local and remote database operations. This is useful when your databases run inside Docker containers.

### Generating Docker Compose Template

To generate a template with Docker Compose examples:

```
bundle exec rails generate cloner -d
```

### Configuration

Add these methods to your `dl.thor` file to enable Docker Compose:

```ruby
# For remote Docker Compose
def remote_docker_compose?
  true
end

def remote_docker_compose_service
  'db'  # Your database service name in docker-compose.yml
end

def remote_docker_compose_path
  remote_app_path  # Path where docker-compose.yml is located
end

# For local Docker Compose
def local_docker_compose?
  true
end

def local_docker_compose_service
  'db'  # Your local database service name
end

def local_docker_compose_path
  Rails.root.to_s  # Path where your local docker-compose.yml is located
end
```

### PostgreSQL with Docker Compose Example

```ruby
class Dl < Cloner::Base
  no_commands do
    def ssh_host
      'production.example.com'
    end
    
    def ssh_user
      'deploy'
    end
    
    def remote_app_path
      '/home/deploy/myapp'
    end
    
    def remote_dump_path
      "#{remote_app_path}/tmp_dump"
    end
    
    # Enable Docker Compose for remote
    def remote_docker_compose?
      true
    end
    
    def remote_docker_compose_service
      'postgres'
    end
    
    # Override to read credentials from .env file
    def read_ar_r_conf
      # Read from remote .env file
      env_content = ""
      do_ssh do |ssh|
        env_content = ssh.exec!("cat #{e remote_app_path}/.env")
      end
      
      # Parse .env content
      env_vars = {}
      env_content.each_line do |line|
        next if line.strip.empty? || line.strip.start_with?('#')
        key, value = line.strip.split('=', 2)
        next unless key && value
        value = value.gsub(/^["']|["']$/, '')
        env_vars[key] = value
      end
      
      {
        adapter: "postgresql",
        host: env_vars['DB_HOST'] || 'postgres',
        database: env_vars['DB_NAME'],
        username: env_vars['DB_USER'],
        password: env_vars['DB_PASSWORD']
      }.stringify_keys
    end
    
    # Enable Docker Compose for local
    def local_docker_compose?
      true
    end
    
    def local_docker_compose_service
      'db'
    end
  end
  
  desc "download", "clone DB from production"
  def download
    load_env
    clone_db
  end
end
```

### How It Works

When Docker Compose is enabled:

1. **Remote operations**: Database dump commands are wrapped with `docker compose exec` on the remote server
2. **Local operations**: Database restore commands pipe data into `docker compose exec -T` locally
3. **Automatic command wrapping**: The gem automatically detects and wraps database commands appropriately

### Supported Databases

Docker Compose support is available for:
- PostgreSQL
- MySQL
- MongoDB

## Changelog

### 0.14.0

- Add Docker Compose support for local and remote database operations
- Add Docker Compose generator template with `-d` option
- Support automatic command wrapping for PostgreSQL, MySQL, and MongoDB when using Docker Compose
- Add helper methods for Docker Compose configuration

### 0.10.0

- Support rails 6 multi database activerecord apps via option

```
def multi_db?
  true
end
def clone_databases
  ["primary", "gis"]
  # nil - clone all databases
end
```

- Backwards incompatible change:

Changed default dump file name to cloner.bak in postgresql to make it same, and to allow to override it and multiple files.

### 0.9.0

- Add option to rsync to allow sync one file (thx @AnatolyShirykalov)
- Add env_database to allow overriding database env (thx @Yarroo)

### 0.8.0

- Change default rsync flags - -z to -zz to support newer versions of rsync
- Allow overriding rsync flags via ```rsync_flags``` and ```rsync_compression```

### 0.7.0

- Add thor file generators

### 0.6.0

- Support MySQL

## Contributing

1. Fork it ( https://github.com/[my-github-username]/cloner/fork )
2. Create your feature branch (`git checkout -b my-new-feature`)
3. Commit your changes (`git commit -am 'Add some feature'`)
4. Push to the branch (`git push origin my-new-feature`)
5. Create a new Pull Request
