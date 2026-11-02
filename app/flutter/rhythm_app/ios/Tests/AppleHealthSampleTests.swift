import Foundation
import HealthKit

/// Real sample construction with a fake persistence boundary implementing Apple's
/// greater-sync-version replacement rule. No HKHealthStore or Health reads/writes.
@main
struct AppleHealthSampleTests {
  static let type = HKObjectType.categoryType(forIdentifier: .sleepAnalysis)!
  static let newYork = TimeZone(identifier: "America/New_York")!
  static let losAngeles = TimeZone(identifier: "America/Los_Angeles")!
  static let identifier = "rhythm-in-bed-" + String(repeating: "a", count: 64)

  static func main() throws {
    try testLocalCalendarAndDaylightSaving()
    try testFallbackAndOlderPayloads()
    try testExistingSampleRepairAndMixedVersions()
    print("PASS: Health samples preserve instants, home/device time zones, DST and repeat-safe metadata repair")
  }

  static func interval(_ start: String, _ end: String,
                       timeZone: Any? = "America/New_York") throws -> AppleHealthInterval {
    let formatter = ISO8601DateFormatter()
    var row: [String: Any] = ["sync_id": identifier,
      "start_epoch_ms": Int64(formatter.date(from: start)!.timeIntervalSince1970 * 1000),
      "end_epoch_ms": Int64(formatter.date(from: end)!.timeIntervalSince1970 * 1000)]
    if let timeZone { row["time_zone"] = timeZone }
    return try AppleHealthInterval.decode([row],
      now: formatter.date(from: end)!.addingTimeInterval(1)).first!
  }

  static func clock(_ date: Date, sample: HKCategorySample) -> String {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.calendar = Calendar(identifier: .gregorian)
    formatter.timeZone = TimeZone(identifier: sample.metadata![HKMetadataKeyTimeZone] as! String)!
    formatter.dateFormat = "yyyy-MM-dd HH:mm"
    return formatter.string(from: date)
  }

  static func testLocalCalendarAndDaylightSaving() throws {
    let cases = [
      ("2026-10-21T03:00:00Z", "2026-10-21T11:00:00Z", "2026-10-20 23:00", "2026-10-21 07:00", 8.0),
      ("2026-12-21T04:00:00Z", "2026-12-21T12:00:00Z", "2026-12-20 23:00", "2026-12-21 07:00", 8.0),
      ("2026-11-01T04:30:00Z", "2026-11-01T12:30:00Z", "2026-11-01 00:30", "2026-11-01 07:30", 8.0),
      ("2026-03-08T05:30:00Z", "2026-03-08T11:30:00Z", "2026-03-08 00:30", "2026-03-08 07:30", 6.0),
    ]
    for (start, end, localStart, localEnd, hours) in cases {
      let value = try interval(start, end)
      let sample = value.makeSample(type: type, fallbackTimeZone: losAngeles)
      precondition(sample.metadata?[HKMetadataKeyTimeZone] as? String == newYork.identifier,
        "A New York home's history must retain its named zone while the phone is in Los Angeles")
      precondition(sample.startDate == Date(timeIntervalSince1970: Double(value.startEpochMs) / 1000))
      precondition(sample.endDate == Date(timeIntervalSince1970: Double(value.endEpochMs) / 1000))
      precondition(clock(sample.startDate, sample: sample) == localStart)
      precondition(clock(sample.endDate, sample: sample) == localEnd)
      precondition(sample.endDate.timeIntervalSince(sample.startDate) == hours * 3600)
      precondition(sample.value == HKCategoryValueSleepAnalysis.inBed.rawValue)
    }
  }

  static func testFallbackAndOlderPayloads() throws {
    for missingOrInvalid: Any? in [nil, "", "Not/A_Zone", "EST", "GMT-04:00", 42] {
      let value = try interval("2026-10-21T03:00:00Z", "2026-10-21T11:00:00Z",
                               timeZone: missingOrInvalid)
      let sample = value.makeSample(type: type, fallbackTimeZone: newYork)
      precondition(sample.metadata?[HKMetadataKeyTimeZone] as? String == newYork.identifier,
        "Missing, malformed or legacy fixed-offset context must fall back to the device's named zone")
      precondition(clock(sample.startDate, sample: sample) == "2026-10-20 23:00")
    }
    for zeroZone in ["UTC", "GMT", "Etc/UTC"] {
      let value = try interval("2026-10-21T03:00:00Z", "2026-10-21T11:00:00Z", timeZone: zeroZone)
      let sample = value.makeSample(type: type, fallbackTimeZone: newYork)
      precondition(TimeZone(identifier: sample.metadata![HKMetadataKeyTimeZone] as! String)!.secondsFromGMT(for: sample.startDate) == 0)
    }
    let alias = try interval("2026-10-21T03:00:00Z", "2026-10-21T11:00:00Z", timeZone: "US/Eastern")
    let sample = alias.makeSample(type: type, fallbackTimeZone: losAngeles)
    precondition(clock(sample.startDate, sample: sample) == "2026-10-20 23:00")
  }

  static func legacySample(_ interval: AppleHealthInterval) -> HKCategorySample {
    HKCategorySample(type: type, value: HKCategoryValueSleepAnalysis.inBed.rawValue,
      start: Date(timeIntervalSince1970: Double(interval.startEpochMs) / 1000),
      end: Date(timeIntervalSince1970: Double(interval.endEpochMs) / 1000),
      metadata: [HKMetadataKeySyncIdentifier: interval.syncID,
        HKMetadataKeySyncVersion: NSNumber(value: Int64.max - interval.endEpochMs),
        HKMetadataKeyTimeZone: "UTC"])
  }

  static func ending(_ original: AppleHealthInterval, delta: Int64) -> AppleHealthInterval {
    AppleHealthInterval(syncID: original.syncID, startEpochMs: original.startEpochMs,
      endEpochMs: original.endEpochMs + delta, timeZoneIdentifier: original.timeZoneIdentifier)
  }

  static func testExistingSampleRepairAndMixedVersions() throws {
    let value = try interval("2026-10-21T03:00:00Z", "2026-10-21T11:00:00Z")
    let store = FakeHealthStore()
    let old = legacySample(value)
    store.save(old)
    let repaired = value.makeSample(type: type, fallbackTimeZone: losAngeles)
    precondition(version(repaired).compare(version(old)) == .orderedDescending,
      "Ordinary resync must replace an existing UTC sample with the same identity and instants")
    store.save(repaired)
    precondition(store.samples.count == 1)
    precondition(store.samples[value.syncID]!.metadata?[HKMetadataKeyTimeZone] as? String == newYork.identifier)
    precondition(store.samples[value.syncID]!.startDate == old.startDate)
    precondition(store.samples[value.syncID]!.endDate == old.endDate)
    let uuid = store.samples[value.syncID]!.uuid
    // A lost acknowledgement/restart replays the same logical sample/version.
    store.save(value.makeSample(type: type, fallbackTimeZone: losAngeles))
    precondition(store.samples[value.syncID]!.uuid == uuid)

    let earlier = ending(value, delta: -1)
    let earlierSample = earlier.makeSample(type: type, fallbackTimeZone: losAngeles)
    precondition(version(earlierSample).compare(version(repaired)) == .orderedDescending)
    store.save(earlierSample)
    store.save(legacySample(value))
    precondition(store.samples[value.syncID]!.endDate == earlierSample.endDate,
      "A stale older client cannot lengthen an earlier-Wake interval")

    // Preserve an already shorter legacy sample against a later-Wake first resync.
    let legacyStore = FakeHealthStore()
    legacyStore.save(legacySample(earlier))
    legacyStore.save(repaired)
    precondition(legacyStore.samples[value.syncID]!.endDate == earlierSample.endDate)
    legacyStore.save(earlierSample)
    precondition(legacyStore.samples[value.syncID]!.metadata?[HKMetadataKeyTimeZone] as? String == newYork.identifier)

    // An older client discovering an earlier Wake can tie the new revision at
    // one millisecond, or win at two. The next updated-client retry repairs it.
    let mixed = FakeHealthStore()
    mixed.save(repaired)
    mixed.save(legacySample(earlier))
    precondition(mixed.samples[value.syncID]!.endDate == repaired.endDate)
    mixed.save(earlierSample)
    precondition(mixed.samples[value.syncID]!.endDate == earlierSample.endDate)
    let stillEarlier = ending(earlier, delta: -2)
    mixed.save(legacySample(stillEarlier))
    precondition(mixed.samples[value.syncID]!.metadata?[HKMetadataKeyTimeZone] as? String == "UTC")
    mixed.save(stillEarlier.makeSample(type: type, fallbackTimeZone: losAngeles))
    precondition(mixed.samples.count == 1)
    precondition(mixed.samples[value.syncID]!.metadata?[HKMetadataKeyTimeZone] as? String == newYork.identifier)

    let minimum = try AppleHealthInterval.decode([["sync_id": identifier,
      "start_epoch_ms": Int64(1), "end_epoch_ms": Int64(2)]], now: Date()).first!
    precondition(minimum.syncVersion == Int64.max - 1)
  }

  static func version(_ sample: HKCategorySample) -> NSNumber {
    sample.metadata![HKMetadataKeySyncVersion] as! NSNumber
  }

  final class FakeHealthStore {
    var samples: [String: HKCategorySample] = [:]
    func save(_ sample: HKCategorySample) {
      let key = sample.metadata![HKMetadataKeySyncIdentifier] as! String
      if let previous = samples[key], version(sample).compare(version(previous)) != .orderedDescending {
        return
      }
      samples[key] = sample
    }
  }
}
