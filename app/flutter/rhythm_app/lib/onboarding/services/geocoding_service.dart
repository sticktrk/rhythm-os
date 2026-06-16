import 'dart:io';
import 'package:flutter/foundation.dart';
import 'package:geocoding/geocoding.dart';

/// A place result from geocoding.
class PlaceResult {
  final String city;
  final String? state;
  final String? country;
  final double latitude;
  final double longitude;

  const PlaceResult({
    required this.city,
    this.state,
    this.country,
    required this.latitude,
    required this.longitude,
  });

  /// Formatted short name for display (e.g., "Los Angeles, CA")
  String get shortName {
    if (state != null && state!.isNotEmpty) {
      return '$city, $state';
    } else if (country != null && country!.isNotEmpty) {
      return '$city, $country';
    }
    return city;
  }
}

/// Service for geocoding using platform APIs.
/// Only available on iOS and Android.
class GeocodingService {
  /// Whether geocoding is available on the current platform.
  static bool get isAvailable {
    if (kIsWeb) return false;
    return Platform.isIOS || Platform.isAndroid || Platform.isMacOS;
  }

  /// Search for places by address/city name.
  /// Returns empty list if not available on current platform.
  Future<List<PlaceResult>> searchPlaces(String query) async {
    if (!isAvailable || query.trim().length < 2) return [];

    try {
      final locations = await locationFromAddress(query);

      // Get place details for each location via reverse geocoding
      final results = <PlaceResult>[];
      for (final location in locations.take(5)) {
        final placemarks = await placemarkFromCoordinates(
          location.latitude,
          location.longitude,
        );

        if (placemarks.isNotEmpty) {
          final placemark = placemarks.first;
          final city = placemark.locality ??
              placemark.subAdministrativeArea ??
              placemark.administrativeArea ??
              query;

          results.add(PlaceResult(
            city: city,
            state: _formatState(placemark.administrativeArea, placemark.isoCountryCode),
            country: placemark.isoCountryCode != 'US' ? placemark.country : null,
            latitude: location.latitude,
            longitude: location.longitude,
          ));
        }
      }

      return results;
    } catch (e) {
      // Return empty list on error (no results found, network error, etc.)
      return [];
    }
  }

  /// Reverse geocode coordinates to get place name.
  Future<PlaceResult?> reverseGeocode(double latitude, double longitude) async {
    if (!isAvailable) return null;

    try {
      final placemarks = await placemarkFromCoordinates(latitude, longitude);

      if (placemarks.isNotEmpty) {
        final placemark = placemarks.first;
        final city = placemark.locality ??
            placemark.subAdministrativeArea ??
            placemark.administrativeArea ??
            'Unknown';

        return PlaceResult(
          city: city,
          state: _formatState(placemark.administrativeArea, placemark.isoCountryCode),
          country: placemark.isoCountryCode != 'US' ? placemark.country : null,
          latitude: latitude,
          longitude: longitude,
        );
      }
    } catch (e) {
      // Return null on error
    }
    return null;
  }

  /// Format state - use abbreviation for US states.
  static String? _formatState(String? state, String? countryCode) {
    if (state == null) return null;
    if (countryCode != 'US') return state;

    const stateAbbreviations = {
      'Alabama': 'AL', 'Alaska': 'AK', 'Arizona': 'AZ', 'Arkansas': 'AR',
      'California': 'CA', 'Colorado': 'CO', 'Connecticut': 'CT', 'Delaware': 'DE',
      'Florida': 'FL', 'Georgia': 'GA', 'Hawaii': 'HI', 'Idaho': 'ID',
      'Illinois': 'IL', 'Indiana': 'IN', 'Iowa': 'IA', 'Kansas': 'KS',
      'Kentucky': 'KY', 'Louisiana': 'LA', 'Maine': 'ME', 'Maryland': 'MD',
      'Massachusetts': 'MA', 'Michigan': 'MI', 'Minnesota': 'MN', 'Mississippi': 'MS',
      'Missouri': 'MO', 'Montana': 'MT', 'Nebraska': 'NE', 'Nevada': 'NV',
      'New Hampshire': 'NH', 'New Jersey': 'NJ', 'New Mexico': 'NM', 'New York': 'NY',
      'North Carolina': 'NC', 'North Dakota': 'ND', 'Ohio': 'OH', 'Oklahoma': 'OK',
      'Oregon': 'OR', 'Pennsylvania': 'PA', 'Rhode Island': 'RI', 'South Carolina': 'SC',
      'South Dakota': 'SD', 'Tennessee': 'TN', 'Texas': 'TX', 'Utah': 'UT',
      'Vermont': 'VT', 'Virginia': 'VA', 'Washington': 'WA', 'West Virginia': 'WV',
      'Wisconsin': 'WI', 'Wyoming': 'WY', 'District of Columbia': 'DC',
    };

    return stateAbbreviations[state] ?? state;
  }
}
