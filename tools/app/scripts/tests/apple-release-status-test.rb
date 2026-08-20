# frozen_string_literal: true

require "minitest/autorun"
require_relative "../../../../app/flutter/rhythm_app/fastlane/apple_release_status"

Build = Struct.new(:version)
AppVersion = Struct.new(:version_string, :app_version_state, :build)

class FakeApp
  attr_reader :queries

  def initialize(live: nil, in_review: nil, pending: nil, edit: nil)
    @versions = {
      live: live,
      in_review: in_review,
      pending: pending,
      edit: edit
    }
    @queries = []
  end

  %i[live in_review pending edit].each do |kind|
    method_kind = kind == :pending ? "pending_release" : kind
    define_method("get_#{method_kind}_app_store_version") do |platform:|
      @queries << [kind, platform]
      @versions[kind]
    end
  end
end

class AppleReleaseStatusTest < Minitest::Test
  def version(state, build: "3", semantic_version: "4.0.303")
    AppVersion.new(semantic_version, state, build.nil? ? nil : Build.new(build))
  end

  def test_reads_pending_release_version_and_exact_included_build
    app = FakeApp.new(pending: version("PENDING_DEVELOPER_RELEASE"))

    status = RhythmAppleReleaseStatus.read(
      app: app,
      version: "4.0.303",
      build_number: "3"
    )

    assert_equal "PENDING_DEVELOPER_RELEASE", status[:raw_state]
    assert_equal [
      [:live, "IOS"],
      [:in_review, "IOS"],
      [:pending, "IOS"],
      [:edit, "IOS"]
    ], app.queries
  end

  def test_accepts_current_distribution_states_from_fastlane_shaped_objects
    %w[
      PENDING_APPLE_RELEASE
      PROCESSING_FOR_DISTRIBUTION
      READY_FOR_DISTRIBUTION
    ].each do |state|
      status = RhythmAppleReleaseStatus.read(
        app: FakeApp.new(live: version(state)),
        version: "4.0.303",
        build_number: "3"
      )
      assert_equal state, status[:raw_state]
    end
  end

  def test_rejects_same_semantic_version_with_a_different_included_build
    status = RhythmAppleReleaseStatus.read(
      app: FakeApp.new(in_review: version("IN_REVIEW", build: "4")),
      version: "4.0.303",
      build_number: "3"
    )

    assert_equal "MISSING", status[:raw_state]
  end

  def test_rejects_version_without_an_included_build
    status = RhythmAppleReleaseStatus.read(
      app: FakeApp.new(edit: version("PREPARE_FOR_SUBMISSION", build: nil)),
      version: "4.0.303",
      build_number: "3"
    )

    assert_equal "MISSING", status[:raw_state]
  end
end
