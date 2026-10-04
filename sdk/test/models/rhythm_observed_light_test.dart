import 'dart:convert';
import 'dart:io';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test('unavailable and unknown readback cannot become observed off', () {
    for (final availability in [
      'unavailable',
      'unknown',
      'disconnected',
      'future'
    ]) {
      final light = RhythmObservedLight.maybeFromJson({
        'availability': availability,
        'lights_on': false,
        'received_at_epoch_ms': 1,
      })!;
      expect(light.currentLightsOn, isNull);
      expect(light.isAvailable, isFalse);
    }
  });

  test('polling emits an observation-only change and preserves desired output',
      () async {
    var brightness = 23;
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final connection = RhythmConnection();
    addTearDown(() async {
      connection.dispose();
      await server.close(force: true);
    });
    server.listen((request) async {
      if (request.uri.path != '/api/state' &&
          request.uri.path != '/api/nodes/state') {
        request.response.statusCode = 404;
      } else {
        request.response.headers.contentType = ContentType.json;
        request.response.write(jsonEncode({
          'version': '1.0.0',
          'nodes': [
            {
              'id': 'light-1',
              'kind': 'light_device',
              'name': 'Light',
              'state': 'active',
              'rhythm_enabled': true,
              'brightness': 80,
              'kelvin': 4000,
              'observed_light': {
                'availability': 'available',
                'lights_on': true,
                'brightness': brightness,
                'rgb': [255, 40, 12],
                'received_at_epoch_ms': brightness,
              },
            }
          ],
        }));
      }
      await request.response.close();
    });
    await connection.connect('127.0.0.1', port: server.port);
    brightness = 41;
    final event =
        connection.rhythmStateEvents.first.timeout(const Duration(seconds: 2));
    await connection.pollNow();
    final state = await event;
    expect(state.observedLight?.brightness, 41);
    expect(state.observedLight?.rgb, (255, 40, 12));
    expect(state.brightness, 80);
    expect(state.kelvin, 4000);
  });
}
