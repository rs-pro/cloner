module Cloner::Internal
  extend ActiveSupport::Concern
  include Cloner::MongoDB
  include Cloner::SSH
  include Cloner::RSync

  def load_env
    unless defined?(Rails)
      require rails_path
    end
    require 'net/ssh'
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
end

