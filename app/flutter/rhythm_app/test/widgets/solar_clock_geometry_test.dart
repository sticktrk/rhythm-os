import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/solar_clock/solar_clock_geometry.dart';

void main() {
  group('SolarClockGeometry', () {
    test('standard24 orientation keeps noon at the top of the dial', () {
      const geometry = SolarClockGeometry(
        center: Offset.zero,
        radius: 10,
        solarNoon: 13.4,
        orientation: SolarClockOrientation.standard24,
      );

      expect(geometry.positionForHour(12).dx, closeTo(0, 0.001));
      expect(geometry.positionForHour(12).dy, closeTo(-10, 0.001));

      expect(geometry.positionForHour(18).dx, closeTo(10, 0.001));
      expect(geometry.positionForHour(18).dy, closeTo(0, 0.001));

      expect(geometry.positionForHour(0).dx, closeTo(0, 0.001));
      expect(geometry.positionForHour(0).dy, closeTo(10, 0.001));

      expect(geometry.positionForHour(6).dx, closeTo(-10, 0.001));
      expect(geometry.positionForHour(6).dy, closeTo(0, 0.001));
    });

    test('solar aligned orientation still places solar noon at the top', () {
      const geometry = SolarClockGeometry(
        center: Offset.zero,
        radius: 10,
        solarNoon: 13.5,
      );

      expect(geometry.positionForHour(13.5).dx, closeTo(0, 0.001));
      expect(geometry.positionForHour(13.5).dy, closeTo(-10, 0.001));
      expect(
        geometry.hourFromPosition(const Offset(0, -10)),
        closeTo(13.5, 0.001),
      );
    });

    test('standard24 orientation round-trips clock positions back to hours',
        () {
      const geometry = SolarClockGeometry(
        center: Offset.zero,
        radius: 10,
        solarNoon: 12.8,
        orientation: SolarClockOrientation.standard24,
      );

      for (final hour in [0.0, 3.0, 6.0, 12.0, 18.0, 23.5]) {
        final position = geometry.positionForHour(hour);
        expect(geometry.hourFromPosition(position), closeTo(hour, 0.001));
      }
    });
  });
}
