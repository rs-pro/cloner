module Cloner::Internal
  extend ActiveSupport::Concern
  def load_env
    require 'net/ssh'
    return if defined?(Rails)
    require rails_path
  end

  def local_auth(conf)
    if conf['password'].nil?
      ""
    else
      "-u #{conf['username']} -p #{conf['password']}]"
    end
  end

  # http://stackoverflow.com/questions/3386233/how-to-get-exit-status-with-rubys-netssh-library
  def ssh_exec!(ssh, command)
    stdout_data = ""
    stderr_data = ""
    exit_code = nil
    exit_signal = nil
    ssh.open_channel do |channel|
      channel.exec(command) do |ch, success|
        unless success
          abort "FAILED: couldn't execute command (ssh.channel.exec)"
        end
        channel.on_data do |ch,data|
          stdout_data+=data
        end

        channel.on_extended_data do |ch,type,data|
          stderr_data+=data
        end

        channel.on_request("exit-status") do |ch,data|
          exit_code = data.read_long
        end

        channel.on_request("exit-signal") do |ch, data|
          exit_signal = data.read_long
        end
      end
    end
    ssh.loop
    [stdout_data, stderr_data, exit_code, exit_signal]
  end

  def check_ssh_err(ret)
    if ret[2] != 0
      puts "Error: SSH command exited with #{exit_code}"
      puts stdout_data
      puts stderr_data
      exit 1
    end
  end

  def conf
    @conf ||= begin
      YAML.load_file(Rails.root.join('config', 'mongoid.yml'))[Rails.env]['sessions']['default']
    end
  end

  def r_conf
    @r_conf ||= begin
      Net::SSH.start(ssh_host, ssh_user, ssh_opts) do |ssh|
        ret = ssh_exec!(ssh, "cat #{remote_app_path}/config/mongoid.yml")
        check_ssh_err(ret)
        YAML.load(ret[0])[env_from]['sessions']['default']
      end
    end
  end

  def db_dump_remote
    puts "backup remote DB via ssh"
    Net::SSH.start(ssh_host, ssh_user, ssh_opts) do |ssh|
      ssh.exec!("rm -R #{remote_dump_path}")
      ret = ssh_exec!(ssh, "mkdir -p #{remote_dump_path}")
      check_ssh_err(ret)
      dump = "mongodump -u #{r_conf['username']} -p #{r_conf['password']} -d #{r_conf['database']} --authenticationDatabase #{r_conf['database']} -o #{remote_dump_path}"
      puts dump
      ret = ssh_exec!(ssh, dump)
      check_ssh_err(ret)
    end
  end

  def db_dump_restore
    puts "restoring DB"
    if Rails.env.staging?
      restore = "mongorestore --drop -d #{db_to} -u #{remote_db_user} -p #{remote_db_pass} --authenticationDatabase admin #{db_path}"
    else
      restore = "mongorestore --drop -d #{db_to} #{local_auth(conf)} #{db_path}"
    end
    puts restore
    pipe = IO.popen(restore)
    while (line = pipe.gets)
      print line
    end
  end

  def db_path
    Rails.root.join("tmp", "dump", db_to).to_s
  end

  def db_to
    conf['database']
  end

  def db_dump_copy
    `mkdir -p #{db_path}`
    rsync("#{remote_dump_path}/#{r_conf['database']}", db_path)
  end

  def clone_db
    db_dump_remote()
    db_dump_copy()
    db_dump_restore()
  end

  def rsync(from, to)
    cmd = "rsync -e ssh --progress -lzuogthvr #{ssh_user}@#{ssh_host}:#{from}/ #{to}/"
    puts "Running RSync: #{cmd}"
    pipe = IO.popen(cmd)
    while (line = pipe.gets)
      print line
    end
    pipe.close
    ret = $?.to_i
    if ret != 0 
      puts "Error: local command exited with #{ret}"
    end
  end

  def rsync_public(folder)
    rsync("#{remote_app_path}/public/#{folder}", Rails.root.join("public/#{folder}"))
  end
end

