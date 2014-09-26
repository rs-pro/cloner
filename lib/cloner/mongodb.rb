module Cloner::MongoDB
  extend ActiveSupport::Concern

  def mongodb_conf
    @conf ||= begin
      YAML.load_file(Rails.root.join('config', 'mongoid.yml'))[Rails.env]['sessions']['default']
    end
  end

  def mongodb_r_conf
    @r_conf ||= begin
      Net::SSH.start(ssh_host, ssh_user, ssh_opts) do |ssh|
        ret = ssh_exec!(ssh, "cat #{remote_app_path}/config/mongoid.yml")
        check_ssh_err(ret)
        YAML.load(ret[0])[env_from]['sessions']['default']
      end
    end
  end

  def mongodb_local_auth
    if mongodb_conf['password'].nil?
      ""
    else
      "-u #{mongodb_conf['username']} -p #{mongodb_conf['password']}"
    end
  end

  def mongodb_dump_remote
    puts "backup remote DB via ssh"
    Net::SSH.start(ssh_host, ssh_user, ssh_opts) do |ssh|
      ssh.exec!("rm -R #{remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{remote_dump_path}")
      check_ssh_err(ret)
      dump = "mongodump -u #{mongodb_r_conf['username']} -p #{mongodb_r_conf['password']} -d #{mongodb_r_conf['database']} --authenticationDatabase #{mongodb_r_conf['database']} -o #{remote_dump_path}"
      puts dump if verbose?
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def mongodb_dump_restore
    puts "restoring DB"
    restore = "mongorestore --drop -d #{mongodb_to} #{mongodb_local_auth} #{mongodb_path}"
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

  def mongodb_path
    Rails.root.join("tmp", "dump", db_to).to_s
  end

  def mongodb_to
    conf['database']
  end

  def mongodb_dump_copy
    `mkdir -p #{db_path}`
    rsync("#{remote_dump_path}/#{r_conf['database']}", db_path)
  end

  def clone_mongodb
    mongodb_dump_remote()
    mongodb_dump_copy()
    mongodb_dump_restore()
  end
end
