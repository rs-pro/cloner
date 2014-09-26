module Cloner::Internal
  extend ActiveSupport::Concern
  include Cloner::MongoDB
  include Cloner::SSH

  def load_env
    require 'net/ssh'
    return if defined?(Rails)
    require rails_path
  end

  def verbose?
    false
  end
  def env_from
    ENV['CLONE_FROM'] || 'production'
  end

  def clone_db
    clone_mongodb
  end

  def rsync(from, to)
    cmd = "rsync -e ssh --progress -lzuogthvr #{ssh_user}@#{ssh_host}:#{from}/ #{to}/"
    puts "Running RSync: #{cmd}"
    pipe = IO.popen(cmd)
    while (line = pipe.gets)
      print line if verbose?
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

