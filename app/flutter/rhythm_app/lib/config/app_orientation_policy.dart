import 'package:flutter/services.dart';

/// The application-wide orientation contract for mobile Flutter surfaces.
const List<DeviceOrientation> appPreferredOrientations = [
  DeviceOrientation.portraitUp,
];

/// Stable, low-cardinality value attached to existing startup analytics.
const String appOrientationPolicyAnalyticsValue = 'portrait_up';
