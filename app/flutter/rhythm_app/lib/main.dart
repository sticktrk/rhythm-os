import 'dart:async';

import 'package:flutter/foundation.dart' show debugPrint, kDebugMode, kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show Level, RhythmConnection, RhythmSdk;

import 'app_shell.dart';
import 'backend/backend.dart';
import 'models/config_model.dart';
import 'api/hybrid_client.dart';
import 'onboarding/onboarding_flow.dart';
import 'services/auth_service.dart';
import 'services/analytics_service.dart';
import 'services/entitlements_service.dart';
import 'services/settings_service.dart';
import 'services/app_state_refresh.dart';
import 'data/local_data_source.dart';
import 'providers/hub_connection_provider.dart';
import 'providers/room_provider.dart';
import 'providers/room_page_provider.dart';
import 'providers/home_provider.dart';
import 'providers/server_sync_provider.dart';
import 'providers/subscription_provider.dart';
import 'config/platform_capabilities.dart';
import 'config/supabase_config.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (kDebugMode) {
    RhythmSdk.enableLogging(level: Level.FINE);
  }

  final caps = PlatformCapabilities.fromPlatform();

  // Force portrait orientation by default on mobile
  // (Designer screen overrides this to landscape)
  if (!kIsWeb) {
    try {
      await SystemChrome.setPreferredOrientations([
        DeviceOrientation.portraitUp,
        DeviceOrientation.portraitDown,
        DeviceOrientation.landscapeLeft,
        DeviceOrientation.landscapeRight,
      ]);
    } catch (e) {
      debugPrint('Could not set orientation: $e');
    }
  }

  // Initialize SettingsService BEFORE Backend (for onboardingComplete check)
  // This also performs one-time migration from SharedPreferences to Hive
  await SettingsService.instance.initialize();

  // Initialize Backend (Supabase auth + analytics, or Offline)
  if (caps.hasCloudBackend) {
    try {
      if (SupabaseConfig.isConfigured) {
        await BackendProvider.initialize(BackendConfig.supabase(
          url: SupabaseConfig.url,
          anonKey: SupabaseConfig.anonKey,
          enableLogging: true,
        ));
        debugPrint('Backend initialized with Supabase');
      } else {
        await BackendProvider.initialize(BackendConfig.offline(
          enableLogging: true,
        ));
        debugPrint('Supabase not configured - running in offline mode');
      }
      // Initialize auth service for deep link handling
      await AuthService().initialize();
      // Initialize analytics
      await AnalyticsService().initialize();
    } catch (e) {
      debugPrint('Backend initialization failed: $e');
    }
  }

  // Entitlements: resolves the user's plan tier (Basic vs Pro). Always
  // bootstraps — for HA add-on (no cloud) this short-circuits to Pro.
  await EntitlementsService.bootstrap(caps);

  HybridApiClient? client;
  String? initError;

  try {
    // Try hybrid mode first (local brain + remote API)
    client = await HybridApiClient.create(
      storedHubs: LocalDataSource().getAllHubs(),
      syncSolarDataOnCreate: false,
    );

    // Fail explicitly if local brain isn't available (except on web,
    // where the app works as a remote client to rhythm-server/ESP32)
    if (!client.hasLocalBrain && !kIsWeb) {
      initError =
          'WASM brain failed to initialize. The Rust WASM module must be built and loaded for the app to function.';
    }
  } catch (e) {
    // If hybrid fails, try local-only mode
    try {
      debugPrint('Hybrid init failed, trying local-only: $e');
      client = await HybridApiClient.localOnly();
      if (!client.hasLocalBrain && !kIsWeb) {
        initError = 'WASM brain failed to initialize.';
      }
    } catch (e2) {
      if (kIsWeb) {
        // Web can proceed without local brain — it'll connect to a remote device
        debugPrint('Web: proceeding without local brain: $e2');
      } else {
        initError = 'Failed to initialize: $e2';
      }
    }
  }

  runApp(RhythmApp(
    client: client,
    initError: initError,
    capabilities: caps,
  ));
}

class RhythmApp extends StatelessWidget {
  final HybridApiClient? client;
  final String? initError;
  final PlatformCapabilities capabilities;

  const RhythmApp({
    super.key,
    this.client,
    this.initError,
    required this.capabilities,
  });

  @override
  Widget build(BuildContext context) {
    final theme = ThemeData(
      useMaterial3: true,
      brightness: Brightness.dark,
      colorScheme: ColorScheme.fromSeed(
        seedColor: const Color(0xFFF9A825),
        brightness: Brightness.dark,
      ),
      scaffoldBackgroundColor: const Color(0xFF1A1A2E),
      cardColor: const Color(0xFF16213E),
    );

    // Show error screen if initialization failed
    if (initError != null || client == null) {
      return MaterialApp(
        title: 'Rhythm Lighting - Error',
        debugShowCheckedModeBanner: false,
        theme: theme,
        home:
            _InitErrorScreen(error: initError ?? 'Client failed to initialize'),
      );
    }

    return MultiProvider(
      providers: [
        Provider<PlatformCapabilities>.value(value: capabilities),
        ChangeNotifierProvider(
          create: (_) => SubscriptionProvider(EntitlementsService.instance),
        ),
        ChangeNotifierProvider(create: (_) => ConfigModel()),
        Provider<RhythmApi>.value(value: client!),
        // Also expose the hybrid client directly for local brain access
        Provider<HybridApiClient>.value(value: client!),
        // Home and hub management (local storage)
        ChangeNotifierProvider(create: (_) => HomeProvider()..initialize()),
        // Hub connection management - wired to HomeProvider for hub data
        ChangeNotifierProxyProvider<HomeProvider, HubConnectionProvider>(
          create: (_) => HubConnectionProvider(),
          update: (_, homeProvider, hubConnection) {
            hubConnection?.configureHubs(homeProvider.currentHomeHubs);
            return hubConnection!;
          },
        ),
        // Room management (syncs rooms from Hue, HA, ESP32)
        ChangeNotifierProvider(create: (_) => RoomProvider()..initialize()),
        // Room page assignments (multi-screen room organization)
        ChangeNotifierProxyProvider<HomeProvider, RoomPageProvider>(
          create: (_) => RoomPageProvider(),
          update: (_, homeProvider, roomPageProvider) {
            roomPageProvider ??= RoomPageProvider();
            roomPageProvider.setLayoutScope(
              RoomPageProvider.layoutScopeFor(
                home: homeProvider.currentHome,
                hubs: homeProvider.currentHomeHubs,
              ),
            );
            return roomPageProvider;
          },
        ),
        // Server connection (transport layer — SDK)
        Provider(
            create: (_) => RhythmConnection(), dispose: (_, c) => c.dispose()),
        // Server sync provider (bridges SDK connection with app state)
        ChangeNotifierProxyProvider3<RhythmConnection, RoomProvider,
            HomeProvider, ServerSyncProvider>(
          lazy: false,
          create: (context) => ServerSyncProvider(
            connection: context.read<RhythmConnection>(),
            roomProvider: context.read<RoomProvider>(),
            homeProvider: context.read<HomeProvider>(),
          ),
          update: (_, connection, roomProvider, homeProvider, syncProvider) {
            syncProvider?.connectIfAvailable();
            return syncProvider!;
          },
        ),
      ],
      child: MaterialApp(
        title: 'Rhythm Lighting',
        debugShowCheckedModeBanner: false,
        theme: theme,
        home: AuthGate(key: AuthGate.globalKey),
      ),
    );
  }
}

/// Auth gate that checks auth state and onboarding completion.
class AuthGate extends StatefulWidget {
  const AuthGate({super.key});

  /// Global key to access AuthGate state for reset functionality.
  static final GlobalKey<State<AuthGate>> globalKey =
      GlobalKey<State<AuthGate>>();

  /// Stream controller for reset events
  static final _resetController = ValueNotifier<int>(0);

  /// Reset the app and show onboarding again.
  static void resetToOnboarding() {
    debugPrint('AuthGate.resetToOnboarding called');
    final state = globalKey.currentState;
    if (state case _AuthGateState authGateState) {
      debugPrint('Using GlobalKey to reset');
      authGateState._resetToOnboarding();
    } else {
      debugPrint('GlobalKey.currentState is null, using ValueNotifier');
      // Fallback: increment the notifier to trigger listeners
      _resetController.value++;
    }
  }

  @override
  State<AuthGate> createState() => _AuthGateState();
}

class _AuthGateState extends State<AuthGate> {
  bool _isLoading = true;
  bool _showOnboarding = true;
  int _onboardingKey = 0; // Used to force OnboardingFlow to recreate

  @override
  void initState() {
    super.initState();
    _checkAuthState();
    // Listen to reset events from static method
    AuthGate._resetController.addListener(_onResetRequested);
  }

  @override
  void dispose() {
    AuthGate._resetController.removeListener(_onResetRequested);
    super.dispose();
  }

  void _onResetRequested() {
    debugPrint('_onResetRequested triggered');
    _resetToOnboarding();
  }

  void _refreshAppStateInBackground() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      unawaited(
        AppStateRefresh.sync(context).then(
          (_) {},
          onError: (Object error, StackTrace stackTrace) {
            debugPrint('AuthGate: Background app state refresh failed: $error');
            debugPrint('$stackTrace');
          },
        ),
      );
    });
  }

  Future<void> _checkAuthState() async {
    // Web platform: skip to designer (no onboarding/auth)
    if (kIsWeb) {
      setState(() {
        _showOnboarding = false;
        _isLoading = false;
      });
      return;
    }

    // Check if onboarding was explicitly completed (from SettingsService/Hive)
    final onboardingComplete = SettingsService.instance.onboardingComplete;

    if (!BackendProvider.isInitialized) {
      // If Backend isn't initialized, just check local prefs
      setState(() {
        _showOnboarding = !onboardingComplete;
        _isLoading = false;
      });
      if (onboardingComplete) {
        _refreshAppStateInBackground();
      }
      return;
    }

    // If onboarding is complete, show the app
    if (onboardingComplete) {
      final authService = AuthService();
      if (authService.currentUserId != null) {
        AnalyticsService().identifyUser(authService.currentUserId!);
      }

      setState(() {
        _showOnboarding = false;
        _isLoading = false;
      });
      _refreshAppStateInBackground();
      return;
    }

    // Check if user exists in Keychain (persists across reinstalls/updates)
    final authService = AuthService();
    if (authService.currentUser != null) {
      // User exists - trust the Keychain session, they completed onboarding before
      debugPrint(
          'Recovering session from Keychain: ${authService.currentUserId}');

      // Identify user for analytics
      if (authService.currentUserId != null) {
        AnalyticsService().identifyUser(authService.currentUserId!);
      }

      // Track this recovery event
      AnalyticsService().logEvent('session_recovered', {
        'user_id': authService.currentUserId ?? 'unknown',
        'is_anonymous': authService.isAnonymous ? 1 : 0,
      });

      // Restore onboarding flag
      await SettingsService.instance.setOnboardingComplete(true);

      setState(() {
        _showOnboarding = false;
        _isLoading = false;
      });
      _refreshAppStateInBackground();
      return;
    }

    // No existing user - this is truly a fresh install
    // Create anonymous user for onboarding
    try {
      await authService.signInAnonymously();
      debugPrint('Created anonymous user: ${authService.currentUserId}');
      if (authService.currentUserId != null) {
        AnalyticsService().identifyUser(authService.currentUserId!);
      }
    } catch (e, stackTrace) {
      debugPrint('Anonymous auth failed: $e');
      debugPrint('  stackTrace: $stackTrace');
    }

    setState(() {
      _showOnboarding = true;
      _isLoading = false;
    });
  }

  Future<void> _completeOnboarding() async {
    // Mark onboarding as complete in Hive
    await SettingsService.instance.setOnboardingComplete(true);

    // Track onboarding completion
    AnalyticsService().logOnboardingCompleted();

    // Ensure we have a user (create anonymous if needed)
    final authService = AuthService();
    var user = authService.currentUser;
    if (user == null) {
      debugPrint(
          'No user at onboarding completion, creating anonymous user...');
      try {
        user = await authService.signInAnonymously();
        debugPrint('Created anonymous user: ${user?.id}');
      } catch (e) {
        debugPrint('Failed to create anonymous user: $e');
      }
    }

    // Identify user for analytics and set initial account status
    if (user != null) {
      AnalyticsService().identifyUser(user.id);
      AnalyticsService()
          .setAccountStatus(user.isAnonymous ? 'anonymous' : 'email');
    }

    // Initialize app state after onboarding
    if (!mounted) return;
    await AppStateRefresh.sync(context);
    if (!mounted) return;

    setState(() {
      _showOnboarding = false;
    });
  }

  /// Reset to onboarding flow (called from settings).
  void _resetToOnboarding() {
    debugPrint(
        '_resetToOnboarding: setting _showOnboarding=true, key=${_onboardingKey + 1}');
    setState(() {
      _showOnboarding = true;
      _onboardingKey++; // Force OnboardingFlow to recreate with fresh state
    });
  }

  @override
  Widget build(BuildContext context) {
    if (_isLoading) {
      return const Scaffold(
        backgroundColor: Color(0xFF0D1117),
        body: Center(
          child: CircularProgressIndicator(
            valueColor: AlwaysStoppedAnimation<Color>(Color(0xFFF9A825)),
          ),
        ),
      );
    }

    if (_showOnboarding) {
      return OnboardingFlow(
        key: ValueKey(_onboardingKey),
        onComplete: _completeOnboarding,
      );
    }

    return const AppShell();
  }
}

class _InitErrorScreen extends StatelessWidget {
  final String error;

  const _InitErrorScreen({required this.error});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(
                Icons.error_outline,
                size: 80,
                color: Colors.red.shade400,
              ),
              const SizedBox(height: 24),
              Text(
                'Initialization Failed',
                style: Theme.of(context).textTheme.headlineMedium?.copyWith(
                      color: Colors.white,
                      fontWeight: FontWeight.bold,
                    ),
              ),
              const SizedBox(height: 16),
              Text(
                error,
                style: const TextStyle(color: Colors.white70, fontSize: 16),
                textAlign: TextAlign.center,
              ),
              const SizedBox(height: 32),
              Container(
                padding: const EdgeInsets.all(16),
                decoration: BoxDecoration(
                  color: Colors.white.withValues(alpha: 0.05),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: Colors.white24),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: const [
                    Text(
                      'To fix this, build the WASM module:',
                      style: TextStyle(
                          color: Colors.white70, fontWeight: FontWeight.bold),
                    ),
                    SizedBox(height: 8),
                    Text(
                      '1. Install wasm-pack: cargo install wasm-pack\n'
                      '2. Build WASM: dart run flutter_rust_bridge build-web\n'
                      '3. Rebuild Flutter web: flutter build web',
                      style: TextStyle(
                          color: Colors.white54, fontFamily: 'monospace'),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
