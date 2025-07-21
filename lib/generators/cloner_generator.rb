class ClonerGenerator < Rails::Generators::Base
  source_root File.expand_path('templates', __dir__)

  class_option :extend, default: false, type: :boolean, aliases: '-e'
  class_option :docker_compose, default: false, type: :boolean, aliases: '-d', desc: 'Include Docker Compose configuration examples'

  desc "This generator create lib/tasks/dl.thor"
  def create_task_file
    if options[:docker_compose]
      create_docker_compose_task_file
    elsif options[:extend]
      create_extended_task_file
    else
      create_default_task_file
    end
  end

  private
  def create_default_task_file
    copy_file 'cloner_base.template', 'lib/tasks/dl.thor'
  end

  def create_extended_task_file
    say 'Create extend file'
    module_parent_name = if Rails::VERSION::MAJOR >= 6
                           Rails.application.class.module_parent_name
                         else
                           Rails.application.class.parent_name
                         end
    @username = module_parent_name.downcase
    template 'cloner_extend.thor.erb', 'lib/tasks/dl.thor'
  end
  
  def create_docker_compose_task_file
    say 'Create Docker Compose file with examples'
    @username = Rails.application.class.parent_name.downcase
    template 'cloner_docker_compose.thor.erb', 'lib/tasks/dl.thor'
  end
end
