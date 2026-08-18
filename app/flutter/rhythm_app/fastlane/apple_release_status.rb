# frozen_string_literal: true

module RhythmAppleReleaseStatus
  PLATFORM = "IOS"

  def self.read(app:, version:, build_number:)
    candidates = [
      app.get_live_app_store_version(platform: PLATFORM),
      app.get_in_review_app_store_version(platform: PLATFORM),
      app.get_pending_release_app_store_version(platform: PLATFORM),
      app.get_edit_app_store_version(platform: PLATFORM)
    ].compact

    app_version = candidates.find do |candidate|
      candidate.version_string.to_s == version.to_s &&
        candidate.build &&
        candidate.build.version.to_s == build_number.to_s
    end

    {
      version: version.to_s,
      build_number: build_number.to_s,
      raw_state: app_version ? app_version.app_version_state.to_s : "MISSING"
    }
  end
end
