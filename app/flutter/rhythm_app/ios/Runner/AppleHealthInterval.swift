import Foundation
import CoreFoundation

struct AppleHealthInterval {
  let syncID: String
  let startEpochMs: Int64
  let endEpochMs: Int64

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
      return AppleHealthInterval(syncID: identifier, startEpochMs: start, endEpochMs: end)
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
