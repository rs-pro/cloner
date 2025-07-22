module Cloner::DockerCompose
  extend ActiveSupport::Concern

  # Docker Compose configuration methods
  def use_docker_compose?
    false
  end

  def local_docker_compose?
    false
  end

  def remote_docker_compose?
    false
  end

  def docker_compose_file
    "compose.yml"
  end

  def local_docker_compose_file
    docker_compose_file
  end

  def remote_docker_compose_file
    docker_compose_file
  end

  def docker_compose_service
    nil
  end

  def local_docker_compose_service
    docker_compose_service
  end

  def remote_docker_compose_service
    docker_compose_service
  end

  def docker_compose_path
    "."
  end

  def local_docker_compose_path
    docker_compose_path
  end

  def remote_docker_compose_path
    remote_app_path
  end

  def docker_compose_exec(service, command, opts = {})
    compose_file = opts[:compose_file] || docker_compose_file
    compose_path = opts[:compose_path] || docker_compose_path
    no_tty = opts[:no_tty] != false ? "--no-TTY" : ""
    env_vars = opts[:env] || {}
    
    env_str = env_vars.map { |k, v| "--env #{e k}=#{e v}" }.join(" ")
    
    "cd #{e compose_path} && docker compose -f #{e compose_file} exec #{no_tty} #{env_str} #{e service} #{command}"
  end

  def local_docker_compose_exec(service, command, opts = {})
    opts[:compose_file] ||= local_docker_compose_file
    opts[:compose_path] ||= local_docker_compose_path
    docker_compose_exec(service, command, opts)
  end

  def remote_docker_compose_exec(service, command, opts = {})
    opts[:compose_file] ||= remote_docker_compose_file
    opts[:compose_path] ||= remote_docker_compose_path
    docker_compose_exec(service, command, opts)
  end

  # Helper to wrap commands with docker compose when needed
  def wrap_command(command, local: true)
    if local && local_docker_compose?
      service = local_docker_compose_service
      return local_docker_compose_exec(service, command) if service
    elsif !local && remote_docker_compose?
      service = remote_docker_compose_service
      return remote_docker_compose_exec(service, command) if service
    end
    command
  end
  
  # Helper to read and parse remote .env file
  def remote_env_content
    @remote_env_content ||= begin
      content = ""
      do_ssh do |ssh|
        # Use docker compose path, not app path
        env_path = "#{e remote_docker_compose_path}/.env"
        ret = ssh_exec!(ssh, "test -f #{env_path} && cat #{env_path} || echo ''")
        # ssh_exec! returns [exit_code, output]
        content = ret[1] if ret && ret[1]
      end
      content || ""
    end
  end
  
  # Parse .env content into a hash
  def remote_env_vars
    @remote_env_vars ||= begin
      vars = {}
      remote_env_content.each_line do |line|
        line = line.strip
        next if line.empty? || line.start_with?('#')
        
        # Handle KEY=VALUE format
        if match = line.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/)
          key = match[1]
          value = match[2]
          
          # Remove surrounding quotes if present
          value = value.gsub(/^["']|["']$/, '') if value
          
          vars[key] = value
        end
      end
      vars
    end
  end
  
  # Helper to read a specific env var from remote .env file
  def read_remote_env(key)
    remote_env_vars[key]
  end
  
  # Helper to read and parse local .env file
  def local_env_content
    @local_env_content ||= begin
      # Use docker compose path for .env file
      env_path = File.join(local_docker_compose_path, '.env')
      if File.exist?(env_path)
        File.read(env_path)
      else
        ""
      end
    end
  end
  
  # Parse local .env content into a hash
  def local_env_vars
    @local_env_vars ||= begin
      vars = {}
      local_env_content.each_line do |line|
        line = line.strip
        next if line.empty? || line.start_with?('#')
        
        # Handle KEY=VALUE format
        if match = line.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/)
          key = match[1]
          value = match[2]
          
          # Remove surrounding quotes if present
          value = value.gsub(/^["']|["']$/, '') if value
          
          vars[key] = value
        end
      end
      vars
    end
  end
  
  # Helper to read a specific env var from local .env file
  def read_local_env(key)
    local_env_vars[key]
  end
  
  # Default local database config for Docker Compose
  # Override this method to customize your database configuration
  def local_db_config
    if local_docker_compose?
      # When using Docker Compose, read from .env file
      config = {
        adapter: 'postgresql',
        host: read_local_env('DB_HOST') || 'localhost',
        port: read_local_env('DB_PORT') || '5432',
        database: read_local_env('DB_NAME'),
        username: read_local_env('DB_USER'),
        password: read_local_env('DB_PASSWORD') || ''
      }.stringify_keys
      
      # Validate required fields
      if config['database'].nil? || config['database'].empty?
        puts "Error: DB_NAME not found in local .env file at #{local_docker_compose_path}/.env"
        puts "Available env vars: #{local_env_vars.keys.join(', ')}"
        puts "Local .env content (first 10 lines):"
        puts local_env_content.lines.first(10).join
        exit 1
      end
      
      if config['username'].nil? || config['username'].empty?
        puts "Error: DB_USER not found in local .env file at #{local_docker_compose_path}/.env"
        puts "Available env vars: #{local_env_vars.keys.join(', ')}"
        exit 1
      end
      
      config
    else
      # Fall back to reading from database.yml
      ar_conf
    end
  end
  
  # Default remote database config for Docker Compose
  # Override this method to customize your database configuration
  def remote_db_config
    if remote_docker_compose?
      # When using Docker Compose, read from .env file
      config = {
        adapter: 'postgresql',
        host: read_remote_env('DB_HOST') || 'db',
        port: read_remote_env('DB_PORT') || '5432',
        database: read_remote_env('DB_NAME'),
        username: read_remote_env('DB_USER'),
        password: read_remote_env('DB_PASSWORD') || ''
      }.stringify_keys
      
      # Validate required fields
      if config['database'].nil? || config['database'].empty?
        puts "Error: DB_NAME not found in remote .env file at #{remote_docker_compose_path}/.env"
        puts "Available env vars: #{remote_env_vars.keys.join(', ')}"
        puts "Remote .env content (first 10 lines):"
        puts remote_env_content.lines.first(10).join
        exit 1
      end
      
      if config['username'].nil? || config['username'].empty?
        puts "Error: DB_USER not found in remote .env file at #{remote_docker_compose_path}/.env"
        puts "Available env vars: #{remote_env_vars.keys.join(', ')}"
        exit 1
      end
      
      config
    else
      # Fall back to reading from database.yml
      ar_r_conf
    end
  end
end