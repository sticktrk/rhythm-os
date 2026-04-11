/// Rhythm Core - Shared Flutter package for Rhythm Lighting
///
/// This package provides:
/// - FRB bindings to the Rust curve calculation engine
/// - Data models for configuration and state
/// - Utility functions for color calculations
/// - Abstract API interface
/// - Light providers (Home Assistant, Philips Hue)
/// - Rhythm Runner for standalone adaptive lighting control
library rhythm_core;

// FRB generated bindings
export 'src/rust/frb_generated.dart';
// API functions (modular structure)
export 'src/rust/api/curve.dart';
export 'src/rust/api/helpers.dart';
export 'src/rust/api/hue.dart';
export 'src/rust/api/runner.dart';
// DTOs
export 'src/rust/api/dto/action.dart';
export 'src/rust/api/dto/color.dart';
export 'src/rust/api/dto/curve.dart';
export 'src/rust/api/dto/hue.dart';
export 'src/rust/api/dto/runner.dart';
export 'src/rust/api/dto/solar.dart';

// Models
export 'models/app_settings.dart';
export 'models/config_state.dart';
export 'models/curve_config_extensions.dart';
export 'models/home.dart';
export 'models/hub.dart';

// Utilities
export 'utils/color_utils.dart';
export 'utils/solar_utils.dart';

// API
export 'api/rhythm_api.dart';
export 'api/native_brain.dart';

// Providers
export 'providers/light_provider.dart';
export 'providers/ha_provider.dart';
export 'providers/ha_websocket_provider.dart';
export 'providers/hue_provider.dart';
export 'providers/hub_discovery.dart';

// Runner
export 'runner/provider_manager.dart';
export 'runner/rhythm_runner.dart';

// Event Sources
export 'events/event_source.dart';
export 'events/hue_sse_source.dart';
export 'events/hue_device_registry.dart';
