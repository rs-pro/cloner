module Cloner::MySQL
  def my_local_auth
    if ar_conf['password'].blank?
      ""
    else
      "--password='#{ar_conf['password']}'"
    end
  end

  def my_remote_auth
    if ar_r_conf['password'].blank?
      ""
    else
      "--password='#{ar_r_conf['password']}'"
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

  def my_dump_remote
    puts "backup remote DB via ssh"
    do_ssh do |ssh|
      ssh.exec!("rm -R #{e remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{e remote_dump_path}")
      check_ssh_err(ret)
      host = ar_r_conf['host'].present? ? " --host #{e ar_r_conf['host']}" : ""
      port = ar_r_conf['port'].present? ? " --port #{e ar_r_conf['port']}" : ""
      dump = "#{my_bin_path 'mysqldump'} #{my_dump_param} --user #{e ar_r_conf['username']} #{my_remote_auth}#{host}#{port} #{e ar_r_conf['database']} > #{e(remote_dump_path + '/cloner.sql')}"
      puts dump if verbose?
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def my_dump_restore
    puts "restoring DB"
    host = ar_conf['host'].present? ? " --host #{e ar_conf['host']}" : ""
    port = ar_conf['port'].present? ? " --port #{e ar_conf['port']}" : ""
    restore = "#{my_bin_path 'mysql'} #{my_restore_param} --user #{e ar_conf['username']} #{my_local_auth}#{host}#{port} #{e ar_to} < #{e(my_path + '/cloner.sql')}"
    puts restore if verbose?
    pipe = IO.popen(restore)
    while (line = pipe.gets)
      print line if verbose?
    end
    ret = $?.to_i
    if ret != 0
      puts "Error: local command exited with #{ret}"
    end
  end

  def my_path
    Rails.root.join("tmp", "dump").to_s
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

