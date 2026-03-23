import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';

/// Get the external library for web.
///
/// On web, the WASM module is loaded via the default mechanism.
ExternalLibrary? getExternalLibrary() {
  // Web uses WASM loading, no external library override needed
  return null;
}
