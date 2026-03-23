/// Backend abstraction layer for Rhythm Lighting.
///
/// This library provides a unified interface for backend services
/// (auth, database, realtime, analytics) that can be swapped between
/// different providers (Supabase, Firebase, etc.).
///
/// Usage:
/// ```dart
/// import 'package:rhythm_app/backend/backend.dart';
///
/// // Initialize at app startup
/// await BackendProvider.initialize(BackendConfig.supabase(
///   url: 'https://xxx.supabase.co',
///   anonKey: 'your-anon-key',
/// ));
///
/// // Access backends
/// final user = BackendProvider.instance.auth.currentUser;
/// ```
library backend;

// Configuration
export 'backend_config.dart';
export 'backend_provider.dart';

// Auth
export 'auth/auth_backend.dart';
export 'auth/auth_user.dart';
export 'auth/supabase_auth_backend.dart';
export 'auth/offline_auth_backend.dart';

// Analytics
export 'analytics/analytics_backend.dart';
export 'analytics/console_analytics_backend.dart';
