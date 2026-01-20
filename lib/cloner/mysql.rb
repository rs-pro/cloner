module Cloner::MySQL
  extend ActiveSupport::Concern
  
  def my_local_auth
    if local_db_config['password'].blank?
      ""
    else
      "--password='#{local_db_config['password']}'"
    end
  end

  def my_remote_auth
    if remote_db_config['password'].blank?
      ""
    else
      "--password='#{remote_db_config['password']}'"
    end
  end

  def my_dump_param
    "--add-drop-table"
  end

  def my_restore_param
    ""
  end

  def my_bin_path(util)
    util
  end
  
  def my_local_bin_path(util)
    if local_docker_compose? && local_docker_compose_service
      # For mysql restore, we pipe data in, so we don't wrap it
      return util if util == "mysql"
      
      # Build docker compose exec command
      compose_cmd = local_docker_compose_exec(
        local_docker_compose_service, 
        util,
        no_tty: true
      )
      return compose_cmd
    end
    my_bin_path(util)
  end
  
  def my_remote_bin_path(util)
    if remote_docker_compose? && remote_docker_compose_service
      # Build docker compose exec command for remote
      compose_cmd = remote_docker_compose_exec(
        remote_docker_compose_service,
        util,
        no_tty: true
      )
      return compose_cmd
    end
    my_bin_path(util)
  end

  def my_dump_remote
    puts "backup remote DB via ssh"
    do_ssh do |ssh|
      ssh.exec!("rm -R #{e remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{e remote_dump_path}")
      check_ssh_err(ret)
      host = remote_db_config['host'].present? ? " --host #{e remote_db_config['host']}" : ""
      port = remote_db_config['port'].present? ? " --port #{e remote_db_config['port']}" : ""
      dump = "#{my_remote_bin_path 'mysqldump'} #{my_dump_param} --user #{e remote_db_config['username']} #{my_remote_auth}#{host}#{port} #{e remote_db_config['database']} > #{e(remote_dump_path + '/'+db_file_name+'.sql')}"
      puts dump if verbose?
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def my_dump_restore
    puts "restoring DB"
    
    if local_docker_compose? && local_docker_compose_service
      # Docker compose restore - pipe the SQL file to docker compose exec
      host = local_db_config['host'].present? ? " --host #{e local_db_config['host']}" : ""
      port = local_db_config['port'].present? ? " --port #{e local_db_config['port']}" : ""
      
      compose_path = local_docker_compose_path
      compose_file = local_docker_compose_file
      service = local_docker_compose_service
      
      # MySQL requires password to be passed differently in Docker
      restore = "cat #{e(my_path + '/'+db_file_name+'.sql')} | (cd #{e compose_path} && docker compose -f #{e compose_file} exec -T #{e service} mysql #{my_restore_param} --user #{e local_db_config['username']} #{my_local_auth}#{host}#{port} #{e ar_to})"
      puts restore if verbose?
      system(restore)
      ret = $?.to_i
    else
      # Standard restore
      host = local_db_config['host'].present? ? " --host #{e local_db_config['host']}" : ""
      port = local_db_config['port'].present? ? " --port #{e local_db_config['port']}" : ""
      restore = "#{my_local_bin_path 'mysql'} #{my_restore_param} --user #{e local_db_config['username']} #{my_local_auth}#{host}#{port} #{e ar_to} < #{e(my_path + '/'+db_file_name+'.sql')}"
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

  def my_path
    File.join(project_root.to_s, "tmp", "dump")
  end

  def my_dump_copy
    FileUtils.mkdir_p(my_path)
    `mkdir -p #{e my_path}`
    rsync(remote_dump_path, my_path)
  end

  def clone_my
    my_dump_remote()
    my_dump_copy()
    my_dump_restore()
  end
end

