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
    "docker-compose.yml"
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
end