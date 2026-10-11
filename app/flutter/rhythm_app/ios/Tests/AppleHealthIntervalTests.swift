import Foundation

@main
struct AppleHealthIntervalTests {
  static func main() throws {
    let start: Int64 = 1770073200000
    let now = Date(timeIntervalSince1970: Double(start + 10 * 3600000) / 1000)
    let row: [String: Any] = ["sync_id": "rhythm-in-bed-" + String(repeating: "a", count: 64),
      "start_epoch_ms": start, "end_epoch_ms": start + 8 * 3600000]
    let interval = try AppleHealthInterval.decode([row], now: now).first!
    precondition(interval.startEpochMs == start)
    precondition(interval.endEpochMs == start + 8 * 3600000)
    let invalidFields: [(String, Any)] = [("sync_id", "private-source"),
      ("start_epoch_ms", true), ("start_epoch_ms", 1.5),
      ("start_epoch_ms", start + 9 * 3600000), ("end_epoch_ms", start + 25 * 3600000),
      ("end_epoch_ms", start + 11 * 3600000)]
    for (key, value) in invalidFields {
      var invalid = row
      invalid[key] = value
      do {
        _ = try AppleHealthInterval.decode([invalid], now: now)
        fatalError("Accepted invalid interval")
      } catch AppleHealthInterval.InvalidInterval.invalid {}
    }
    for invalid in [[], [row, row], Array(repeating: row, count: 101)] {
      do {
        _ = try AppleHealthInterval.decode(invalid, now: now)
        fatalError("Accepted invalid batch")
      } catch AppleHealthInterval.InvalidInterval.invalid {}
    }
    print("PASS: native Health interval contract rejects invalid, future, duplicate and oversized writes")
  }
}
