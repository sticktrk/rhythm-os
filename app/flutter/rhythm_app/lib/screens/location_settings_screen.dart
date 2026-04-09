import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter_timezone/flutter_timezone.dart';
import 'package:geolocator/geolocator.dart';
import 'package:geocoding/geocoding.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';

/// Location settings screen with clear permission guidance.
/// GPS-only approach with excellent UX for handling permission states.
class LocationSettingsScreen extends StatefulWidget {
  const LocationSettingsScreen({super.key});

  static Future<bool?> show(BuildContext context) {
    return Navigator.of(context).push<bool>(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const LocationSettingsScreen();
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<LocationSettingsScreen> createState() => _LocationSettingsScreenState();
}

class _LocationSettingsScreenState extends State<LocationSettingsScreen>
    with TickerProviderStateMixin {
  double? _latitude;
  double? _longitude;
  String? _locationName;

  _PermissionState _permissionState = _PermissionState.unknown;
  bool _isLoading = false;
  String? _statusMessage;

  late AnimationController _pulseController;
  late AnimationController _orbitController;
  late Animation<double> _pulseAnimation;

  @override
  void initState() {
    super.initState();
    _loadCurrentLocation();
    _checkPermissionStatus();

    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 2500),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 0.4, end: 0.8).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );

    _orbitController = AnimationController(
      duration: const Duration(seconds: 8),
      vsync: this,
    )..repeat();
  }

  @override
  void dispose() {
    _pulseController.dispose();
    _orbitController.dispose();
    super.dispose();
  }

  Future<void> _loadCurrentLocation() async {
    final homeProvider = context.read<HomeProvider>();
    final home = homeProvider.currentHome;
    if (home?.location != null) {
      setState(() {
        _latitude = home!.location!.latitude;
        _longitude = home.location!.longitude;
        _locationName = home.location!.cityName;
      });
    }
  }

  Future<void> _checkPermissionStatus() async {
    final serviceEnabled = await Geolocator.isLocationServiceEnabled();
    if (!serviceEnabled) {
      setState(() => _permissionState = _PermissionState.serviceDisabled);
      return;
    }

    final permission = await Geolocator.checkPermission();
    setState(() {
      _permissionState = switch (permission) {
        LocationPermission.denied => _PermissionState.denied,
        LocationPermission.deniedForever => _PermissionState.deniedForever,
        LocationPermission.whileInUse ||
        LocationPermission.always =>
          _PermissionState.granted,
        LocationPermission.unableToDetermine => _PermissionState.unknown,
      };
    });

    // Auto-detect if permission already granted and no location set
    if (_permissionState == _PermissionState.granted && _latitude == null) {
      _detectLocation();
    }
  }

  Future<void> _requestPermissionAndDetect() async {
    setState(() {
      _isLoading = true;
      _statusMessage = null;
    });

    try {
      final serviceEnabled = await Geolocator.isLocationServiceEnabled();
      if (!serviceEnabled) {
        setState(() {
          _permissionState = _PermissionState.serviceDisabled;
          _statusMessage = 'Please enable location services';
          _isLoading = false;
        });
        return;
      }

      var permission = await Geolocator.checkPermission();
      if (permission == LocationPermission.denied) {
        permission = await Geolocator.requestPermission();
      }

      if (permission == LocationPermission.denied) {
        setState(() {
          _permissionState = _PermissionState.denied;
          _statusMessage = 'Location permission is required';
          _isLoading = false;
        });
        return;
      }

      if (permission == LocationPermission.deniedForever) {
        setState(() {
          _permissionState = _PermissionState.deniedForever;
          _statusMessage = 'Please enable in Settings';
          _isLoading = false;
        });
        return;
      }

      setState(() => _permissionState = _PermissionState.granted);
      await _detectLocation();
    } catch (e) {
      setState(() {
        _statusMessage = 'Something went wrong';
        _isLoading = false;
      });
    }
  }

  Future<void> _detectLocation() async {
    final homeProvider = context.read<HomeProvider>();
    final serverSyncProvider = context.read<ServerSyncProvider>();
    setState(() {
      _isLoading = true;
      _statusMessage = 'Finding your location...';
    });

    try {
      final position = await Geolocator.getCurrentPosition(
        locationSettings: const LocationSettings(
          accuracy: LocationAccuracy.low,
          timeLimit: Duration(seconds: 15),
        ),
      );

      final timezone = (await FlutterTimezone.getLocalTimezone()).identifier;

      // Try to get place name via reverse geocoding (uses device's native geocoder)
      String? placeName;
      try {
        final placemarks = await placemarkFromCoordinates(
          position.latitude,
          position.longitude,
        );
        if (placemarks.isNotEmpty) {
          final place = placemarks.first;
          // Format as "City, State" or "City, Country"
          final city = place.locality ?? place.subAdministrativeArea;
          final region = place.administrativeArea ?? place.country;
          if (city != null && region != null) {
            placeName = '$city, $region';
          } else if (city != null) {
            placeName = city;
          } else if (region != null) {
            placeName = region;
          }
        }
      } catch (_) {
        // Geocoding failed silently - coordinates still work
      }

      if (!mounted) return;

      // Save to HomeProvider (syncs to cloud)
      final location = HomeLocation(
        latitude: position.latitude,
        longitude: position.longitude,
        cityName: placeName,
      );
      await homeProvider.updateCurrentHomeLocation(location);

      // Also update timezone on the home
      if (homeProvider.currentHome != null) {
        final updatedHome = homeProvider.currentHome!.copyWith(
          timezone: timezone,
          updatedAt: DateTime.now(),
          pendingSync: true,
        );
        await homeProvider.updateCurrentHome(updatedHome);
      }

      // Push to connected server (explicit user action — this is the only
      // path that should override the server's house location).
      serverSyncProvider.pushLocation(
        lat: position.latitude,
        lon: position.longitude,
        timezoneName: timezone,
      );

      setState(() {
        _latitude = position.latitude;
        _longitude = position.longitude;
        _locationName = placeName;
        _statusMessage = 'Location updated';
        _isLoading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _statusMessage = 'Could not get location';
        _isLoading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _Palette.bg,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(24, 8, 24, 40),
                child: Column(
                  children: [
                    _buildVisualization(),
                    const SizedBox(height: 32),
                    _buildMainCard(),
                    const SizedBox(height: 20),
                    _buildInfoCard(),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(_latitude != null),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _Palette.accent.withValues(alpha: 0.12),
                border: Border.all(
                  color: _Palette.accent.withValues(alpha: 0.25),
                ),
              ),
              child: const Icon(Icons.close, color: _Palette.accent, size: 20),
            ),
          ),
          const Expanded(
            child: Text(
              'Location',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _Palette.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildVisualization() {
    final hasLocation = _latitude != null && _longitude != null;

    return AnimatedBuilder(
      animation: _pulseAnimation,
      builder: (context, child) {
        return Container(
          height: 200,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(24),
            gradient: RadialGradient(
              center: Alignment.center,
              radius: 0.8,
              colors: [
                (hasLocation ? _Palette.success : _Palette.warm)
                    .withValues(alpha: _pulseAnimation.value * 0.08),
                _Palette.card.withValues(alpha: 0.5),
                _Palette.bg,
              ],
            ),
          ),
          child: Stack(
            alignment: Alignment.center,
            children: [
              // Orbit ring
              AnimatedBuilder(
                animation: _orbitController,
                builder: (context, _) {
                  return CustomPaint(
                    size: const Size(180, 180),
                    painter: _OrbitPainter(
                      progress: _orbitController.value,
                      hasLocation: hasLocation,
                    ),
                  );
                },
              ),
              // Center content
              Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  // Location marker
                  AnimatedContainer(
                    duration: const Duration(milliseconds: 400),
                    width: 56,
                    height: 56,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: hasLocation
                          ? _Palette.success.withValues(alpha: 0.15)
                          : _Palette.warm.withValues(alpha: 0.12),
                      border: Border.all(
                        color: hasLocation
                            ? _Palette.success.withValues(alpha: 0.4)
                            : _Palette.warm.withValues(alpha: 0.3),
                        width: 2,
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: (hasLocation
                                  ? _Palette.success
                                  : _Palette.warm)
                              .withValues(alpha: _pulseAnimation.value * 0.4),
                          blurRadius: 20,
                          spreadRadius: 0,
                        ),
                      ],
                    ),
                    child: Icon(
                      hasLocation
                          ? Icons.location_on_rounded
                          : Icons.location_searching_rounded,
                      color: hasLocation ? _Palette.success : _Palette.warm,
                      size: 26,
                    ),
                  ),
                  const SizedBox(height: 16),
                  // Location name and coordinates
                  if (hasLocation) ...[
                    if (_locationName != null) ...[
                      Text(
                        _locationName!,
                        style: const TextStyle(
                          color: _Palette.textPrimary,
                          fontSize: 18,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                      const SizedBox(height: 8),
                      Text(
                        '${_formatCoordinate(_latitude!, true)}  ${_formatCoordinate(_longitude!, false)}',
                        style: TextStyle(
                          color: _Palette.textSecondary.withValues(alpha: 0.7),
                          fontSize: 12,
                          fontWeight: FontWeight.w400,
                          fontFamily: 'monospace',
                          letterSpacing: 1,
                        ),
                      ),
                    ] else ...[
                      Text(
                        _formatCoordinate(_latitude!, true),
                        style: const TextStyle(
                          color: _Palette.textPrimary,
                          fontSize: 20,
                          fontWeight: FontWeight.w300,
                          fontFamily: 'monospace',
                          letterSpacing: 1.5,
                        ),
                      ),
                      const SizedBox(height: 4),
                      Text(
                        _formatCoordinate(_longitude!, false),
                        style: TextStyle(
                          color: _Palette.textPrimary.withValues(alpha: 0.7),
                          fontSize: 20,
                          fontWeight: FontWeight.w300,
                          fontFamily: 'monospace',
                          letterSpacing: 1.5,
                        ),
                      ),
                    ],
                  ] else
                    Text(
                      'No location set',
                      style: TextStyle(
                        color: _Palette.textSecondary.withValues(alpha: 0.7),
                        fontSize: 14,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                ],
              ),
            ],
          ),
        );
      },
    );
  }

  String _formatCoordinate(double value, bool isLat) {
    final dir = isLat ? (value >= 0 ? 'N' : 'S') : (value >= 0 ? 'E' : 'W');
    return '${value.abs().toStringAsFixed(4)}° $dir';
  }

  Widget _buildMainCard() {
    return Container(
      padding: const EdgeInsets.all(24),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        children: [
          _buildStatusSection(),
          const SizedBox(height: 24),
          _buildActionButton(),
          if (_statusMessage != null) ...[
            const SizedBox(height: 16),
            _buildStatusMessage(),
          ],
        ],
      ),
    );
  }

  Widget _buildStatusSection() {
    final (icon, color, title, subtitle) = _getStatusContent();

    return Row(
      children: [
        Container(
          width: 48,
          height: 48,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: color.withValues(alpha: 0.12),
          ),
          child: Icon(icon, color: color, size: 24),
        ),
        const SizedBox(width: 16),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                style: const TextStyle(
                  color: _Palette.textPrimary,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 3),
              Text(
                subtitle,
                style: TextStyle(
                  color: _Palette.textSecondary,
                  fontSize: 13,
                  height: 1.3,
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }

  (IconData, Color, String, String) _getStatusContent() {
    if (_latitude != null) {
      return (
        Icons.check_circle_rounded,
        _Palette.success,
        'Location Set',
        'Sunrise and sunset times will be calculated for your area',
      );
    }

    return switch (_permissionState) {
      _PermissionState.granted => (
          Icons.gps_fixed_rounded,
          _Palette.accent,
          'Ready to Detect',
          'Tap below to find your current location',
        ),
      _PermissionState.denied => (
          Icons.location_disabled_rounded,
          _Palette.warm,
          'Permission Needed',
          'Rhythm needs location access to calculate sunrise and sunset',
        ),
      _PermissionState.deniedForever => (
          Icons.settings_rounded,
          _Palette.error,
          'Permission Blocked',
          'Open your device Settings to enable location for Rhythm',
        ),
      _PermissionState.serviceDisabled => (
          Icons.location_off_rounded,
          _Palette.error,
          'Location Services Off',
          'Turn on Location Services in your device settings',
        ),
      _PermissionState.unknown => (
          Icons.hourglass_empty_rounded,
          _Palette.textSecondary,
          'Checking...',
          'Determining location permission status',
        ),
    };
  }

  Widget _buildActionButton() {
    final hasLocation = _latitude != null;
    final (label, onTap, color) = _getButtonContent();

    return GestureDetector(
      onTap: _isLoading ? null : onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 250),
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          gradient: LinearGradient(
            colors: [
              color,
              color.withValues(alpha: 0.85),
            ],
          ),
          borderRadius: BorderRadius.circular(14),
          boxShadow: [
            BoxShadow(
              color: color.withValues(alpha: 0.35),
              blurRadius: 16,
              offset: const Offset(0, 6),
            ),
          ],
        ),
        child: Center(
          child: _isLoading
              ? SizedBox(
                  width: 22,
                  height: 22,
                  child: CircularProgressIndicator(
                    strokeWidth: 2.5,
                    valueColor: AlwaysStoppedAnimation(
                      hasLocation ? Colors.white : const Color(0xFF0D1117),
                    ),
                  ),
                )
              : Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      _getButtonIcon(),
                      color: _getButtonTextColor(),
                      size: 20,
                    ),
                    const SizedBox(width: 10),
                    Text(
                      label,
                      style: TextStyle(
                        color: _getButtonTextColor(),
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ],
                ),
        ),
      ),
    );
  }

  (String, VoidCallback?, Color) _getButtonContent() {
    if (_latitude != null && _permissionState == _PermissionState.granted) {
      return ('Update Location', _detectLocation, _Palette.accent);
    }

    return switch (_permissionState) {
      _PermissionState.granted => (
          'Detect My Location',
          _detectLocation,
          _Palette.warm,
        ),
      _PermissionState.deniedForever => (
          'Open Settings',
          () => Geolocator.openAppSettings(),
          _Palette.accent,
        ),
      _PermissionState.serviceDisabled => (
          'Open Location Settings',
          () => Geolocator.openLocationSettings(),
          _Palette.accent,
        ),
      _ => (
          'Enable Location',
          _requestPermissionAndDetect,
          _Palette.warm,
        ),
    };
  }

  IconData _getButtonIcon() {
    if (_latitude != null) return Icons.refresh_rounded;

    return switch (_permissionState) {
      _PermissionState.granted => Icons.my_location_rounded,
      _PermissionState.deniedForever ||
      _PermissionState.serviceDisabled =>
        Icons.open_in_new_rounded,
      _ => Icons.location_searching_rounded,
    };
  }

  Color _getButtonTextColor() {
    if (_latitude != null) return Colors.white;
    return const Color(0xFF0D1117);
  }

  Widget _buildStatusMessage() {
    final isError = _statusMessage?.contains('could not') == true ||
        _statusMessage?.contains('wrong') == true ||
        _statusMessage?.contains('required') == true ||
        _statusMessage?.contains('enable') == true;

    final isSuccess = _statusMessage?.contains('updated') == true;

    final color = isSuccess
        ? _Palette.success
        : isError
            ? _Palette.error
            : _Palette.accent;

    return AnimatedOpacity(
      opacity: 1.0,
      duration: const Duration(milliseconds: 200),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          color: color.withValues(alpha: 0.1),
          borderRadius: BorderRadius.circular(10),
          border: Border.all(color: color.withValues(alpha: 0.2)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              isSuccess
                  ? Icons.check_rounded
                  : isError
                      ? Icons.error_outline_rounded
                      : Icons.info_outline_rounded,
              color: color,
              size: 16,
            ),
            const SizedBox(width: 8),
            Text(
              _statusMessage!,
              style: TextStyle(
                color: color,
                fontSize: 13,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildInfoCard() {
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: _Palette.accent.withValues(alpha: 0.04),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: _Palette.accent.withValues(alpha: 0.08)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            Icons.wb_sunny_rounded,
            color: _Palette.warm.withValues(alpha: 0.7),
            size: 18,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              'Your location calculates local sunrise and sunset times so Rhythm can adjust your lighting throughout the day.',
              style: TextStyle(
                color: _Palette.textSecondary,
                fontSize: 12,
                height: 1.5,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _Palette {
  static const bg = Color(0xFF0B0E13);
  static const card = Color(0xFF13171E);
  static const border = Color(0xFF232A35);
  static const textPrimary = Color(0xFFE8EDF4);
  static const textSecondary = Color(0xFF8A919C);
  static const accent = Color(0xFF58A6FF);
  static const warm = Color(0xFFF9A825);
  static const success = Color(0xFF4ADE80);
  static const error = Color(0xFFF87171);
}

enum _PermissionState {
  unknown,
  granted,
  denied,
  deniedForever,
  serviceDisabled,
}

class _OrbitPainter extends CustomPainter {
  final double progress;
  final bool hasLocation;

  _OrbitPainter({required this.progress, required this.hasLocation});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2 - 8;

    // Orbit ring
    final ringPaint = Paint()
      ..color = _Palette.border.withValues(alpha: 0.4)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1;
    canvas.drawCircle(center, radius, ringPaint);

    // Animated dot
    final angle = progress * 2 * math.pi - math.pi / 2;
    final dotPos = Offset(
      center.dx + radius * math.cos(angle),
      center.dy + radius * math.sin(angle),
    );

    final dotPaint = Paint()
      ..color = hasLocation ? _Palette.success : _Palette.warm
      ..style = PaintingStyle.fill;
    canvas.drawCircle(dotPos, 4, dotPaint);

    // Glow
    final glowPaint = Paint()
      ..color = (hasLocation ? _Palette.success : _Palette.warm)
          .withValues(alpha: 0.3)
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6);
    canvas.drawCircle(dotPos, 6, glowPaint);
  }

  @override
  bool shouldRepaint(covariant _OrbitPainter oldDelegate) =>
      oldDelegate.progress != progress ||
      oldDelegate.hasLocation != hasLocation;
}
