module Cloner::Postgres
  extend ActiveSupport::Concern

  def pg_local_auth
    if ar_conf['password'].blank?
      ""
    else
      "PGPASSWORD='#{ar_conf['password']}' "
    end
  end

  def pg_remote_auth
    if ar_r_conf['password'].blank?
      ""
    else
      "PGPASSWORD='#{ar_r_conf['password']}' "
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
    pg_bin_path(util)
  end
  
  def pg_remote_bin_path(util)
    pg_bin_path(util)
  end 

  def pg_dump_remote
    puts "backup remote DB via ssh"
    do_ssh do |ssh|
      ssh.exec!("rm -R #{e remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{e remote_dump_path}")
      check_ssh_err(ret)
      host = ar_r_conf['host'].present? ? " -h #{e ar_r_conf['host']}" : ""
      port = ar_r_conf['port'].present? ? " -p #{e ar_r_conf['port']}" : ""
      dump = pg_remote_auth + "#{pg_remote_bin_path 'pg_dump'} #{pg_dump_param} -U #{e ar_r_conf['username']}#{host}#{port} #{e ar_r_conf['database']} > #{e(remote_dump_path + '/tmp.bak')}"
      puts dump if verbose?
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def pg_dump_restore
    puts "restoring DB"
    host = ar_conf['host'].present? ? " -h #{e ar_conf['host']}" : ""
    port = ar_conf['port'].present? ? " -p #{e ar_conf['port']}" : ""
    restore = pg_local_auth + "#{pg_local_bin_path 'pg_restore'} #{pg_restore_param} -U #{e ar_conf['username']}#{host}#{port} -d #{e ar_to} #{e(pg_path + '/tmp.bak')}"
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

  def pg_path
    Rails.root.join("tmp", "dump").to_s
  end

  def pg_dump_copy
    FileUtils.mkdir_p(pg_path)
    `mkdir -p #{e pg_path}`
    rsync(remote_dump_path + '/', pg_path)
  end

  def clone_pg
    pg_dump_remote()
    pg_dump_copy()
    pg_dump_restore()
  end
end
