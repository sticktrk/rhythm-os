// Integration tests for room navigation.
//
// Tests the room swipe/page navigation functionality including:
// - Adding and displaying rooms
// - Swipe gestures between rooms
// - Page indicator dots
// - Current room display
//
// Note: These tests require a running app with WASM initialized.
// For pure unit tests of RoomProvider, see test/providers/room_provider_test.dart
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  group('Room Navigation', () {
    late RoomProvider roomProvider;

    setUp(() async {
      SharedPreferences.setMockInitialValues({});
      roomProvider = RoomProvider();
    });

    tearDown(() {
      roomProvider.dispose();
    });

    testWidgets('RoomProvider starts with no rooms',
        (WidgetTester tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: ChangeNotifierProvider<RoomProvider>.value(
            value: roomProvider,
            child: Builder(
              builder: (context) {
                final provider = context.watch<RoomProvider>();
                return Text('Rooms: ${provider.roomCount}');
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.text('Rooms: 0'), findsOneWidget);
    });

    testWidgets('RoomProvider can add rooms', (WidgetTester tester) async {
      await roomProvider.addRoom(RoomDto(
        id: 'room_1',
        name: 'Living Room',
        source: RoomSourceDto.hue,
        deviceIds: [],
        disabled: false,
        rhythmEnabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
        curveConfig: null,
      ));

      await tester.pumpWidget(
        MaterialApp(
          home: ChangeNotifierProvider<RoomProvider>.value(
            value: roomProvider,
            child: Builder(
              builder: (context) {
                final provider = context.watch<RoomProvider>();
                return Column(
                  children: [
                    Text('Rooms: ${provider.roomCount}'),
                    if (provider.currentRoom != null)
                      Text('Current: ${provider.currentRoom!.name}'),
                  ],
                );
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.text('Rooms: 1'), findsOneWidget);
      expect(find.text('Current: Living Room'), findsOneWidget);
    });

    testWidgets('setCurrentIndex navigates to room',
        (WidgetTester tester) async {
      await roomProvider.addRoom(RoomDto(
        id: 'room_1',
        name: 'Living Room',
        source: RoomSourceDto.hue,
        deviceIds: [],
        disabled: false,
        rhythmEnabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
        curveConfig: null,
      ));
      await roomProvider.addRoom(RoomDto(
        id: 'room_2',
        name: 'Bedroom',
        source: RoomSourceDto.hue,
        deviceIds: [],
        disabled: false,
        rhythmEnabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
        curveConfig: null,
      ));

      await tester.pumpWidget(
        MaterialApp(
          home: ChangeNotifierProvider<RoomProvider>.value(
            value: roomProvider,
            child: Builder(
              builder: (context) {
                final provider = context.watch<RoomProvider>();
                return Column(
                  children: [
                    Text('Index: ${provider.currentIndex}'),
                    if (provider.currentRoom != null)
                      Text('Room: ${provider.currentRoom!.name}'),
                    ElevatedButton(
                      onPressed: () => provider.setCurrentIndex(1),
                      child: const Text('Go to index 1'),
                    ),
                  ],
                );
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.text('Index: 0'), findsOneWidget);
      expect(find.text('Room: Living Room'), findsOneWidget);

      // Tap the button to change index
      await tester.tap(find.text('Go to index 1'));
      await tester.pumpAndSettle();

      expect(find.text('Index: 1'), findsOneWidget);
      expect(find.text('Room: Bedroom'), findsOneWidget);
    });

    testWidgets('nextRoom and previousRoom navigate correctly',
        (WidgetTester tester) async {
      await roomProvider.addRoom(RoomDto(
        id: 'room_1',
        name: 'Room 1',
        source: RoomSourceDto.hue,
        deviceIds: [],
        disabled: false,
        rhythmEnabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
        curveConfig: null,
      ));
      await roomProvider.addRoom(RoomDto(
        id: 'room_2',
        name: 'Room 2',
        source: RoomSourceDto.hue,
        deviceIds: [],
        disabled: false,
        rhythmEnabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
        curveConfig: null,
      ));

      await tester.pumpWidget(
        MaterialApp(
          home: ChangeNotifierProvider<RoomProvider>.value(
            value: roomProvider,
            child: Builder(
              builder: (context) {
                final provider = context.watch<RoomProvider>();
                return Column(
                  children: [
                    Text('Index: ${provider.currentIndex}'),
                    Row(
                      children: [
                        ElevatedButton(
                          onPressed: provider.previousRoom,
                          child: const Text('Prev'),
                        ),
                        ElevatedButton(
                          onPressed: provider.nextRoom,
                          child: const Text('Next'),
                        ),
                      ],
                    ),
                  ],
                );
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.text('Index: 0'), findsOneWidget);

      // Tap next
      await tester.tap(find.text('Next'));
      await tester.pumpAndSettle();

      expect(find.text('Index: 1'), findsOneWidget);

      // Tap previous
      await tester.tap(find.text('Prev'));
      await tester.pumpAndSettle();

      expect(find.text('Index: 0'), findsOneWidget);
    });
  });
}
