import Foundation
import CoreFoundation
import HealthKit

struct AppleHealthInterval {
  let syncID: String
  let startEpochMs: Int64
  let endEpochMs: Int64
  let timeZoneIdentifier: String?

  init(syncID: String, startEpochMs: Int64, endEpochMs: Int64,
       timeZoneIdentifier: String? = nil) {
    self.syncID = syncID
    self.startEpochMs = startEpochMs
    self.endEpochMs = endEpochMs
    self.timeZoneIdentifier = timeZoneIdentifier
  }

  // For one Sleep-event identity, the first valid Wake is authoritative.
  // Late history may shorten an exported interval. HealthKit only replaces
  // matching samples with a greater version, so invert the positive timestamp.
  // Revision 1 replaces prior UTC metadata on ordinary resync. The decoder
  // proves end >= 2, so this remains positive and cannot overflow Int64.
  // Int64/NSNumber preserves millisecond precision and identical retries.
  var syncVersion: Int64 { Int64.max - endEpochMs + 1 }

  func makeSample(type: HKCategoryType, fallbackTimeZone: TimeZone) -> HKCategorySample {
    let timeZone = displayTimeZone(fallback: fallbackTimeZone)
    return HKCategorySample(type: type, value: HKCategoryValueSleepAnalysis.inBed.rawValue,
      start: Date(timeIntervalSince1970: Double(startEpochMs) / 1000),
      end: Date(timeIntervalSince1970: Double(endEpochMs) / 1000),
      metadata: [HKMetadataKeySyncIdentifier: syncID,
        HKMetadataKeySyncVersion: NSNumber(value: syncVersion),
        HKMetadataKeyTimeZone: timeZone.identifier])
  }

  private func displayTimeZone(fallback: TimeZone) -> TimeZone {
    guard let identifier = timeZoneIdentifier,
      // Accept named IANA aliases too, but not legacy abbreviations such as
      // EST or raw offsets that discard the home's historical DST rules.
      identifier.contains("/") || identifier == "UTC" || identifier == "GMT",
      let timeZone = TimeZone(identifier: identifier)
    else { return fallback }
    return timeZone
  }

  static func decode(_ value: Any?, now: Date = Date()) throws -> [AppleHealthInterval] {
    guard let rows = value as? [[String: Any]], !rows.isEmpty, rows.count <= 100 else {
      throw InvalidInterval.invalid
    }
    var identifiers = Set<String>()
    return try rows.map { row in
      guard let identifier = row["sync_id"] as? String,
        identifier.range(of: "^rhythm-in-bed-[0-9a-f]{64}$", options: .regularExpression) != nil,
        identifiers.insert(identifier).inserted,
        let start = integer(row["start_epoch_ms"]),
        let end = integer(row["end_epoch_ms"]), start > 0, end > start,
        end - start <= 24 * 60 * 60 * 1000,
        Double(end) <= now.timeIntervalSince1970 * 1000
      else { throw InvalidInterval.invalid }
      return AppleHealthInterval(syncID: identifier, startEpochMs: start, endEpochMs: end,
        timeZoneIdentifier: row["time_zone"] as? String)
    }
  }

  private static func integer(_ value: Any?) -> Int64? {
    guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
    let double = number.doubleValue
    guard double.isFinite, double.rounded() == double, double > 0, double < Double(Int64.max) else { return nil }
    return number.int64Value
  }

  enum InvalidInterval: Error { case invalid }
}
