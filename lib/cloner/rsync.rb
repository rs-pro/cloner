module Cloner::RSync
  extend ActiveSupport::Concern
  def rsync(from, to)
    cmd = "rsync -e ssh -zutvr --checksum #{ssh_user}@#{ssh_host}:#{from}/ #{to}/"
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
