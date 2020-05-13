module Cloner::RSync
  extend ActiveSupport::Concern

  def rsync_compression
    "-zz"
  end

  def rsync_flags
    port = ssh_opts[:port] || 22
    "#{rsync_compression} -utvr --checksum -e \"ssh -p #{port}\""
  end

  def rsync(from, to)
    cmd = "rsync #{e rsync_flags} #{e ssh_user}@#{e ssh_host}:#{e from}/ #{e to}/"
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
