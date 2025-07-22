#!/usr/bin/env ruby

require 'pathname'
require_relative '../lib/cloner'

# Mock Rails for testing
class ::Rails
  def self.root
    Pathname.new(File.expand_path("../", __FILE__))
  end
  def self.env
    "development"
  end
end

class TestDockerCompose < Cloner::Base
  no_commands do
    def rails_path
      File.expand_path("../", __FILE__)
    end

    def load_env
      require 'net/ssh'
      require 'shellwords'
      require 'active_support/core_ext/object'
    end

    # Test with mock SSH - no actual connection
    def ssh_host
      'localhost'
    end

    def ssh_user
      'test'
    end

    def env_from
      "production"
    end

    # Docker Compose configuration
    def use_docker_compose?
      true
    end

    def local_docker_compose?
      true
    end

    def remote_docker_compose?
      true
    end

    def local_docker_compose_service
      'testdb'
    end

    def remote_docker_compose_service
      'rtdb'
    end

    def local_docker_compose_path
      Rails.root.parent.to_s
    end

    def remote_app_path
      '/root/compose/rtrack'
    end

    def remote_dump_path
      "#{remote_app_path}/tmp_dump"
    end

    # Mock database configurations
    def read_ar_r_conf
      {
        adapter: "postgresql",
        host: "rtdb",
        database: "rtrack",
        username: "rtrack",
        password: "testpass"
      }.stringify_keys
    end

    def ar_conf
      {
        adapter: "postgresql",
        host: "localhost",
        database: "test_development",
        username: "testuser",
        password: "testpass"
      }.stringify_keys
    end

    def ar_to
      ar_conf['database']
    end

    def verbose?
      true
    end

    # Override SSH to test without actual connection
    def do_ssh(&block)
      puts "MOCK SSH: Would connect to #{ssh_user}@#{ssh_host}"
      # Create mock SSH session
      mock_ssh = Object.new
      def mock_ssh.exec!(cmd)
        puts "MOCK SSH EXEC: #{cmd}"
        # Return mock responses
        if cmd.include?('.env')
          return "DB_HOST=rtdb\nDB_PORT=5432\nDB_NAME=rtrack\nDB_USER=rtrack\nDB_PASSWORD=testpass\n"
        elsif cmd.include?('mkdir')
          return ""
        elsif cmd.include?('pg_dump')
          return ""
        end
        ""
      end
      def mock_ssh.open_channel(&block)
        mock_channel = Object.new
        def mock_channel.exec(cmd, &cb)
          puts "MOCK CHANNEL EXEC: #{cmd}"
        end
        def mock_channel.on_data(&cb)
          cb.call(nil, "")
        end
        def mock_channel.on_extended_data(&cb)
        end
        def mock_channel.on_request(type, &cb)
          if type == "exit-status"
            data = Object.new
            def data.read_long
              0
            end
            cb.call(nil, data)
          end
        end
        block.call(mock_channel)
      end
      def mock_ssh.loop
      end
      def mock_ssh.scp
        mock_scp = Object.new
        def mock_scp.download!(from, to, opts = {})
          puts "MOCK SCP: Download #{from} to #{to}"
        end
        mock_scp
      end
      block.call(mock_ssh)
    end
    
    # Override ssh_exec! to simplify testing
    def ssh_exec!(ssh, cmd)
      puts "MOCK SSH EXEC!: #{cmd}"
      # Return mock .env content when requested
      if cmd.include?('.env')
        env_content = "DB_HOST=rtdb\nDB_PORT=5432\nDB_NAME=rtrack\nDB_USER=rtrack\nDB_PASSWORD=testpass\n"
        [0, env_content]
      else
        [0, ""]
      end
    end
    
    # Override check_ssh_err for testing
    def check_ssh_err(ret)
      # Do nothing in test
    end

    # Override rsync for testing
    def rsync(from, to)
      puts "MOCK RSYNC: #{from} -> #{to}"
      # Create a dummy backup file for testing
      FileUtils.mkdir_p(pg_path)
      File.write("#{pg_path}/#{db_file_name}.bak", "MOCK BACKUP DATA")
    end
  end

  desc "test_compose", "Test Docker Compose functionality"
  def test_compose
    load_env
    
    puts "=== Testing Docker Compose Integration ==="
    puts "\nConfiguration:"
    puts "- Local Docker Compose: #{local_docker_compose?}"
    puts "- Remote Docker Compose: #{remote_docker_compose?}"
    puts "- Local Service: #{local_docker_compose_service}"
    puts "- Remote Service: #{remote_docker_compose_service}"
    
    puts "\n=== Testing pg_bin_path methods ==="
    puts "Local pg_dump command:"
    puts pg_local_bin_path('pg_dump')
    
    puts "\nRemote pg_dump command:"
    puts pg_remote_bin_path('pg_dump')
    
    puts "\n=== Testing env var reading ==="
    puts "Remote DB Config:"
    puts remote_db_config.inspect
    
    puts "\nLocal DB Config:"
    puts local_db_config.inspect
    
    puts "\n=== Simulating database clone ==="
    clone_db
    
    puts "\n=== Test completed successfully! ==="
  end
end

if __FILE__ == $0
  TestDockerCompose.start(['test_compose'])
end