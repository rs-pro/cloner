class ClonerGenerator < Rails::Generators::Base
  source_root File.expand_path('templates', __dir__)

  class_option :extend, default: false, type: :boolean, aliases: '-e'

  desc "This generator create lib/tasks/dl.thor"
  def create_task_file
    unless options[:extend]
      create_default_task_file
    else
      create_extended_task_file
    end
  end

  private
  def create_default_task_file
    copy_file 'cloner_base.template', 'lib/tasks/dl.thor'
  end

  def create_extended_task_file
    say 'Create extend file'
    @username = Rails.application.class.parent_name.downcase
    template 'cloner_extend.thor.erb', 'lib/tasks/dl.thor'
  end
end
