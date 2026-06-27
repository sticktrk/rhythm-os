import 'package:flutter/material.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/config/supabase_config.dart';
import 'package:rhythm_app/services/auth_service.dart';

import 'admin_dashboard_screen.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final initError = await _initializeBackend();
  runApp(AdminPortalApp(initError: initError));
}

Future<String?> _initializeBackend() async {
  if (!SupabaseConfig.isConfigured) {
    return 'Supabase is not configured for the admin portal.';
  }

  try {
    await BackendProvider.initialize(
      BackendConfig.supabase(
        url: SupabaseConfig.url,
        anonKey: SupabaseConfig.anonKey,
        enableLogging: true,
      ),
    );
    await AuthService().initialize();
    return null;
  } catch (error, stackTrace) {
    debugPrint('RhythmAdmin: backend initialization failed: $error');
    debugPrint('$stackTrace');
    return 'Failed to initialize admin backend: $error';
  }
}

class AdminPortalApp extends StatelessWidget {
  const AdminPortalApp({super.key, this.initError});

  final String? initError;

  @override
  Widget build(BuildContext context) {
    final theme = ThemeData(
      useMaterial3: true,
      brightness: Brightness.dark,
      colorScheme: ColorScheme.fromSeed(
        seedColor: const Color(0xFF92D7BB),
        brightness: Brightness.dark,
      ),
      scaffoldBackgroundColor: const Color(0xFF0D1117),
    );

    return MaterialApp(
      title: 'Rhythm Admin',
      debugShowCheckedModeBanner: false,
      theme: theme,
      home: initError == null
          ? const AdminDashboardScreen()
          : _AdminInitErrorScreen(error: initError!),
    );
  }
}

class _AdminInitErrorScreen extends StatelessWidget {
  const _AdminInitErrorScreen({required this.error});

  final String error;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 440),
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                const Text(
                  'Admin portal unavailable',
                  style: TextStyle(
                    fontSize: 24,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                const SizedBox(height: 12),
                Text(
                  error,
                  style: TextStyle(
                    color: Theme.of(context).colorScheme.error,
                    height: 1.35,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
