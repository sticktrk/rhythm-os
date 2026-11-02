import Flutter
import HealthKit

/// Writes button-reported Time in Bed. No Health read authorization or queries.
final class AppleHealthBridge {
  private let store = HKHealthStore()
  private let sleepType = HKObjectType.categoryType(forIdentifier: .sleepAnalysis)!
  private var inFlight = false

  private func authorization() -> String {
    guard HKHealthStore.isHealthDataAvailable() else { return "unavailable" }
    switch store.authorizationStatus(for: sleepType) {
    case .sharingAuthorized: return "authorized"
    case .sharingDenied: return "denied"
    case .notDetermined: return "not_determined"
    @unknown default: return "unavailable"
    }
  }

  func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    switch call.method {
    case "authorization": result(authorization())
    case "requestAuthorization":
      guard HKHealthStore.isHealthDataAvailable() else { result("unavailable"); return }
      guard !inFlight else { result(failure("busy")); return }
      inFlight = true
      store.requestAuthorization(toShare: [sleepType], read: []) { [weak self] _, error in
        DispatchQueue.main.async {
          guard let self else { return }
          self.inFlight = false
          // request completion only says the sheet completed, not consent.
          if error == nil { result(self.authorization()) }
          else { result(self.failure("authorization")) }
        }
      }
    case "saveIntervals":
      guard authorization() == "authorized" else { result(failure("authorization")); return }
      guard !inFlight else { result(failure("busy")); return }
      let intervals: [AppleHealthInterval]
      do { intervals = try AppleHealthInterval.decode(call.arguments) }
      catch { result(failure("invalid_intervals")); return }
      let fallbackTimeZone = TimeZone.current
      let samples = intervals.map {
        $0.makeSample(type: sleepType, fallbackTimeZone: fallbackTimeZone)
      }
      inFlight = true
      store.save(samples) { [weak self] saved, _ in
        DispatchQueue.main.async {
          guard let self else { return }
          self.inFlight = false
          if saved { result(true) }
          else { result(self.failure("health_write")) }
        }
      }
    default: result(FlutterMethodNotImplemented)
    }
  }

  private func failure(_ code: String) -> FlutterError {
    FlutterError(code: code, message: "Apple Health sync could not complete.", details: nil)
  }
}
