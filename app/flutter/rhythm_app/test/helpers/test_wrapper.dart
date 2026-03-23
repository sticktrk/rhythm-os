import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_app/models/config_model.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/hub_connection_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../mocks/mock_rhythm_api.dart';

/// Initialize SharedPreferences for testing.
Future<void> setupTestPreferences([Map<String, Object>? values]) async {
  SharedPreferences.setMockInitialValues(values ?? {});
}

/// Build a testable widget wrapped with required providers.
///
/// Use this to test widgets that depend on providers.
Widget buildTestableWidget(
  Widget child, {
  ConfigModel? configModel,
  RoomProvider? roomProvider,
  HubConnectionProvider? hubConnectionProvider,
  RhythmApi? api,
}) {
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<ConfigModel>.value(
        value: configModel ?? ConfigModel(),
      ),
      ChangeNotifierProvider<RoomProvider>.value(
        value: roomProvider ?? RoomProvider(),
      ),
      ChangeNotifierProvider<HubConnectionProvider>.value(
        value: hubConnectionProvider ?? HubConnectionProvider(),
      ),
      Provider<RhythmApi>.value(
        value: api ?? MockRhythmApi(),
      ),
    ],
    child: MaterialApp(
      theme: ThemeData(
        useMaterial3: true,
        brightness: Brightness.dark,
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFFF9A825),
          brightness: Brightness.dark,
        ),
      ),
      home: Scaffold(body: child),
    ),
  );
}

/// Build a minimal testable widget without providers.
///
/// Use this for simple widget tests that don't need state management.
Widget buildMinimalWidget(Widget child) {
  return MaterialApp(
    theme: ThemeData(
      useMaterial3: true,
      brightness: Brightness.dark,
    ),
    home: Scaffold(body: child),
  );
}

/// Create a ConfigModel with pre-set values for testing.
ConfigModel createTestConfigModel({
  double selectedHour = 12.0,
  String activeHalf = 'morning',
  bool showSolarContext = true,
}) {
  final model = ConfigModel();
  model.setSelectedHour(selectedHour);
  model.setActiveHalf(activeHalf);
  model.setShowSolarContext(showSolarContext);
  return model;
}
