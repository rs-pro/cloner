module Cloner::Postgres
  extend ActiveSupport::Concern

  def ar_conf
    @conf ||= begin
      YAML.load_file(Rails.root.join('config', 'database.yml'))[Rails.env]
    end
  end

  def ar_to
    ar_conf['database']
  end

  def ar_r_conf
    @ar_r_conf ||= begin
      do_ssh do |ssh|
        ret = ssh_exec!(ssh, "cat #{e(remote_app_path + '/config/database.yml')}")
        check_ssh_err(ret)
        begin
          res = YAML.load(ret[0])[env_from]
          raise 'no data' if res.blank?
          #res['host'] ||= '127.0.0.1'
        rescue Exception => e
          puts "unable to read remote database.yml for env #{env_from}."
          puts "Remote file contents:"
          puts ret[0]
        end
        res
      end
    end
  end

  def pg_local_auth
    if ar_conf['password'].nil?
      ""
    else
      "PGPASSWORD='#{ar_conf['password']}' "
    end
  end

  def pg_remote_auth
    if ar_r_conf['password'].nil?
      ""
    else
      "PGPASSWORD='#{ar_r_conf['password']}' "
    end
  end

  def pg_dump_remote
    puts "backup remote DB via ssh"
    do_ssh do |ssh|
      ssh.exec!("rm -R #{e remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{e remote_dump_path}")
      check_ssh_err(ret)
      host = ar_r_conf['host'].present? ? "-h #{e ar_r_conf['host']}" : ""
      dump = pg_remote_auth + "pg_dump -Fc -U #{e ar_r_conf['username']} #{host} #{e ar_r_conf['database']} > #{e(remote_dump_path + '/tmp.bak')}"
      puts dump if verbose?
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def pg_dump_restore
    puts "restoring DB"
    host = ar_conf['host'].present? ? "-h #{e ar_conf['host']}" : ""
    restore = pg_local_auth + "pg_restore --no-owner -Fc -c -U #{e ar_conf['username']} #{host} -d #{e ar_to} #{e(pg_path + '/tmp.bak')}"
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
