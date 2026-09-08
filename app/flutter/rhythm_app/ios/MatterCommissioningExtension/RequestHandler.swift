import Foundation
import MatterSupport

final class RequestHandler: MatterAddDeviceExtensionRequestHandler {
  override func selectWiFiNetwork(
    from wifiScanResults: [MatterAddDeviceExtensionRequestHandler.WiFiScanResult]
  ) async throws -> MatterAddDeviceExtensionRequestHandler.WiFiNetworkAssociation {
    .defaultSystemNetwork
  }

  override func selectThreadNetwork(
    from threadScanResults: [MatterAddDeviceExtensionRequestHandler.ThreadScanResult]
  ) async throws -> MatterAddDeviceExtensionRequestHandler.ThreadNetworkAssociation {
    .defaultSystemNetwork
  }

  override func commissionDevice(
    in home: MatterAddDeviceRequest.Home?,
    onboardingPayload: String,
    commissioningID: UUID
  ) async throws {
    guard
      let defaults = UserDefaults(suiteName: PhoneMatterBridge.appGroup),
      let container = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: PhoneMatterBridge.appGroup),
      let fileName = defaults.string(forKey: PhoneMatterBridge.requestFileKey),
      let sessionID = defaults.string(forKey: PhoneMatterBridge.sessionIDKey)
    else {
      throw ExtensionBridgeError.missingRequest
    }

    do {
      let context = try PhoneMatterRequestContext.load(
        in: container, name: fileName, sessionID: sessionID)
      let request = try context.handoffRequest(onboardingPayload: onboardingPayload)
      let configuration = URLSessionConfiguration.ephemeral
      configuration.timeoutIntervalForRequest = 240
      configuration.timeoutIntervalForResource = 250
      let (data, response) = try await URLSession(configuration: configuration).data(for: request)
      guard let httpResponse = response as? HTTPURLResponse else {
        throw ExtensionBridgeError.missingResponse
      }
      let result = try PhoneMatterBridge.responseEnvelope(data: data, statusCode: httpResponse.statusCode)
      defaults.set(result, forKey: PhoneMatterBridge.responseKey)
    } catch let error as PhoneMatterBridge.ServerPairingError {
      PhoneMatterBridge.recordFailure(defaults, stage: "server_rejected", message: error.message)
      throw error
    } catch {
      PhoneMatterBridge.recordFailure(
        defaults,
        stage: "handoff",
        message: "The iPhone could not reach the Rhythm Box to finish pairing."
      )
      throw error
    }
  }

  override func rooms(
    in home: MatterAddDeviceRequest.Home?
  ) async -> [MatterAddDeviceRequest.Room] {
    []
  }
}

private enum ExtensionBridgeError: Error, Equatable {
  case missingRequest
  case missingResponse
}
