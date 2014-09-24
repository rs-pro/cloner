# coding: utf-8
lib = File.expand_path('../lib', __FILE__)
$LOAD_PATH.unshift(lib) unless $LOAD_PATH.include?(lib)
require 'cloner/version'

Gem::Specification.new do |spec|
  spec.name          = "cloner"
  spec.version       = Cloner::VERSION
  spec.authors       = ["glebtv"]
  spec.email         = ["glebtv@gmail.com"]
  spec.summary       = %q{Easily clone your production Mongoid database and files for local development}
  spec.description   = %q{}
  spec.homepage      = "https://github.com/rs-pro/cloner"
  spec.license       = "MIT"

  spec.files         = `git ls-files -z`.split("\x0")
  spec.executables   = spec.files.grep(%r{^bin/}) { |f| File.basename(f) }
  spec.test_files    = spec.files.grep(%r{^(test|spec|features)/})
  spec.require_paths = ["lib"]

  spec.add_dependency "thor"
  spec.add_dependency "activesupport"
  spec.add_dependency 'net-ssh'

  spec.add_development_dependency "bundler", "~> 1.7"
  spec.add_development_dependency "rake", "~> 10.0"
end

