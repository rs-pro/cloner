module Cloner::Ar
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

  def clone_ar
    if ar_conf["adapter"] != ar_r_conf["adapter"]
      puts "Error: ActiveRecord adapter mismatch: local #{ar_conf["adapter"]}, remote #{ar_r_conf["adapter"]}"
      puts "it is not possible to convert from one database to another via this tool."
      exit
    end

    case ar_conf["adapter"]
    when 'postgresql'
      clone_pg
    when 'mysql2'
      clone_my
    else
      puts "unknown activerecord adapter: #{ar_conf["adapter"]}"
      puts "currently supported adapters: mysql2, postgresql"
      exit
    end
  end
end

