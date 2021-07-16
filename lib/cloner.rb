require 'cloner/version'
require 'thor'
require 'active_support/concern'
require 'fileutils'

module Cloner
  autoload :Base, "cloner/base"
  autoload :Internal, "cloner/internal"
  autoload :Ar, "cloner/ar"
  autoload :MongoDB, "cloner/mongodb"
  autoload :Postgres, "cloner/postgres"
  autoload :MySQL, "cloner/mysql"
  autoload :SSH, "cloner/ssh"
  autoload :RSync, "cloner/rsync"
end

