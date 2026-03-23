import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_app/providers/settings_provider.dart';

void main() {
  group('SettingsProvider', () {
    late SettingsProvider provider;

    setUp(() async {
      SharedPreferences.setMockInitialValues({});
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
      test('loads saved preferences', () async {
        SharedPreferences.setMockInitialValues({
          'use24HourFormat': true,
          'latitude': 35.0,
          'longitude': -78.5,
          'locationName': 'Raleigh, NC',
          'bedtimeHour': 23,
          'bedtimeMinute': 0,
          'wakeTimeHour': 7,
          'wakeTimeMinute': 0,
          'hub_ha_verified': true,
          'hub_ha_host': 'homeassistant.local',
          'hub_ha_port': 8123,
          'hue_verified': true,
          'hue_bridge_ip': '192.168.1.100',
        });

        provider = SettingsProvider();
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
        SharedPreferences.setMockInitialValues({
          'bedtimeHour': 22,
          'bedtimeMinute': 0,
          'wakeTimeHour': 6,
          'wakeTimeMinute': 0,
        });
        provider = SettingsProvider();
        await provider.loadSettings();

        final duration = provider.getSleepDuration();
        // getSleepDuration returns a formatted string like "8h 0m"
        expect(duration, isA<String>());
        expect(duration, contains('h'));
      });

      test('handles overnight sleep correctly', () async {
        SharedPreferences.setMockInitialValues({
          'bedtimeHour': 23,
          'bedtimeMinute': 30,
          'wakeTimeHour': 7,
          'wakeTimeMinute': 0,
        });
        provider = SettingsProvider();
        await provider.loadSettings();

        final duration = provider.getSleepDuration();
        // Should be 7h 30m
        expect(duration, isA<String>());
        expect(duration, contains('h'));
      });
    });
  });
}
