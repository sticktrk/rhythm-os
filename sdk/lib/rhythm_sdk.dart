/// Dart SDK for communicating with Rhythm Lighting servers.
///
/// Provides typed API clients for:
/// - Curve configuration (GET/POST /api/config, /api/curve, /api/steps, /api/time)
/// - Room/device management (rooms, topology, triage, settings, hub credentials)
/// - Real-time state sync (polling + SSE with automatic reconnect)
/// - Device diagnostics (vitals, logs, crash, wifi, reboot)
/// - OTA firmware updates (check, download, upload, verify)
library;

// Models
export 'src/models/rhythm_config_state.dart';
export 'src/models/rhythm_connection_state.dart';
export 'src/models/rhythm_curve_config.dart';
export 'src/models/rhythm_curve_data.dart';
export 'src/models/rhythm_firmware.dart';
export 'src/models/rhythm_hello.dart';
export 'src/models/rhythm_room.dart';
export 'src/models/rhythm_settings.dart';
export 'src/models/rhythm_step_sequences.dart';
export 'src/models/rhythm_time_info.dart';

// Errors
export 'src/errors/rhythm_exception.dart';

// API clients
export 'src/api/rhythm_config_api.dart';
export 'src/api/rhythm_diagnostics_api.dart';
export 'src/api/rhythm_ota_api.dart';
export 'src/api/rhythm_server_api.dart';

// Real-time
export 'src/realtime/rhythm_connection.dart';
