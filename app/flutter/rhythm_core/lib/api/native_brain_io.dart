import 'dart:io';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';

/// Get the external library for native platforms.
///
/// For iOS/macOS with Cargokit static linking, the Rust library
/// is linked into the main executable and accessed via DynamicLibrary.process().
ExternalLibrary? getExternalLibrary() {
  if (Platform.isIOS || Platform.isMacOS) {
    // Static library is force-loaded into the app via Cargokit
    // Symbols are available in the process
    return ExternalLibrary.process(iKnowHowToUseIt: true);
  }
  // Android and other platforms: let FRB find the library
  return null;
}
