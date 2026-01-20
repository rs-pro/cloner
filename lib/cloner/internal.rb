module Cloner::Internal
  extend ActiveSupport::Concern
  include Cloner::DockerCompose
  include Cloner::MongoDB
  include Cloner::Ar
  include Cloner::Postgres
  include Cloner::MySQL
  include Cloner::SSH
  include Cloner::RSync

  def e(str)
    Shellwords.escape(str)
  end

  def load_env
    unless defined?(Rails)
      require rails_path
    end
    require 'net/ssh'
  end

  # Returns the project root directory path.
  # Override this method when not using Rails.
  def project_root
    if defined?(Rails)
      Rails.root
    else
      raise NotImplementedError, "project_root must be defined when not using Rails"
    end
  end

  # Returns the current environment name.
  # Override this method when not using Rails.
  def project_env
    if defined?(Rails)
      Rails.env
    else
      raise NotImplementedError, "project_env must be defined when not using Rails"
    end
  end

  def verbose?
    false
  end
  def env_from
    ENV['CLONE_FROM'] || 'production'
  end

  def clone_db
    if defined?(Mongoid)
      clone_mongodb
    else
      clone_ar
    end
  end
end

