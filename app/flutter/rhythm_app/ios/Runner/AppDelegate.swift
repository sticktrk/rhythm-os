import Flutter
import Matter
import MatterSupport
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    guard let registrar = engineBridge.pluginRegistry.registrar(
      forPlugin: "PhoneMatterCommissioner"
    ) else { return }
    let channel = FlutterMethodChannel(
      name: "lighting.rhythm.app/phone_matter_commissioner",
      binaryMessenger: registrar.messenger()
    )
    channel.setMethodCallHandler { [weak self] call, result in
      self?.handlePhoneMatterCall(call, result: result)
    }
  }

  override func buildMenu(with builder: any UIMenuBuilder) {
    // Flutter doesn't use storyboard menus — prevent iPadOS from
    // loading Main.storyboard which crashes on iPadOS 26 beta.
  }

  private var phoneMatterInFlight = false

  private func handlePhoneMatterCall(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    switch call.method {
    case "isSupported":
      if #available(iOS 17.6, *) {
        result(MatterAddDeviceRequest.isSupported)
      } else {
        result(false)
      }
    case "commission":
      guard !phoneMatterInFlight else {
        result(FlutterError(code: "platform_commissioning",
          message: "Another Matter commissioning request is already active.", details: nil))
        return
      }
      guard #available(iOS 17.6, *), MatterAddDeviceRequest.isSupported else {
        result(FlutterError(
          code: "native_unavailable",
          message: "Phone Matter commissioning is unavailable on this iPhone.",
          details: nil
        ))
        return
      }
      guard
        let arguments = call.arguments as? [String: Any],
        let baseURL = (arguments["base_url"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
        !baseURL.isEmpty,
        let setupCode = (arguments["setup_payload"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
        !setupCode.isEmpty,
        let sessionID = (arguments["session_id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
        !sessionID.isEmpty,
        let defaults = UserDefaults(suiteName: PhoneMatterBridge.appGroup),
        let container = FileManager.default.containerURL(
          forSecurityApplicationGroupIdentifier: PhoneMatterBridge.appGroup)
      else {
        result(FlutterError(
          code: "handoff",
          message: "The Matter commissioning request is incomplete.",
          details: nil
        ))
        return
      }

      PhoneMatterBridge.clear(defaults)
      do {
        let requestContext = PhoneMatterRequestContext(
          baseURL: baseURL, authToken: arguments["auth_token"] as? String,
          originalSetupPayload: setupCode, sessionID: sessionID, createdAt: Date())
        let fileName = try requestContext.store(in: container)
        defaults.set(fileName, forKey: PhoneMatterBridge.requestFileKey)
        defaults.set(sessionID, forKey: PhoneMatterBridge.sessionIDKey)
      } catch {
        PhoneMatterBridge.clear(defaults)
        result(FlutterError(code: "handoff",
          message: "The iPhone could not prepare the Matter handoff. Please try again.", details: nil))
        return
      }
      phoneMatterInFlight = true

      Task { @MainActor in
        defer {
          PhoneMatterBridge.clear(defaults)
          phoneMatterInFlight = false
        }
        do {
          guard let payload = PhoneMatterBridge.setupPayload(setupCode) else {
            throw PhoneMatterBridgeError.invalidSetupPayload
          }
          let home = MatterAddDeviceRequest.Home(displayName: "Rhythm Home")
          let topology = MatterAddDeviceRequest.Topology(
            ecosystemName: "Rhythm",
            homes: [home]
          )
          let request = MatterAddDeviceRequest(
            topology: topology,
            setupPayload: payload
          )
          try await request.perform()
          guard
            let responseData = defaults.data(forKey: PhoneMatterBridge.responseKey),
            let response = try JSONSerialization.jsonObject(with: responseData) as? [String: Any]
          else {
            throw PhoneMatterBridgeError.missingServerResponse
          }
          result(response)
        } catch {
          let stage = defaults.string(forKey: PhoneMatterBridge.failureStageKey)
            ?? PhoneMatterBridge.stage(for: error)
          let message = defaults.string(forKey: PhoneMatterBridge.failureMessageKey)
            ?? PhoneMatterBridge.message(for: error)
          result(FlutterError(code: stage, message: message, details: nil))
        }
      }
    default:
      result(FlutterMethodNotImplemented)
    }
  }
}

private enum PhoneMatterBridgeError: Error {
  case invalidSetupPayload
  case missingServerResponse
}

extension PhoneMatterBridge {
  @available(iOS 17.6, *)
  static func setupPayload(_ value: String) -> MTRSetupPayload? {
    MTRSetupPayload(payload: value)
  }

  static func stage(for error: Error) -> String {
    if error is CancellationError { return "cancelled" }
    if case PhoneMatterBridgeError.missingServerResponse = error {
      return "handoff"
    }
    return "platform_commissioning"
  }

  static func message(for error: Error) -> String {
    if error is CancellationError {
      return "Matter setup was cancelled before the Rhythm Box finished pairing."
    }
    if case PhoneMatterBridgeError.invalidSetupPayload = error {
      return "The Matter setup code is invalid. Scan the code again and retry."
    }
    if case PhoneMatterBridgeError.missingServerResponse = error {
      return "The iPhone completed setup, but received no final result from the Rhythm Box."
    }
    return "iPhone Matter setup did not complete. Please reopen the pairing window and try again."
  }
}
