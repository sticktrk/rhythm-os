import Foundation

/// The original owner code crosses the app/extension boundary only in this
/// protected, backup-excluded, per-attempt file. Platform handoff codes never
/// enter this file or UserDefaults.
struct PhoneMatterRequestContext: Codable {
  let baseURL: String
  let authToken: String?
  let originalSetupPayload: String
  let sessionID: String
  let createdAt: Date
  let backend: String?
  let codeSource: String?

  init(baseURL: String, authToken: String?, originalSetupPayload: String,
       sessionID: String, createdAt: Date, backend: String? = nil, codeSource: String? = nil) {
    self.baseURL = baseURL; self.authToken = authToken
    self.originalSetupPayload = originalSetupPayload; self.sessionID = sessionID
    self.createdAt = createdAt; self.backend = backend; self.codeSource = codeSource
  }

  static let lifetime: TimeInterval = 600

  func handoffRequest(onboardingPayload: String) throws -> URLRequest {
    guard backend == nil || backend == "rhythm" || backend == "ha_addon",
      codeSource == nil || codeSource == "original_label" || codeSource == "sharing"
    else { throw ContextError.invalidRequest }
    let path = backend == "ha_addon" ? "/api/addon/matter/pair" : "/api/devices/pair"
    guard let url = URL(string: baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
      + path) else { throw ContextError.invalidRequest }
    var request = URLRequest(url: url)
    request.httpMethod = "POST"
    request.timeoutInterval = 240
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    if let token = authToken, !token.isEmpty {
      request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    }
    if backend == "ha_addon" {
      request.httpBody = try JSONSerialization.data(withJSONObject: [
        "session_id": sessionID, "setup_code": originalSetupPayload,
        "code_source": codeSource ?? "original_label", "rendezvous": "phone",
        "handoff_setup_payload": onboardingPayload,
      ])
      return request
    }
    request.httpBody = try JSONSerialization.data(withJSONObject: [
      "hub_type": "matter",
      "session_id": sessionID,
      "params": [
        "setup_payload": originalSetupPayload,
        "network": "wifi",
        "rendezvous": "phone",
        "session_id": sessionID,
        "handoff_setup_payload": onboardingPayload,
      ],
    ])
    return request
  }

  /// Keep the platform's commissioning window open until HA confirms completion.
  /// Only receipt GETs are repeated; a lost result never replays commissioning.
  func awaitHaCompletion(data initialData: Data, statusCode initialStatus: Int,
                         session: URLSession, pollNanoseconds: UInt64 = 1_500_000_000,
                         isCurrent: () -> Bool = { true }) async throws -> Data {
    guard isCurrent() else { throw CancellationError() }
    guard backend == "ha_addon" else {
      return try PhoneMatterBridge.responseEnvelope(data: initialData, statusCode: initialStatus)
    }
    guard sessionID.range(of: "^[A-Za-z0-9_-]{1,128}$", options: .regularExpression) != nil,
      let url = URL(string: baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        + "/api/addon/matter/pairing/" + sessionID) else { throw ContextError.invalidRequest }
    let deadline = Date().addingTimeInterval(230)
    var data = initialData
    var status = initialStatus
    while true {
      guard isCurrent() else { throw CancellationError() }
      _ = try PhoneMatterBridge.responseEnvelope(data: data, statusCode: status)
      guard let receipt = try JSONSerialization.jsonObject(with: data) as? [String: Any],
        receipt["session_id"] as? String == sessionID else { throw ContextError.invalidRequest }
      if receipt["status"] as? String == "completed" {
        return try PhoneMatterBridge.responseEnvelope(data: data, statusCode: status)
      }
      guard receipt["status"] as? String == "pending", Date() < deadline else {
        throw PhoneMatterBridge.ServerPairingError(message:
          "Home Assistant did not confirm pairing. Check this attempt in Rhythm before trying again.")
      }
      try await Task.sleep(nanoseconds: pollNanoseconds)
      guard isCurrent() else { throw CancellationError() }
      var request = URLRequest(url: url)
      request.timeoutInterval = 10
      request.setValue("no-store", forHTTPHeaderField: "Cache-Control")
      if let token = authToken, !token.isEmpty {
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
      }
      let response: URLResponse
      (data, response) = try await session.data(for: request)
      guard let httpResponse = response as? HTTPURLResponse else { throw ContextError.invalidRequest }
      status = httpResponse.statusCode
    }
  }

  func store(in container: URL) throws -> String {
    let directory = try Self.directory(in: container)
    // A terminated app may not have executed its defer. Remove only expired
    // files from this feature's private directory before staging another one.
    for url in try FileManager.default.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: [.contentModificationDateKey]
    ) where url.pathExtension == "json" {
      if let modified = try? url.resourceValues(forKeys: [.contentModificationDateKey])
        .contentModificationDate, Date().timeIntervalSince(modified) >= Self.lifetime {
        try? FileManager.default.removeItem(at: url)
      }
    }
    let name = UUID().uuidString + ".json"
    let url = directory.appendingPathComponent(name)
    var options: Data.WritingOptions = [.atomic]
    #if os(iOS)
    options.insert(.completeFileProtection)
    #endif
    try JSONEncoder().encode(self).write(to: url, options: options)
    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    return name
  }

  static func load(in container: URL, name: String, sessionID: String,
                   now: Date = Date()) throws -> PhoneMatterRequestContext {
    let context = try JSONDecoder().decode(Self.self, from: Data(contentsOf:
      try file(in: container, name: name)))
    let age = now.timeIntervalSince(context.createdAt)
    guard context.sessionID == sessionID, age >= 0, age < lifetime,
          !context.originalSetupPayload.isEmpty else { throw ContextError.invalidRequest }
    return context
  }

  static func remove(in container: URL, name: String) {
    guard let url = try? file(in: container, name: name) else { return }
    try? FileManager.default.removeItem(at: url)
  }

  private static func file(in container: URL, name: String) throws -> URL {
    guard name.hasSuffix(".json"), UUID(uuidString: String(name.dropLast(5))) != nil else {
      throw ContextError.invalidRequest
    }
    return try directory(in: container).appendingPathComponent(name)
  }

  private static func directory(in container: URL) throws -> URL {
    var directory = container.appendingPathComponent("PhoneMatterRequests", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                            attributes: [.posixPermissions: 0o700])
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try directory.setResourceValues(values)
    return directory
  }

  enum ContextError: Error { case invalidRequest }
}

/// Shared app/extension bridge metadata. Secrets live only in the request file.
enum PhoneMatterBridge {
  static let appGroup = "group.lighting.rhythm.app.matter"
  static let requestFileKey = "phone_matter.request_file"
  static let sessionIDKey = "phone_matter.session_id"
  static let responseKey = "phone_matter.response"
  static let failureStageKey = "phone_matter.failure_stage"
  static let failureMessageKey = "phone_matter.failure_message"

  /// Only offer phone-assisted commissioning when the extension is embedded.
  /// Runtime availability remains gated for builds without this optional target.
  static func isExtensionPackaged(in bundle: Bundle = .main) -> Bool {
    guard let plugins = bundle.builtInPlugInsURL else { return false }
    let path = plugins.appendingPathComponent("MatterCommissioningExtension.appex").path
    var isDirectory: ObjCBool = false
    return FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory)
      && isDirectory.boolValue
  }

  struct ServerPairingError: Error { let message: String }

  /// A 200 receipt is owned by the shared Dart parser, including failed,
  /// pending and future statuses. Preserve warnings and recovery details.
  static func responseEnvelope(data: Data, statusCode: Int) throws -> Data {
    let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    guard statusCode == 200 else {
      let message = ((body?["error"] ?? body?["message"]) as? String)
        .map(safeMessage) ?? "The Rhythm Box could not finish Matter pairing."
      throw ServerPairingError(message: message)
    }
    guard let body else { throw PhoneMatterRequestContext.ContextError.invalidRequest }
    return try JSONSerialization.data(withJSONObject: ["http_status": statusCode, "body": body])
  }

  static func clear(_ defaults: UserDefaults) {
    if let name = defaults.string(forKey: requestFileKey),
       let container = FileManager.default.containerURL(
         forSecurityApplicationGroupIdentifier: appGroup) {
      PhoneMatterRequestContext.remove(in: container, name: name)
    }
    [
      requestFileKey,
      sessionIDKey,
      responseKey,
      failureStageKey,
      failureMessageKey,
    ].forEach(defaults.removeObject(forKey:))
  }

  static func recordFailure(
    _ defaults: UserDefaults,
    stage: String,
    message: String
  ) {
    defaults.set(stage, forKey: failureStageKey)
    defaults.set(safeMessage(message), forKey: failureMessageKey)
  }

  static func safeMessage(_ value: String) -> String {
    String(value.prefix(240))
  }
}
