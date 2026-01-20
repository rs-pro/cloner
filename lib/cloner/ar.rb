module Cloner::Ar
  def read_ar_conf
    @conf ||= begin
      YAML.load(ERB.new(File.read(File.join(project_root.to_s, 'config', 'database.yml'))).result, aliases: true)[env_database]
    end
  end
  def ar_conf
    if multi_db?
      read_ar_conf[@current_database]
    else
      read_ar_conf
    end
  end

  def multi_db?
    false
  end

  def clone_databases
    # clone all databases by default
    nil
  end

  def env_database
    project_env
  end

  def ar_to
    local_db_config['database']
  end

  def read_ar_r_conf
    @ar_r_conf ||= begin
      do_ssh do |ssh|
        ret = ssh_exec!(ssh, "cat #{e(remote_app_path + '/config/database.yml')}")
        check_ssh_err(ret)
        begin
          res = YAML.load(ERB.new(ret[0]).result, aliases: true)[env_from]
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

  def db_file_name
    if multi_db?
      "cloner_" + @current_database
    else
      "cloner"
    end
  end

  def ar_r_conf
    if multi_db?
      read_ar_r_conf[@current_database]
    else
      read_ar_r_conf
    end
  end

  def run_clone_ar
    if local_db_config["adapter"] != remote_db_config["adapter"]
      puts "Error: ActiveRecord adapter mismatch: local #{local_db_config["adapter"]}, remote #{remote_db_config["adapter"]}"
      puts "it is not possible to convert from one database to another via this tool."
      exit
    end

    case local_db_config["adapter"]
    when 'postgresql'
      clone_pg
    when 'mysql2'
      clone_my
    else
      puts "unknown activerecord adapter: #{local_db_config["adapter"]}"
      puts "currently supported adapters: mysql2, postgresql"
      exit
    end
  end

  def clone_ar
    if multi_db?
      dblist = clone_databases
      if dblist.nil?
        dblist = read_ar_conf.keys
      end
      puts "cloning multiple databases: #{dblist.join(", ")}"
      dblist.each do |dbn|
        @current_database = dbn
        run_clone_ar
      end
      @current_database = nil
    else
      run_clone_ar
    end
  end
end

