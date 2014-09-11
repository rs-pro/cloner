class Cloner::Base < Thor
  include Thor::Actions

  no_commands do
    include Cloner::Internal
  end

  default_command :download
end

