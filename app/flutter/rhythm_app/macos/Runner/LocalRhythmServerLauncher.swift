import Foundation
import FlutterMacOS

final class LocalRhythmServerLauncher {
  static let shared = LocalRhythmServerLauncher()

  private let queue = DispatchQueue(label: "lighting.rhythm.app.local-rhythm-server")
  private var process: Process?
  private var logHandle: FileHandle?
  private var lastPort: Int = 54448

  private init() {}

  func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(
      name: "lighting.rhythm.app/local_rhythm_server",
      binaryMessenger: messenger
    )
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self = self else {
        result(FlutterError(code: "unavailable", message: "Launcher unavailable", details: nil))
        return
      }

      switch call.method {
      case "start":
        let args = call.arguments as? [String: Any]
        let port = args?["port"] as? Int ?? 54448
        self.start(port: port, result: result)
      case "stop":
        self.stop()
        result(nil)
      case "status":
        result(self.status())
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  func stop() {
    queue.sync {
      guard let process = process else {
        closeLogHandle()
        return
      }
      if process.isRunning {
        process.terminate()
      }
      self.process = nil
      closeLogHandle()
    }
  }

  private func start(port: Int, result: @escaping FlutterResult) {
    queue.async {
      do {
        if let process = self.process, process.isRunning {
          self.lastPort = port
          DispatchQueue.main.async {
            result(self.status(lockedProcess: process, supported: true))
          }
          return
        }

        let executableURL = try self.serverExecutableURL()
        let dataDir = try self.ensureDataDirectory()
        let logURL = dataDir.appendingPathComponent("rhythm-server.log")
        let logHandle = try self.openLogHandle(at: logURL)

        let process = Process()
        process.executableURL = executableURL
        process.currentDirectoryURL = executableURL.deletingLastPathComponent()
        process.arguments = [
          "--port", "\(port)",
          "--data-dir", dataDir.path,
          "--log-level", "info",
        ]
        var environment = ProcessInfo.processInfo.environment
        environment["RHYTHM_PLATFORM_TYPE"] = "desktop"
        environment["RHYTHM_PLATFORM_CONTEXT"] = "server"
        process.environment = environment
        process.standardOutput = logHandle
        process.standardError = logHandle
        process.terminationHandler = { [weak self] terminated in
          self?.queue.async {
            if self?.process === terminated {
              self?.process = nil
              self?.closeLogHandle()
            }
          }
        }

        try process.run()
        self.process = process
        self.logHandle = logHandle
        self.lastPort = port

        let payload = self.status(lockedProcess: process, supported: true)
          .merging([
            "executablePath": executableURL.path,
            "dataDir": dataDir.path,
          ]) { _, new in new }
        DispatchQueue.main.async {
          result(payload)
        }
      } catch {
        DispatchQueue.main.async {
          result(FlutterError(
            code: "start_failed",
            message: "Could not start local Rhythm Server",
            details: "\(error)"
          ))
        }
      }
    }
  }

  private func status() -> [String: Any] {
    queue.sync {
      status(lockedProcess: process, supported: true)
    }
  }

  private func status(lockedProcess process: Process?, supported: Bool) -> [String: Any] {
    var payload: [String: Any] = [
      "supported": supported,
      "running": process?.isRunning ?? false,
      "port": lastPort,
    ]
    if let pid = process?.processIdentifier {
      payload["pid"] = Int(pid)
    }
    return payload
  }

  private func serverExecutableURL() throws -> URL {
    guard let resourceURL = Bundle.main.resourceURL else {
      throw LauncherError.missingResourceDirectory
    }
    let executableURL = resourceURL
      .appendingPathComponent("rhythm-os", isDirectory: true)
      .appendingPathComponent("rhythm-server")
    guard FileManager.default.isExecutableFile(atPath: executableURL.path) else {
      throw LauncherError.missingExecutable(executableURL.path)
    }
    return executableURL
  }

  private func ensureDataDirectory() throws -> URL {
    let root = try FileManager.default.url(
      for: .applicationSupportDirectory,
      in: .userDomainMask,
      appropriateFor: nil,
      create: true
    )
    let dataDir = root
      .appendingPathComponent("Rhythm", isDirectory: true)
      .appendingPathComponent("LocalServer", isDirectory: true)
    try FileManager.default.createDirectory(
      at: dataDir,
      withIntermediateDirectories: true,
      attributes: nil
    )
    return dataDir
  }

  private func openLogHandle(at url: URL) throws -> FileHandle {
    let fileManager = FileManager.default
    if !fileManager.fileExists(atPath: url.path) {
      fileManager.createFile(atPath: url.path, contents: nil)
    }
    let handle = try FileHandle(forWritingTo: url)
    handle.seekToEndOfFile()
    return handle
  }

  private func closeLogHandle() {
    if let logHandle = logHandle {
      try? logHandle.close()
    }
    logHandle = nil
  }
}

private enum LauncherError: Error, CustomStringConvertible {
  case missingResourceDirectory
  case missingExecutable(String)

  var description: String {
    switch self {
    case .missingResourceDirectory:
      return "Bundle resource directory is missing"
    case .missingExecutable(let path):
      return "Bundled rhythm-server is missing or not executable at \(path)"
    }
  }
}
