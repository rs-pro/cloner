#!/usr/bin/env ruby

# Test that verifies the gem can be used without Rails
# by overriding project_root and project_env methods

$LOAD_PATH.unshift File.expand_path('../lib', __dir__)

require 'pathname'
require 'minitest/autorun'
require 'cloner'

# Ensure Rails is NOT defined for this test
if defined?(Rails)
  raise "Rails should not be defined for this test"
end

class TestWithoutRails < Minitest::Test
  class NonRailsCloner < Cloner::Base
    no_commands do
      def load_env
        require 'net/ssh'
        require 'shellwords'
        require 'active_support/core_ext/object'
      end

      # Required overrides for non-Rails usage
      def project_root
        Pathname.new(File.expand_path("../fixtures", __FILE__))
      end

      def project_env
        "test"
      end

      # Mock SSH config
      def ssh_host
        'localhost'
      end

      def ssh_user
        'test'
      end

      def env_from
        "production"
      end

      def remote_app_path
        '/app'
      end

      def remote_dump_path
        '/tmp/dump'
      end

      # Mock database configs for testing path methods
      def ar_conf
        {
          'adapter' => 'postgresql',
          'database' => 'test_db',
          'username' => 'test',
          'password' => 'test'
        }
      end

      def ar_r_conf
        ar_conf
      end

      def mongodb_to
        'test_mongodb'
      end
    end
  end

  def setup
    @cloner = NonRailsCloner.new
    @cloner.load_env
  end

  def test_project_root_returns_pathname
    assert_kind_of Pathname, @cloner.project_root
  end

  def test_project_env_returns_string
    assert_equal "test", @cloner.project_env
  end

  def test_pg_path_uses_project_root
    expected = File.join(@cloner.project_root.to_s, "tmp", "dump")
    assert_equal expected, @cloner.pg_path
  end

  def test_my_path_uses_project_root
    expected = File.join(@cloner.project_root.to_s, "tmp", "dump")
    assert_equal expected, @cloner.my_path
  end

  def test_mongodb_path_uses_project_root
    expected = File.join(@cloner.project_root.to_s, "tmp", "dump", "test_mongodb")
    assert_equal expected, @cloner.mongodb_path
  end

  def test_env_database_uses_project_env
    assert_equal "test", @cloner.env_database
  end

  def test_rails_not_defined
    refute defined?(Rails), "Rails should not be defined in this test"
  end
end
