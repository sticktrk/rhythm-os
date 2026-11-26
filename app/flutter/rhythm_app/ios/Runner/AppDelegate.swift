import Flutter
import Matter
import MatterSupport
import UIKit
import UserNotifications

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  private let appleHealthBridge = AppleHealthBridge()
  private let pushBridge = AssistantPushBridge()

  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    UNUserNotificationCenter.current().delegate = pushBridge
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    if let pushRegistrar = engineBridge.pluginRegistry.registrar(forPlugin: "AssistantPush") {
      pushBridge.register(messenger: pushRegistrar.messenger())
    }
    if let healthRegistrar = engineBridge.pluginRegistry.registrar(forPlugin: "AppleHealthSync") {
      let healthChannel = FlutterMethodChannel(name: "lighting.rhythm.app/apple_health",
        binaryMessenger: healthRegistrar.messenger())
      healthChannel.setMethodCallHandler { [weak self] call, result in
        self?.appleHealthBridge.handle(call, result: result)
      }
    }
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

  override func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
    super.application(application, didRegisterForRemoteNotificationsWithDeviceToken: deviceToken)
    pushBridge.updateToken(deviceToken)
  }

  override func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
    super.application(application, didFailToRegisterForRemoteNotificationsWithError: error)
    pushBridge.updateToken(nil)
  }

  func receivedAttention(_ notification: UNNotification) {
    pushBridge.received(notification)
  }

  override func buildMenu(with builder: any UIMenuBuilder) {
    // Flutter doesn't use storyboard menus — prevent iPadOS from
    // loading Main.storyboard which crashes on iPadOS 26 beta.
  }

  private var phoneMatterInFlight = false
  private var phoneMatterSessionID: String?
  private var phoneMatterTask: Task<Void, Never>?

  private func handlePhoneMatterCall(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    switch call.method {
    case "cancel":
      let id = (call.arguments as? [String: Any])?["session_id"] as? String
      if id != nil && id == phoneMatterSessionID {
        phoneMatterTask?.cancel()
        if let defaults = UserDefaults(suiteName: PhoneMatterBridge.appGroup) {
          PhoneMatterBridge.clear(defaults)
        }
      }
      result(nil)
    case "isSupported":
      if #available(iOS 17.6, *), PhoneMatterBridge.isExtensionPackaged() {
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
      guard #available(iOS 17.6, *), PhoneMatterBridge.isExtensionPackaged(),
        MatterAddDeviceRequest.isSupported else {
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

      let backend = arguments["backend"] as? String
      let codeSource = arguments["code_source"] as? String
      guard (backend == nil || backend == "rhythm" || backend == "ha_addon"),
        (codeSource == nil || codeSource == "original_label" || codeSource == "sharing"),
        (backend != "ha_addon" || sessionID.range(of: "^[A-Za-z0-9_-]{1,128}$", options: .regularExpression) != nil) else {
        result(FlutterError(code: "handoff", message: "Unsupported Matter handoff target.", details: nil))
        return
      }
      PhoneMatterBridge.clear(defaults)
      do {
        let requestContext = PhoneMatterRequestContext(
          baseURL: baseURL, authToken: arguments["auth_token"] as? String,
          originalSetupPayload: setupCode, sessionID: sessionID, createdAt: Date(),
          backend: backend, codeSource: codeSource)
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
      phoneMatterSessionID = sessionID

      phoneMatterTask = Task { @MainActor in
        defer {
          PhoneMatterBridge.clear(defaults)
          phoneMatterInFlight = false
          phoneMatterSessionID = nil
          phoneMatterTask = nil
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

/// UIScene delivers a notification that launched a closed app in connection options.
class RhythmSceneDelegate: FlutterSceneDelegate {
  override func scene(_ scene: UIScene, willConnectTo session: UISceneSession,
                      options connectionOptions: UIScene.ConnectionOptions) {
    super.scene(scene, willConnectTo: session, options: connectionOptions)
    if let response = connectionOptions.notificationResponse {
      (UIApplication.shared.delegate as? AppDelegate)?.receivedAttention(response.notification)
    }
  }
}

private final class AssistantPushBridge: NSObject, UNUserNotificationCenterDelegate {
  private var channel: FlutterMethodChannel?
  private var token: [String: String]?
  private var pending: [String: String]?
  private var listening = false

  func register(messenger: FlutterBinaryMessenger) {
    listening = false
    let channel = FlutterMethodChannel(name: "lighting.rhythm.app/push", binaryMessenger: messenger)
    self.channel = channel
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { result(nil); return }
      switch call.method {
      case "requestAuthorization":
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { granted, _ in
          DispatchQueue.main.async {
            if granted { UIApplication.shared.registerForRemoteNotifications() }
            result(granted)
          }
        }
      case "restoreRegistration":
        UNUserNotificationCenter.current().getNotificationSettings { settings in
          DispatchQueue.main.async {
            if [.authorized, .provisional, .ephemeral].contains(settings.authorizationStatus) {
              UIApplication.shared.registerForRemoteNotifications()
            }
            result(nil)
          }
        }
      case "stopRegistration":
        UIApplication.shared.unregisterForRemoteNotifications()
        self.token = nil
        result(nil)
      case "getToken": result(self.token)
      case "takeInitialNotification":
        self.listening = true
        result(self.pending)
        self.pending = nil
      default: result(FlutterMethodNotImplemented)
      }
    }
  }

  private static var environment: String? {
    // Store-distributed apps omit the embedded profile and use production APNs.
    guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision") else {
      return "production"
    }
    guard let data = try? Data(contentsOf: url),
          let start = data.range(of: Data("<?xml".utf8)),
          let end = data.range(of: Data("</plist>".utf8), in: start.lowerBound..<data.endIndex),
          let plist = try? PropertyListSerialization.propertyList(
            from: data.subdata(in: start.lowerBound..<end.upperBound), format: nil) as? [String: Any],
          let entitlements = plist["Entitlements"] as? [String: Any],
          let environment = entitlements["aps-environment"] as? String,
          ["development", "production"].contains(environment) else { return nil }
    return environment
  }

  func updateToken(_ data: Data?) {
    if let data, let environment = Self.environment, let appID = Bundle.main.bundleIdentifier {
      token = ["token": data.map { String(format: "%02x", $0) }.joined(),
               "environment": environment, "app_id": appID]
    } else {
      token = nil
    }
    if listening { channel?.invokeMethod("tokenChanged", arguments: token) }
  }

  func received(_ notification: UNNotification) {
    guard let message = notification.request.content.userInfo["rhythm"] as? [String: String],
          ["connection", "plan"].contains(message["kind"] ?? ""),
          let hub = message["hub_id"], !hub.isEmpty else { return }
    if listening { channel?.invokeMethod("attention", arguments: message) }
    else { pending = message }
  }

  func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                              withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
    DispatchQueue.main.async {
      self.received(notification)
      completionHandler([])
    }
  }

  func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                              withCompletionHandler completionHandler: @escaping () -> Void) {
    DispatchQueue.main.async {
      self.received(response.notification)
      completionHandler()
    }
  }
}
