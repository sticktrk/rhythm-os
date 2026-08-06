import 'dart:async';
import 'dart:convert';

import 'package:app_links/app_links.dart';
import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:crypto/crypto.dart' as crypto;
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
import 'onboarding/screens/account_gate_screen.dart';
import 'onboarding/screens/password_recovery_screen.dart';
import 'services/auth_service.dart';
import 'services/analytics_service.dart';
import 'services/app_log_service.dart';
import 'services/entitlements_service.dart';
import 'services/recent_servers_service.dart';
import 'services/settings_service.dart';
import 'services/app_state_refresh.dart';
import 'services/app_startup_performance.dart';
import 'services/virtual_experience_service.dart';
import 'data/local_data_source.dart';
import 'providers/hub_connection_provider.dart';
import 'providers/room_provider.dart';
import 'providers/room_page_provider.dart';
import 'providers/home_provider.dart';
import 'providers/server_sync_provider.dart';
import 'providers/subscription_provider.dart';
import 'config/feature_flags.dart';
import 'config/platform_capabilities.dart';
import 'config/supabase_config.dart';
import 'widgets/hub_connection_loading_screen.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  AppLogService.instance.install();
  if (kDebugMode) {
    RhythmSdk.enableLogging(level: Level.FINE);
  }

  final caps = PlatformCapabilities.fromPlatform();
  AppStartupPerformance.instance.start();

  // Render a Flutter frame immediately so iOS/Android can release the native
  // launch screen while storage, account, and local-brain startup continues.
  runApp(RhythmBootstrap(capabilities: caps));

  // Force portrait orientation by default on mobile
  // (Designer screen overrides this to landscape)
  if (!kIsWeb) {
    unawaited(_configurePreferredOrientations());
  }
}

Future<void> _configurePreferredOrientations() async {
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

/// Values produced by startup before the provider graph can be built.
class RhythmStartupResult {
  final HybridApiClient? client;
  final String? initError;

  const RhythmStartupResult({this.client, this.initError});
}

typedef RhythmStartupInitializer = Future<RhythmStartupResult> Function(
  PlatformCapabilities capabilities,
);

/// Owns asynchronous startup after Flutter has already drawn its first frame.
class RhythmBootstrap extends StatefulWidget {
  final PlatformCapabilities capabilities;
  final RhythmStartupInitializer initializer;

  const RhythmBootstrap({
    super.key,
    required this.capabilities,
    this.initializer = initializeRhythmApp,
  });

  @override
  State<RhythmBootstrap> createState() => _RhythmBootstrapState();
}

class _RhythmBootstrapState extends State<RhythmBootstrap> {
  late final Future<RhythmStartupResult> _startup;

  @override
  void initState() {
    super.initState();
    _startup = widget.initializer(widget.capabilities);
  }

  @override
  Widget build(BuildContext context) {
    return FutureBuilder<RhythmStartupResult>(
      future: _startup,
      builder: (context, snapshot) {
        if (snapshot.connectionState != ConnectionState.done) {
          return const _StartupLoadingApp();
        }

        final result = snapshot.data;
        final String? initError;
        if (snapshot.hasError) {
          initError = 'Failed to initialize: ${snapshot.error}';
        } else if (result == null) {
          initError = 'Client failed to initialize';
        } else {
          initError = result.initError;
        }

        return RhythmApp(
          client: result?.client,
          initError: initError,
          capabilities: widget.capabilities,
        );
      },
    );
  }
}

class _StartupLoadingApp extends StatelessWidget {
  const _StartupLoadingApp();

  @override
  Widget build(BuildContext context) {
    return const MaterialApp(
      debugShowCheckedModeBanner: false,
      home: Scaffold(
        key: Key('rhythm_startup_loading'),
        backgroundColor: Color(0xFF0D1117),
        body: HubConnectionLoadingScreen(),
      ),
    );
  }
}

Future<RhythmStartupResult> initializeRhythmApp(
  PlatformCapabilities caps,
) async {
  // Initialize SettingsService BEFORE Backend (for onboardingComplete check)
  // This also performs one-time migration from SharedPreferences to Hive
  await SettingsService.instance.initialize();
  _startOptionalLocalServices();

  String? initError;

  // App builds with cloud capabilities require a real account backend. A
  // missing or broken Supabase configuration is a build error, not an
  // invitation to create an anonymous "offline" session: that session can
  // never satisfy the account gate. Demo mode is handled by the account UI
  // after the real backend has initialized.
  if (caps.hasCloudBackend) {
    if (!SupabaseConfig.isConfigured) {
      initError =
          'Account sign-in is not configured for this app build. Rebuild with valid Supabase credentials.';
      debugPrint(initError);
    } else {
      try {
        await BackendProvider.initialize(
          BackendConfig.supabase(
            url: SupabaseConfig.url,
            anonKey: SupabaseConfig.anonKey,
            enableLogging: true,
          ),
          initializeAnalytics: false,
        ).timeout(const Duration(seconds: 20));
        debugPrint('Backend initialized with Supabase');
        unawaited(_initializeAnalyticsInBackground());
      } catch (e) {
        initError =
            'Account sign-in could not be initialized. Check your connection and restart the app. ($e)';
        debugPrint('Backend initialization failed: $e');
      }
    }
  }

  // Entitlements are only a presentation prerequisite when enforcement is on.
  // With the current disabled default, expose the service immediately and let
  // its cloud/preferences work complete without delaying the provider graph.
  if (initError == null) {
    if (FeatureFlags.entitlementsEnabled) {
      await EntitlementsService.bootstrap(caps);
    } else {
      EntitlementsService.start(caps);
    }
  }

  HybridApiClient? client;

  if (initError == null) {
    try {
      // Try hybrid mode first (local brain + remote API)
      client = await HybridApiClient.create(
        storedHubs: LocalDataSource().getAllHubs(),
        syncSolarDataOnCreate: false,
      );

      // Fail explicitly if local brain isn't available (except on web,
      // where the app works as a remote client to rhythm-server)
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
  }

  return RhythmStartupResult(
    client: client,
    initError: initError,
  );
}

void _startOptionalLocalServices() {
  unawaited(
    AppLogService.instance.initializeStorage().catchError(
      (Object error, StackTrace stackTrace) {
        debugPrint('App log storage initialization failed: $error');
        debugPrint('$stackTrace');
      },
    ),
  );
  unawaited(
    RecentServersService.instance.initialize().catchError(
      (Object error, StackTrace stackTrace) {
        debugPrint('Recent server hydration failed: $error');
        debugPrint('$stackTrace');
      },
    ),
  );
}

Future<void> _initializeAnalyticsInBackground() async {
  try {
    await BackendProvider.initializeAnalytics();
    await AnalyticsService().initialize();
  } catch (error, stackTrace) {
    debugPrint('Analytics background initialization failed: $error');
    debugPrint('$stackTrace');
  }
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
        Provider<RhythmApi>.value(value: client!),
        // Also expose the hybrid client directly for local brain access
        Provider<HybridApiClient>.value(value: client!),
        // Home and hub management (local storage)
        ChangeNotifierProvider(create: (_) => HomeProvider()..initialize()),
        // Mirror the current Home's cached/server-accepted curve without an
        // extra startup `/api/state` read.
        ChangeNotifierProxyProvider<HomeProvider, ConfigModel>(
          create: (_) => ConfigModel(),
          update: (_, homeProvider, configModel) {
            configModel ??= ConfigModel();
            final curveConfig = homeProvider.currentHome?.curveConfig;
            if (curveConfig != null && curveConfig != configModel.config) {
              configModel.updateFromHomeCurveConfig(curveConfig);
            }
            return configModel;
          },
        ),
        // Hub connection management - wired to HomeProvider for hub data
        ChangeNotifierProxyProvider<HomeProvider, HubConnectionProvider>(
          create: (_) => HubConnectionProvider(),
          update: (_, homeProvider, hubConnection) {
            hubConnection?.configureHubs(homeProvider.currentHomeHubs);
            return hubConnection!;
          },
        ),
        // Room management (syncs rooms from Hue, HA, Rhythm bridge)
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
            connectivityCheck: Connectivity().checkConnectivity,
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

/// Auth gate that checks auth state before entering the app shell.
class AuthGate extends StatefulWidget {
  const AuthGate({super.key});

  /// Global key to access AuthGate state for reset functionality.
  static final GlobalKey<State<AuthGate>> globalKey =
      GlobalKey<State<AuthGate>>();

  /// Stream controller for reset events
  static final _resetController = ValueNotifier<int>(0);

  /// Reset the app back to the pre-home hardware gate.
  ///
  /// The name is retained for existing logout/delete/factory-reset callers.
  static void resetToOnboarding() {
    debugPrint('AuthGate.resetToOnboarding called');
    final state = globalKey.currentState;
    if (state case _AuthGateState authGateState) {
      debugPrint('Using GlobalKey to reset');
      unawaited(authGateState._resetToOnboarding());
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
  bool _requiresAccount = false;

  /// Bumped on sign-out/reset so the [AppShell] subtree is rebuilt from
  /// scratch — landing the user at the start of the hardware onboarding
  /// funnel instead of whatever tab or funnel step was showing before.
  int _shellGeneration = 0;

  /// The device had an anonymous session from before accounts became
  /// mandatory — the gate shows migration copy instead of first-run copy.
  bool _existingAnonymousUser = false;
  bool _showPasswordRecovery = false;
  StreamSubscription<AuthEvent>? _authEventSubscription;
  StreamSubscription<Uri>? _passwordRecoveryLinkSubscription;
  DateTime? _acceptPasswordRecoveryAuthEventUntil;

  @override
  void initState() {
    super.initState();
    _checkAuthState();
    _authEventSubscription = AuthService().authEvents.listen(_onAuthEvent);
    _listenForPasswordRecoveryLinks();
    // Listen to reset events from static method
    AuthGate._resetController.addListener(_onResetRequested);
    // Listen for Virtual Experience entry so demo state can be seeded whether
    // entry starts from the hardware gate or elsewhere in the app shell.
    VirtualExperienceService.instance.addListener(_onVirtualExperienceChanged);
  }

  @override
  void dispose() {
    _authEventSubscription?.cancel();
    _passwordRecoveryLinkSubscription?.cancel();
    AuthGate._resetController.removeListener(_onResetRequested);
    VirtualExperienceService.instance
        .removeListener(_onVirtualExperienceChanged);
    super.dispose();
  }

  void _onAuthEvent(AuthEvent event) {
    if (event != AuthEvent.passwordRecovery || !mounted) return;
    final acceptUntil = _acceptPasswordRecoveryAuthEventUntil;
    if (acceptUntil == null || DateTime.now().isAfter(acceptUntil)) {
      debugPrint('AuthGate: Ignoring stale password recovery auth event');
      return;
    }

    _showPasswordRecoveryScreen(source: 'auth_event');
  }

  void _listenForPasswordRecoveryLinks() {
    if (kIsWeb) return;

    final appLinks = AppLinks();
    _passwordRecoveryLinkSubscription = appLinks.uriLinkStream.listen(
      (uri) => unawaited(_handleIncomingLink(uri)),
      onError: (Object error, StackTrace stackTrace) {
        debugPrint('AuthGate: Password recovery link error: $error');
        debugPrint('$stackTrace');
      },
    );

    unawaited(_handleInitialLink(appLinks));
  }

  Future<void> _handleInitialLink(AppLinks appLinks) async {
    try {
      final uri = await appLinks.getInitialLink();
      if (uri != null) {
        await _handleIncomingLink(uri);
      }
    } on PlatformException catch (error, stackTrace) {
      debugPrint('AuthGate: Could not read initial app link: ${error.message}');
      debugPrint('$stackTrace');
    } catch (error, stackTrace) {
      debugPrint('AuthGate: Could not read initial app link: $error');
      debugPrint('$stackTrace');
    }
  }

  Future<void> _handleIncomingLink(Uri uri) async {
    if (!_isPasswordRecoveryLink(uri)) return;
    final fingerprint = _passwordRecoveryLinkFingerprint(uri);
    if (await SettingsService.instance
        .hasHandledPasswordRecoveryLink(fingerprint)) {
      debugPrint('AuthGate: Skipping already handled password recovery link');
      return;
    }
    await SettingsService.instance.markPasswordRecoveryLinkHandled(fingerprint);

    debugPrint('AuthGate: Password recovery link received: $uri');
    if (!_passwordRecoveryLinkHasSessionMaterial(uri)) {
      debugPrint('AuthGate: Ignoring password recovery link without session');
      return;
    }

    _acceptPasswordRecoveryAuthEventUntil =
        DateTime.now().add(const Duration(seconds: 10));
    final exchanged = await _exchangePasswordRecoveryLink(uri);
    if (!exchanged) {
      debugPrint('AuthGate: Password recovery link did not create a session');
      return;
    }
    _showPasswordRecoveryScreen(source: 'deep_link');
  }

  bool _passwordRecoveryLinkHasSessionMaterial(Uri uri) {
    return (uri.queryParameters['token_hash']?.isNotEmpty ?? false) ||
        (uri.queryParameters['tokenHash']?.isNotEmpty ?? false) ||
        (uri.queryParameters['code']?.isNotEmpty ?? false) ||
        uri.fragment.isNotEmpty;
  }

  String _passwordRecoveryLinkFingerprint(Uri uri) {
    final tokenHash =
        uri.queryParameters['token_hash'] ?? uri.queryParameters['tokenHash'];
    final code = uri.queryParameters['code'];
    final material = tokenHash != null && tokenHash.isNotEmpty
        ? 'token_hash:$tokenHash'
        : code != null && code.isNotEmpty
            ? 'code:$code'
            : uri.toString();
    return crypto.sha256.convert(utf8.encode(material)).toString();
  }

  bool _isPasswordRecoveryLink(Uri uri) {
    final host = uri.host.toLowerCase();
    final path = uri.path.toLowerCase();
    return (uri.scheme == 'rhythmapp' || uri.scheme == 'lighting.rhythm.app') &&
        (host == 'password-reset' || path == '/password-reset');
  }

  Future<bool> _exchangePasswordRecoveryLink(Uri uri) async {
    if (!BackendProvider.isInitialized) return false;

    final tokenHash =
        uri.queryParameters['token_hash'] ?? uri.queryParameters['tokenHash'];
    if (tokenHash != null && tokenHash.isNotEmpty) {
      try {
        await AuthService().verifyPasswordRecoveryTokenHash(tokenHash);
        debugPrint('AuthGate: Password recovery token verified');
        return true;
      } catch (error, stackTrace) {
        debugPrint(
            'AuthGate: Password recovery token verification failed: $error');
        debugPrint('$stackTrace');
        return false;
      }
    }

    final auth = BackendProvider.instance.auth;
    if (auth is! SupabaseAuthBackend) return false;

    try {
      await auth.client.auth.getSessionFromUrl(uri);
      debugPrint('AuthGate: Password recovery session exchanged');
      return true;
    } catch (error, stackTrace) {
      debugPrint('AuthGate: Password recovery session exchange failed: $error');
      debugPrint('$stackTrace');
      return false;
    }
  }

  void _showPasswordRecoveryScreen({required String source}) {
    if (!mounted || _showPasswordRecovery) return;

    debugPrint('AuthGate: Showing password recovery screen from $source');
    unawaited(AnalyticsService().logScreenView('password_recovery'));
    setState(() {
      _showPasswordRecovery = true;
      _isLoading = false;
    });
  }

  void _onVirtualExperienceChanged() {
    final active = VirtualExperienceService.instance.isActive;
    if (active) {
      unawaited(_enterVirtualExperience());
    }
  }

  void _onResetRequested() {
    debugPrint('_onResetRequested triggered');
    unawaited(_resetToOnboarding());
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
    // Web platform: skip auth bootstrap and show the app shell.
    if (kIsWeb) {
      await SettingsService.instance.setOnboardingComplete(true);
      if (!mounted) return;
      setState(() {
        _isLoading = false;
      });
      _refreshAppStateInBackground();
      return;
    }

    if (!BackendProvider.isInitialized) {
      await SettingsService.instance.setOnboardingComplete(true);
      if (!mounted) return;
      setState(() {
        _isLoading = false;
      });
      _refreshAppStateInBackground();
      return;
    }

    final authService = AuthService();
    if (authService.currentUser != null) {
      debugPrint(
          'Recovering session from Keychain: ${authService.currentUserId}');

      // A recovered anonymous session predates the account requirement:
      // this user chose (or defaulted to) local-only before it was removed.
      _existingAnonymousUser = authService.isAnonymous;

      if (authService.currentUserId != null) {
        AnalyticsService().identifyUser(authService.currentUserId!);
      }

      AnalyticsService().logEvent('session_recovered', {
        'user_id': authService.currentUserId ?? 'unknown',
        'is_anonymous': authService.isAnonymous ? 1 : 0,
      });
    }

    await SettingsService.instance.setOnboardingComplete(true);
    if (!mounted) return;
    setState(() {
      _isLoading = false;
      _requiresAccount = _computeRequiresAccount();
    });
    _refreshAppStateInBackground();
  }

  /// Whether the account gate must be shown before entering the app.
  ///
  /// The launch gate exists to migrate legacy anonymous sessions to a real
  /// account, so it fires only when such a session was recovered. Fresh
  /// installs (no session) go through the hardware onboarding funnel first;
  /// [HardwareOnboardingGate] enforces sign-in before the connect step, so
  /// there is still no path into a paired app without an account. Web and
  /// no-backend (HA add-on) builds have no account infrastructure and are
  /// exempt.
  bool _computeRequiresAccount() {
    if (kIsWeb || !BackendProvider.isInitialized) return false;
    final authService = AuthService();
    return authService.currentUser != null && authService.isAnonymous;
  }

  Future<void> _onAccountGateSignedIn() async {
    await SettingsService.instance.setOnboardingComplete(true);

    final authService = AuthService();
    final userId = authService.currentUserId;
    if (userId != null) {
      AnalyticsService().identifyUser(userId);
      AnalyticsService()
          .setAccountStatus(authService.isAnonymous ? 'anonymous' : 'email');
    }

    if (!mounted) return;
    setState(() {
      _requiresAccount = _computeRequiresAccount();
      if (!_requiresAccount) _existingAnonymousUser = false;
    });
    _refreshAppStateInBackground();
  }

  /// Drop the user into [AppShell] in Virtual Experience mode.
  Future<void> _enterVirtualExperience() async {
    AnalyticsService().logEvent('virtual_experience_entered');
    if (!mounted) return;
    // Seed the demo rooms. Safe whether we were already inside AppShell or
    // the request arrived while AuthGate was still bootstrapping.
    await AppStateRefresh.sync(context);
  }

  Future<void> _completePasswordRecovery() async {
    await SettingsService.instance.setOnboardingComplete(true);

    final authService = AuthService();
    final userId = authService.currentUserId;
    if (userId != null) {
      AnalyticsService().identifyUser(userId);
      AnalyticsService()
          .setAccountStatus(authService.isAnonymous ? 'anonymous' : 'email');
    }

    if (!mounted) return;
    setState(() {
      _showPasswordRecovery = false;
      _isLoading = false;
      _requiresAccount = _computeRequiresAccount();
    });
    _refreshAppStateInBackground();
  }

  /// Reset to the clean pre-home hardware gate (called from settings).
  Future<void> _resetToOnboarding() async {
    debugPrint('_resetToOnboarding: resetting to clean hardware gate');
    await SettingsService.instance.setOnboardingComplete(true);

    if (!mounted) return;
    setState(() {
      _showPasswordRecovery = false;
      _isLoading = false;
      _requiresAccount = _computeRequiresAccount();
      // Post-logout/reset local state is wiped — the gate shows first-run
      // copy, not the migration prompt.
      _existingAnonymousUser = false;
      _shellGeneration++;
    });
    _refreshAppStateInBackground();
  }

  @override
  Widget build(BuildContext context) {
    if (_showPasswordRecovery) {
      return PasswordRecoveryScreen(onComplete: _completePasswordRecovery);
    }

    if (_isLoading) {
      return const HubConnectionLoadingScreen();
    }

    if (_requiresAccount) {
      return AccountGateScreen(
        onSignedIn: _onAccountGateSignedIn,
        existingUser: _existingAnonymousUser,
      );
    }

    return AppShell(key: ValueKey('shell_$_shellGeneration'));
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
