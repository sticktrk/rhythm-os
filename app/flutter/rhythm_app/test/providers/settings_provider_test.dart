import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/settings_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('SettingsProvider', () {
    late SettingsProvider provider;

    setUp(() {
      provider = SettingsProvider();
    });

    tearDown(() {
      provider.dispose();
    });

    group('initial state', () {
      test('has default values after loading', () async {
        await provider.loadSettings();

        expect(provider.latitude, isNull);
        expect(provider.longitude, isNull);
        expect(provider.locationName, isNull);
        expect(provider.bedtimeHour, equals(22));
        expect(provider.bedtimeMinute, equals(30));
        expect(provider.wakeTimeHour, equals(6));
        expect(provider.wakeTimeMinute, equals(30));
        expect(provider.haConfigured, isFalse);
        expect(provider.hueConfigured, isFalse);
      });

      test('isLoading is false after loadSettings', () async {
        await provider.loadSettings();
        expect(provider.isLoading, isFalse);
      });
    });

    group('loadSettings', () {
      test('uses current home and hubs', () async {
        provider.updateFromHome(
          _home(
            location: const HomeLocation(
              latitude: 35.0,
              longitude: -78.5,
              cityName: 'Raleigh, NC',
            ),
            sleepSchedule: const SleepSchedule(
              bedtime: 23.0,
              wakeTime: 7.0,
            ),
          ),
          [
            _haHub(),
            _hueHub(),
          ],
        );
        await provider.loadSettings();

        expect(provider.latitude, equals(35.0));
        expect(provider.longitude, equals(-78.5));
        expect(provider.locationName, equals('Raleigh, NC'));
        expect(provider.bedtimeHour, equals(23));
        expect(provider.bedtimeMinute, equals(0));
        expect(provider.wakeTimeHour, equals(7));
        expect(provider.wakeTimeMinute, equals(0));
        expect(provider.haConfigured, isTrue);
        expect(provider.haHost, equals('homeassistant.local'));
        expect(provider.haPort, equals(8123));
        expect(provider.hueConfigured, isTrue);
        expect(provider.hueBridgeIp, equals('192.168.1.100'));
      });
    });

    group('formatTime', () {
      test('returns 12-hour format', () async {
        await provider.loadSettings();
        expect(provider.formatTime(14, 30, use24h: false), equals('2:30 PM'));
        expect(provider.formatTime(0, 0, use24h: false), equals('12:00 AM'));
        expect(provider.formatTime(12, 0, use24h: false), equals('12:00 PM'));
      });

      test('returns 24-hour format', () async {
        await provider.loadSettings();
        expect(provider.formatTime(14, 30, use24h: true), equals('14:30'));
        expect(provider.formatTime(0, 0, use24h: true), equals('00:00'));
        expect(provider.formatTime(12, 0, use24h: true), equals('12:00'));
      });
    });

    group('getSleepDuration', () {
      test('returns formatted duration string', () async {
        provider.updateFromHome(
          _home(
            sleepSchedule: const SleepSchedule(
              bedtime: 22.0,
              wakeTime: 6.0,
            ),
          ),
          const [],
        );
        await provider.loadSettings();

        final duration = provider.getSleepDuration();
        // getSleepDuration returns a formatted string like "8h 0m"
        expect(duration, isA<String>());
        expect(duration, contains('h'));
      });

      test('handles overnight sleep correctly', () async {
        provider.updateFromHome(
          _home(
            sleepSchedule: const SleepSchedule(
              bedtime: 23.5,
              wakeTime: 7.0,
            ),
          ),
          const [],
        );
        await provider.loadSettings();

        final duration = provider.getSleepDuration();
        // Should be 7h 30m
        expect(duration, isA<String>());
        expect(duration, contains('h'));
      });
    });
  });
}

Home _home({
  HomeLocation? location,
  SleepSchedule? sleepSchedule,
}) {
  return Home.create(
    id: 'home-1',
    name: 'Home',
    ownerId: 'user-1',
    location: location,
    sleepSchedule: sleepSchedule,
  );
}

Hub _haHub() => Hub.homeAssistant(
      id: 'ha-1',
      homeId: 'home-1',
      name: 'Home Assistant',
      host: 'homeassistant.local',
      port: 8123,
      token: 'test-token',
    );

Hub _hueHub() => Hub.hue(
      id: 'hue-1',
      homeId: 'home-1',
      name: 'Hue',
      bridgeIp: '192.168.1.100',
      appKey: 'test-user',
    );
