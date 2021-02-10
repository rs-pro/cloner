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

## Changelog

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
