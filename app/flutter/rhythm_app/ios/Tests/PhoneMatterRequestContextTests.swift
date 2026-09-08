import Foundation

@main
struct PhoneMatterRequestContextTests {
  static func main() async throws {
    let plugins = Bundle.main.builtInPlugInsURL!
    let extensionURL = plugins.appendingPathComponent("MatterCommissioningExtension.appex")
    defer { try? FileManager.default.removeItem(at: plugins) }
    precondition(!PhoneMatterBridge.isExtensionPackaged())
    try FileManager.default.createDirectory(at: plugins, withIntermediateDirectories: true)
    try Data().write(to: extensionURL)
    precondition(!PhoneMatterBridge.isExtensionPackaged())
    try FileManager.default.removeItem(at: extensionURL)
    try FileManager.default.createDirectory(at: extensionURL, withIntermediateDirectories: true)
    precondition(PhoneMatterBridge.isExtensionPackaged())
    try FileManager.default.removeItem(at: extensionURL)
    precondition(!PhoneMatterBridge.isExtensionPackaged())
    print("PASS: phone commissioning is unavailable without a packaged extension")

    let container = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: container) }
    let now = Date()
    let context = PhoneMatterRequestContext(
      baseURL: "http://127.0.0.1:\(CommandLine.arguments[1])/",
      authToken: "test-token", originalSetupPayload: "MT:ORIGINAL-OWNER-CODE",
      sessionID: "phone-attempt-1", createdAt: now)
    let name = try context.store(in: container)
    let loaded = try PhoneMatterRequestContext.load(
      in: container, name: name, sessionID: context.sessionID, now: now)
    let file = container.appendingPathComponent("PhoneMatterRequests").appendingPathComponent(name)
    let permissions = try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as? Int
    precondition(permissions == 0o600)
    let values = try file.deletingLastPathComponent().resourceValues(forKeys: [.isExcludedFromBackupKey])
    precondition(values.isExcludedFromBackup == true)
    let storedData = try Data(contentsOf: file)
    precondition(!String(decoding: storedData, as: UTF8.self).contains("TEMPORARY"))

    let request = try loaded.handoffRequest(onboardingPayload: "MT:TEMPORARY-HANDOFF")
    precondition(request.value(forHTTPHeaderField: "Authorization") == "Bearer test-token")
    // Run Foundation's actual HTTP path, with the extension's ATS policy in
    // this test bundle, against the loopback-only echo server.
    let (data, response) = try await URLSession(configuration: .ephemeral).data(for: request)
    precondition((response as? HTTPURLResponse)?.statusCode == 200)
    let echoed = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let params = echoed["params"] as! [String: Any]
    let fixture = try JSONSerialization.jsonObject(with: Data(contentsOf:
      URL(fileURLWithPath: CommandLine.arguments[2]))) as! [String: Any]
    precondition(NSDictionary(dictionary: echoed).isEqual(to: fixture["ios_request"] as! [String: Any]))
    precondition(params["setup_payload"] as? String == "MT:ORIGINAL-OWNER-CODE")
    precondition(params["handoff_setup_payload"] as? String == "MT:TEMPORARY-HANDOFF")
    precondition(params["session_id"] as? String == context.sessionID)
    print("PASS: native HTTP handoff keeps original and temporary payloads distinct")

    let failedReceipt = fixture["failed_receipt"] as! [String: Any]
    let failedData = try JSONSerialization.data(withJSONObject: failedReceipt)
    for status in ["failed", "complete", "pending", "future_status"] {
      var receipt = failedReceipt
      receipt["status"] = status
      let envelopeData = try PhoneMatterBridge.responseEnvelope(
        data: JSONSerialization.data(withJSONObject: receipt), statusCode: 200)
      let envelope = try JSONSerialization.jsonObject(with: envelopeData) as! [String: Any]
      precondition(envelope["http_status"] as? Int == 200)
      precondition(NSDictionary(dictionary: envelope["body"] as! [String: Any]).isEqual(to: receipt))
    }
    do {
      _ = try PhoneMatterBridge.responseEnvelope(data: failedData, statusCode: 403)
      preconditionFailure("HTTP rejection was accepted")
    } catch let error as PhoneMatterBridge.ServerPairingError {
      precondition(error.message == failedReceipt["error"] as! String)
    }
    for data in [Data("not json".utf8), Data("[]".utf8)] {
      do {
        _ = try PhoneMatterBridge.responseEnvelope(data: data, statusCode: 200)
        preconditionFailure("malformed receipt was accepted")
      } catch {}
    }
    let suite = "phone-matter-test-\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }
    defaults.set(try PhoneMatterBridge.responseEnvelope(data: failedData, statusCode: 200),
                 forKey: PhoneMatterBridge.responseKey)
    PhoneMatterBridge.recordFailure(defaults, stage: "handoff", message: "Test")
    PhoneMatterBridge.clear(defaults)
    precondition(defaults.data(forKey: PhoneMatterBridge.responseKey) == nil)
    precondition(defaults.string(forKey: PhoneMatterBridge.failureStageKey) == nil)
    print("PASS: all 200 receipts preserve recovery/warnings; HTTP rejection and malformed bodies stay failures")

    for (candidateName, sessionID, date) in [
      (name, "different-attempt", now),
      (name, context.sessionID, now.addingTimeInterval(601)),
      (name, context.sessionID, now.addingTimeInterval(-1)),
      ("../request.json", context.sessionID, now),
    ] {
      do {
        _ = try PhoneMatterRequestContext.load(
          in: container, name: candidateName, sessionID: sessionID, now: date)
        preconditionFailure("invalid request context was accepted")
      } catch {}
    }
    PhoneMatterRequestContext.remove(in: container, name: name)
    precondition(!FileManager.default.fileExists(atPath: file.path))
    print("PASS: request files are private, excluded from backup, expire, and are removed")
  }
}
