module Cloner::Postgres
  extend ActiveSupport::Concern

  def pg_local_auth
    if local_db_config['password'].blank?
      ""
    else
      "PGPASSWORD='#{local_db_config['password']}' "
    end
  end

  def pg_remote_auth
    if remote_db_config['password'].blank?
      ""
    else
      "PGPASSWORD='#{remote_db_config['password']}' "
    end
  end

  def pg_dump_extra
    ""
  end

  def pg_dump_param
    if pg_dump_extra != ""
      puts "WARN pg_dump_extra is deprecated, use def pg_dump_param; super + ' extra'"
    end
    "-Fc #{pg_dump_extra}"
  end

  def pg_restore_param
    "--no-owner -Fc -c"
  end
  
  def pg_bin_path(util)
    util
  end

  def pg_local_bin_path(util)
    if local_docker_compose? && local_docker_compose_service
      # For pg_restore, we pipe data in, so we don't wrap it in docker compose
      return util if util == "pg_restore"
      
      # Build docker compose exec command
      env_vars = {}
      env_vars['PGPASSWORD'] = local_db_config['password'] if local_db_config['password'].present?
      
      compose_cmd = local_docker_compose_exec(
        local_docker_compose_service, 
        "env PGPASSWORD='#{local_db_config['password']}' #{util}",
        env: env_vars,
        no_tty: true
      )
      return compose_cmd
    end
    pg_bin_path(util)
  end
  
  def pg_remote_bin_path(util)
    if remote_docker_compose? && remote_docker_compose_service
      # Build docker compose exec command for remote
      env_vars = {}
      env_vars['PGPASSWORD'] = remote_db_config['password'] if remote_db_config['password'].present?
      
      compose_cmd = remote_docker_compose_exec(
        remote_docker_compose_service,
        "env PGPASSWORD='#{remote_db_config['password']}' #{util}",
        env: env_vars,
        no_tty: true
      )
      return compose_cmd
    end
    pg_bin_path(util)
  end 

  def pg_dump_remote
    puts "backup remote DB via ssh"
    do_ssh do |ssh|
      ssh.exec!("rm -R #{e remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{e remote_dump_path}")
      check_ssh_err(ret)
      host = remote_db_config['host'].present? ? " -h #{e remote_db_config['host']}" : ""
      port = remote_db_config['port'].present? ? " -p #{e remote_db_config['port']}" : ""
      dump = pg_remote_auth + "#{pg_remote_bin_path 'pg_dump'} #{pg_dump_param} -U #{e remote_db_config['username']}#{host}#{port} #{e remote_db_config['database']} > #{e(remote_dump_path + '/'+db_file_name+'.bak')}"
      puts dump if verbose?
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def pg_dump_restore
    puts "restoring DB"
    
    if local_docker_compose? && local_docker_compose_service
      # Docker compose restore - pipe the backup file to docker compose exec
      host = local_db_config['host'].present? ? " -h #{e local_db_config['host']}" : ""
      port = local_db_config['port'].present? ? " -p #{e local_db_config['port']}" : ""
      
      env_str = local_db_config['password'].present? ? "--env PGPASSWORD=#{e local_db_config['password']}" : ""
      compose_path = local_docker_compose_path
      compose_file = local_docker_compose_file
      service = local_docker_compose_service
      
      restore = "cat #{e(pg_path + '/'+db_file_name+'.bak')} | (cd #{e compose_path} && docker compose -f #{e compose_file} exec -T #{env_str} #{e service} pg_restore #{pg_restore_param} -U #{e local_db_config['username']}#{host}#{port} -d #{e ar_to})"
      puts restore if verbose?
      system(restore)
      ret = $?.to_i
    else
      # Standard restore
      host = local_db_config['host'].present? ? " -h #{e local_db_config['host']}" : ""
      port = local_db_config['port'].present? ? " -p #{e local_db_config['port']}" : ""
      restore = pg_local_auth + "#{pg_local_bin_path 'pg_restore'} #{pg_restore_param} -U #{e local_db_config['username']}#{host}#{port} -d #{e ar_to} #{e(pg_path + '/'+db_file_name+'.bak')}"
      puts restore if verbose?
      pipe = IO.popen(restore)
      while (line = pipe.gets)
        print line if verbose?
      end
      ret = $?.to_i
    end
    
    if ret != 0
      puts "Error: local command exited with #{ret}"
    end
  end

  def pg_path
    File.join(project_root.to_s, "tmp", "dump")
  end

  def pg_dump_copy
    FileUtils.mkdir_p(pg_path)
    `mkdir -p #{e pg_path}`
    rsync(remote_dump_path, pg_path)
  end

  def clone_pg
    pg_dump_remote()
    pg_dump_copy()
    pg_dump_restore()
  end
end
