import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    GeneratedPluginRegistrant.register(with: self)

    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  override func buildMenu(with builder: any UIMenuBuilder) {
    // Flutter doesn't use storyboard menus — prevent iPadOS from
    // loading Main.storyboard which crashes on iPadOS 26 beta.
  }
}
