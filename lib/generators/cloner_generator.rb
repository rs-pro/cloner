class ClonerGenerator < Rails::Generators::Base
  source_root File.expand_path('templates', __dir__)

  desc "This generator create lib/tasks/dl.thor"
  def create_task_file
    copy_file "cloner_base.template", "lib/tasks/dl.thor"
  end
end
