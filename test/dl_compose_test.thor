require_relative '../lib/cloner'
require 'pathname'
require 'net/ssh'
require 'shellwords'
require 'active_support/core_ext/object'

class ::Rails
  def self.root
    Pathname.new(File.expand_path("../", __FILE__))
  end
  def self.env
    "development"
  end
end

class DlComposeTest < Cloner::Base
  no_commands do
    def rails_path
      File.expand_path("../", __FILE__)
    end

    def load_env
      require 'net/ssh'
      require 'shellwords'
      require 'active_support/core_ext/object'
    end

    # SSH configuration
    def ssh_host
      'rscx.ru'
    end

    def ssh_user
      'root'
    end

    def env_from
      "production"
    end

    # Remote paths
    def remote_dump_path
      '/root/compose/aichaos/tmp_dump'
    end

    def remote_app_path
      '/root/compose/aichaos'
    end

    # Docker Compose configuration for remote
    def remote_docker_compose?
      true
    end

    def remote_docker_compose_service
      'db'  # Assuming the PostgreSQL service is named 'db'
    end

    def remote_docker_compose_path
      remote_app_path
    end

    # Docker Compose configuration for local
    def local_docker_compose?
      true
    end

    def local_docker_compose_service
      'testdb'
    end

    def local_docker_compose_path
      Rails.root.parent.to_s
    end

    # Override to read credentials from remote .env file
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
        # Remove quotes if present
        value = value.gsub(/^["']|["']$/, '')
        env_vars[key] = value
      end
      
      {
        adapter: "postgresql",
        host: env_vars['DB_HOST'] || 'db',
        port: env_vars['DB_PORT'] || '5432',
        database: env_vars['DB_NAME'] || 'aichaos',
        username: env_vars['DB_USER'] || 'aichaos',
        password: env_vars['DB_PASSWORD'] || ''
      }.stringify_keys
    end

    # Local database configuration
    def ar_conf
      {
        adapter: "postgresql",
        host: "localhost",
        port: "5432",
        database: "test_development",
        username: "testuser",
        password: "testpass"
      }.stringify_keys
    end

    def ar_to
      ar_conf['database']
    end

    def db_file_name
      "cloner_test"
    end

    # Override pg_bin_path to handle Docker Compose
    def pg_remote_bin_path(util)
      if remote_docker_compose? && remote_docker_compose_service
        env_vars = read_ar_r_conf
        "cd #{e remote_app_path} && docker compose exec --no-TTY --env PGPASSWORD='#{env_vars['password']}' #{e remote_docker_compose_service} #{util}"
      else
        super
      end
    end

    def verbose?
      true
    end
  end

  desc "test", "Test cloning from remote Docker Compose PostgreSQL"
  def test
    load_env
    
    puts "Testing Docker Compose PostgreSQL cloning..."
    puts "Remote: #{ssh_user}@#{ssh_host}:#{remote_app_path}"
    puts "Local: #{local_docker_compose_path}"
    
    # Start local Docker Compose
    puts "\nStarting local Docker Compose..."
    system("cd #{e local_docker_compose_path} && docker compose up -d")
    sleep 5  # Wait for database to be ready
    
    # Clone the database
    clone_db
    
    puts "\nTest completed!"
  end
end