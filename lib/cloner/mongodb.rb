module Cloner::MongoDB
  extend ActiveSupport::Concern

  def mongodb_conf
    @conf ||= begin
      yml = YAML.load_file(Rails.root.join('config', 'mongoid.yml'))[Rails.env]
      if yml.key?('sessions')
        yml['sessions']['default']
      else
        yml['clients']['default']
      end
    end
  end

  def mongodb_to
    mongodb_conf['database']
  end

  def mongodb_r_conf
    @r_conf ||= begin
      Net::SSH.start(ssh_host, ssh_user, ssh_opts) do |ssh|
        ret = ssh_exec!(ssh, "cat #{e(remote_app_path + '/config/mongoid.yml')}")
        check_ssh_err(ret)
        yml = YAML.load(ret[0])[env_from]
        if yml.key?('sessions')
          yml['sessions']['default']
        else
          yml['clients']['default']
        end
      end
    end
  end

  def mongodb_local_auth
    if mongodb_conf['options'].present? && mongodb_conf['password'].present?
      "-u #{e mongodb_conf['options']['user']} -p #{e mongodb_conf['options']['password']}"
    elsif mongodb_conf['password'].present?
      "-u #{e mongodb_conf['username']} -p #{e mongodb_conf['password']}"
    else
      ""
    end
  end

  def mongodb_dump_remote
    puts "backup remote DB via ssh"
    Net::SSH.start(ssh_host, ssh_user, ssh_opts) do |ssh|
      ssh.exec!("rm -R #{remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{remote_dump_path}")
      check_ssh_err(ret)
      if mongodb_r_conf['options'].present? && mongodb_r_conf['options']['password'].present?
        username, password = mongodb_r_conf['options']['user'], mongodb_r_conf['options']['password']
      else
        username, password = mongodb_r_conf['username'], mongodb_r_conf['password']
      end
      dump = "mongodump -u #{e username} -p #{e password} -d #{e mongodb_r_conf['database']} --authenticationDatabase #{e mongodb_r_conf['database']} -o #{e remote_dump_path}"
      puts dump if verbose?
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def mongodb_dump_restore
    puts "restoring DB"
    restore = "mongorestore --drop -d #{e mongodb_to} #{mongodb_local_auth} #{e mongodb_path}"
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
    Rails.root.join("tmp", "dump", mongodb_to).to_s
  end

  def mongodb_dump_copy
    FileUtils.mkdir_p(mongodb_path)
    rsync("#{remote_dump_path}/#{mongodb_r_conf['database']}", mongodb_path)
  end

  def clone_mongodb
    mongodb_dump_remote()
    mongodb_dump_copy()
    mongodb_dump_restore()
  end
end
