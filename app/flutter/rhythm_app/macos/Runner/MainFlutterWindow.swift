import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    FlutterMethodChannel(name: "lighting.rhythm.app/push",
      binaryMessenger: flutterViewController.engine.binaryMessenger
    ).setMethodCallHandler { _, result in result("unsupported") }
    LocalRhythmServerLauncher.shared.register(
      with: flutterViewController.engine.binaryMessenger
    )

    super.awakeFromNib()
  }
}
