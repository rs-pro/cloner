# Cloner

Easily clone your production Mongoid database and files for local development

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

create ```lib/tasks/dl.thor``` with following content:

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
        def ssh_opts
          {}
        end
        def remote_db_user
          'gleb'
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

Adjust it to your project and deployment.

Run it:

    thor dl

## Additional

All functions from cloner/internal.rb can be overriden, for example:

    def env_from
      'production'
    end
    def ssh_opts
      {}
    end

## Contributing

1. Fork it ( https://github.com/[my-github-username]/cloner/fork )
2. Create your feature branch (`git checkout -b my-new-feature`)
3. Commit your changes (`git commit -am 'Add some feature'`)
4. Push to the branch (`git push origin my-new-feature`)
5. Create a new Pull Request
