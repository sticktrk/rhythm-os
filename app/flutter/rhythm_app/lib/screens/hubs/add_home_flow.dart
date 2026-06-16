import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_timezone/flutter_timezone.dart';
import 'package:geolocator/geolocator.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../../onboarding/services/geocoding_service.dart';
import '../../services/analytics_service.dart';
import '../../services/location_detection_service.dart';
import '../../widgets/solar_orbit.dart';

/// Result of the [AddHomeFlow] — the home name plus an optional resolved
/// location. Location is intentionally optional: a user can create the home
/// now and set its location later from Settings.
class AddHomeResult {
  final String name;
  final HomeLocation? location;
  final String? timezone;

  const AddHomeResult({
    required this.name,
    this.location,
    this.timezone,
  });
}

/// Two-step "create a home" flow, shown right after a device is paired:
///
///   Step 1 — Name      What is this home called?
///   Step 2 — Location  Where is it? (drives sunrise/sunset). Skippable.
///
/// Replaces the bare "name this home" text dialog. Returns an [AddHomeResult],
/// or null if the user backs all the way out.
class AddHomeFlow extends StatefulWidget {
  final String defaultName;

  const AddHomeFlow({super.key, required this.defaultName});

  static Future<AddHomeResult?> show(
    BuildContext context, {
    required String defaultName,
  }) {
    AnalyticsService().logScreenView('add_home_flow');
    return Navigator.of(context, rootNavigator: true).push<AddHomeResult>(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        transitionDuration: const Duration(milliseconds: 320),
        reverseTransitionDuration: const Duration(milliseconds: 240),
        pageBuilder: (_, __, ___) => AddHomeFlow(defaultName: defaultName),
        transitionsBuilder: (_, animation, __, child) {
          final curved = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return FadeTransition(
            opacity: curved,
            child: SlideTransition(
              position: Tween<Offset>(
                begin: const Offset(0, 0.04),
                end: Offset.zero,
              ).animate(curved),
              child: child,
            ),
          );
        },
      ),
    );
  }

  @override
  State<AddHomeFlow> createState() => _AddHomeFlowState();
}

enum _Step { name, location }

class _AddHomeFlowState extends State<AddHomeFlow> {
  final _nameController = TextEditingController();
  final _geocoding = GeocodingService();

  _Step _step = _Step.name;
  PlaceResult? _location;
  String? _timezone;
  bool _isLocating = false;
  String? _locationError;

  @override
  void initState() {
    super.initState();
    _nameController.text = widget.defaultName;
  }

  @override
  void dispose() {
    _nameController.dispose();
    super.dispose();
  }

  String get _name {
    final trimmed = _nameController.text.trim();
    return trimmed.isEmpty ? widget.defaultName : trimmed;
  }

  void _toLocation() {
    if (_nameController.text.trim().isEmpty) return;
    HapticFeedback.selectionClick();
    FocusScope.of(context).unfocus();
    setState(() => _step = _Step.location);
  }

  void _back() {
    HapticFeedback.selectionClick();
    if (_step == _Step.location) {
      setState(() => _step = _Step.name);
    } else {
      Navigator.of(context).pop();
    }
  }

  Future<void> _useCurrentLocation() async {
    setState(() {
      _isLocating = true;
      _locationError = null;
    });
    try {
      final serviceEnabled = await Geolocator.isLocationServiceEnabled();
      if (!serviceEnabled) {
        _failLocating('Location services are off. Turn them on and try again.');
        return;
      }

      final permission =
          await LocationDetectionService.requestPermissionIfNeeded(
        debugSource: 'AddHomeLocation',
      );
      if (permission == LocationPermission.deniedForever) {
        _failLocating(
          'Location permission is off for Rhythm. Enable it in Settings.',
        );
        return;
      }
      if (!LocationDetectionService.isGranted(permission)) {
        _failLocating('Location permission denied.');
        return;
      }

      final detected = await LocationDetectionService.getCurrentPosition(
        debugSource: 'AddHomeLocation',
      );
      final pos = detected.position;

      String? cityName;
      if (GeocodingService.isAvailable) {
        final place = await _geocoding
            .reverseGeocode(pos.latitude, pos.longitude)
            .timeout(const Duration(seconds: 5), onTimeout: () => null);
        cityName = place?.shortName;
      }
      final tz = (await FlutterTimezone.getLocalTimezone()).identifier;
      if (!mounted) return;

      HapticFeedback.mediumImpact();
      setState(() {
        _isLocating = false;
        _timezone = tz;
        _location = PlaceResult(
          city: cityName ?? 'Current location',
          latitude: pos.latitude,
          longitude: pos.longitude,
        );
      });
      AnalyticsService().logOnboardingLocationMethod('gps');
    } catch (e) {
      debugPrint('AddHomeLocation: failed: $e');
      _failLocating(_locationErrorMessage(e));
    }
  }

  void _failLocating(String message) {
    if (!mounted) return;
    setState(() {
      _isLocating = false;
      _locationError = message;
    });
  }

  String _locationErrorMessage(Object error) {
    if (error is LocationServiceDisabledException) {
      return 'Location services are off. Turn them on and try again.';
    }
    if (error is PermissionDeniedException) {
      return 'Location permission denied.';
    }
    if (error is TimeoutException) {
      return "Couldn't get a fix. Make sure location is on, then retry.";
    }
    return "Couldn't get your location. Try searching for a city instead.";
  }

  Future<void> _searchCity() async {
    FocusScope.of(context).unfocus();
    final place = await showModalBottomSheet<PlaceResult>(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (_) => _CitySearchSheet(geocoding: _geocoding),
    );
    if (place == null || !mounted) return;
    final tz = (await FlutterTimezone.getLocalTimezone()).identifier;
    if (!mounted) return;
    HapticFeedback.selectionClick();
    setState(() {
      _location = place;
      _timezone = tz;
      _locationError = null;
    });
    AnalyticsService().logOnboardingLocationMethod('city_search');
  }

  void _finish({required bool withLocation}) {
    HapticFeedback.mediumImpact();
    HomeLocation? location;
    if (withLocation && _location != null) {
      location = HomeLocation(
        latitude: _location!.latitude,
        longitude: _location!.longitude,
        cityName: _location!.city,
      );
    }
    Navigator.of(context).pop(
      AddHomeResult(
        name: _name,
        location: location,
        timezone: withLocation ? _timezone : null,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.transparent,
      resizeToAvoidBottomInset: true,
      body: Stack(
        fit: StackFit.expand,
        children: [
          const ColoredBox(color: CelestialColors.backgroundDark),
          const _AuroraGlow(),
          SafeArea(
            child: Column(
              children: [
                _TopBar(step: _step, onBack: _back),
                Expanded(
                  child: AnimatedSwitcher(
                    duration: const Duration(milliseconds: 300),
                    switchInCurve: Curves.easeOutCubic,
                    switchOutCurve: Curves.easeInCubic,
                    transitionBuilder: (child, anim) => FadeTransition(
                      opacity: anim,
                      child: SlideTransition(
                        position: Tween<Offset>(
                          begin: const Offset(0.06, 0),
                          end: Offset.zero,
                        ).animate(anim),
                        child: child,
                      ),
                    ),
                    child: _step == _Step.name
                        ? _NameStep(
                            key: const ValueKey('name'),
                            controller: _nameController,
                            onContinue: _toLocation,
                          )
                        : _LocationStep(
                            key: const ValueKey('location'),
                            homeName: _name,
                            location: _location,
                            isLocating: _isLocating,
                            error: _locationError,
                            onUseCurrent: _useCurrentLocation,
                            onSearch: _searchCity,
                            onClear: () => setState(() => _location = null),
                            onCreate: () => _finish(withLocation: true),
                            onSkip: () => _finish(withLocation: false),
                          ),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// Top bar — back chevron + two-segment step indicator.
// ═══════════════════════════════════════════════════════════════════════════

class _TopBar extends StatelessWidget {
  final _Step step;
  final VoidCallback onBack;
  const _TopBar({required this.step, required this.onBack});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(6, 6, 18, 0),
      child: Row(
        children: [
          IconButton(
            onPressed: onBack,
            splashRadius: 22,
            icon: Icon(
              Icons.arrow_back_rounded,
              color: CelestialColors.textSecondary.withValues(alpha: 0.85),
              size: 22,
            ),
          ),
          const Spacer(),
          _StepDot(active: true),
          const SizedBox(width: 7),
          _StepDot(active: step == _Step.location),
        ],
      ),
    );
  }
}

class _StepDot extends StatelessWidget {
  final bool active;
  const _StepDot({required this.active});

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 280),
      width: active ? 22 : 7,
      height: 7,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(4),
        color: active
            ? CelestialColors.sunWarm
            : CelestialColors.textSecondary.withValues(alpha: 0.3),
        boxShadow: active
            ? [
                BoxShadow(
                  color: CelestialColors.sunWarm.withValues(alpha: 0.45),
                  blurRadius: 8,
                  spreadRadius: 0.5,
                ),
              ]
            : null,
      ),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// Step 1 — Name
// ═══════════════════════════════════════════════════════════════════════════

class _NameStep extends StatelessWidget {
  final TextEditingController controller;
  final VoidCallback onContinue;

  const _NameStep({
    super.key,
    required this.controller,
    required this.onContinue,
  });

  @override
  Widget build(BuildContext context) {
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(28, 12, 28, 28),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const SizedBox(height: 12),
          const _StepGlyph(icon: Icons.home_rounded),
          const SizedBox(height: 28),
          const _Eyebrow(text: 'STEP 01  ·  YOUR HOME'),
          const SizedBox(height: 14),
          const _Title('Name your home'),
          const SizedBox(height: 10),
          const _Subtitle(
            'This is how it shows up in the app. You can rename it anytime.',
          ),
          const SizedBox(height: 32),
          _HomeNameField(controller: controller, onSubmitted: (_) => onContinue()),
          const SizedBox(height: 28),
          ListenableBuilder(
            listenable: controller,
            builder: (context, _) => _PrimaryButton(
              label: 'Continue',
              icon: Icons.arrow_forward_rounded,
              enabled: controller.text.trim().isNotEmpty,
              onTap: onContinue,
            ),
          ),
        ],
      ),
    );
  }
}

class _HomeNameField extends StatelessWidget {
  final TextEditingController controller;
  final ValueChanged<String> onSubmitted;

  const _HomeNameField({required this.controller, required this.onSubmitted});

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: Colors.white.withValues(alpha: 0.03),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: Colors.white.withValues(alpha: 0.12)),
      ),
      child: TextField(
        controller: controller,
        autofocus: true,
        textCapitalization: TextCapitalization.words,
        textInputAction: TextInputAction.next,
        onSubmitted: onSubmitted,
        style: const TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 17,
          fontWeight: FontWeight.w500,
        ),
        cursorColor: CelestialColors.sunWarm,
        decoration: InputDecoration(
          hintText: 'e.g. Beach House',
          hintStyle: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            fontWeight: FontWeight.w400,
          ),
          prefixIcon: Icon(
            Icons.label_outline_rounded,
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            size: 20,
          ),
          border: InputBorder.none,
          contentPadding: const EdgeInsets.symmetric(
            horizontal: 16,
            vertical: 18,
          ),
        ),
      ),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// Step 2 — Location
// ═══════════════════════════════════════════════════════════════════════════

class _LocationStep extends StatelessWidget {
  final String homeName;
  final PlaceResult? location;
  final bool isLocating;
  final String? error;
  final VoidCallback onUseCurrent;
  final VoidCallback onSearch;
  final VoidCallback onClear;
  final VoidCallback onCreate;
  final VoidCallback onSkip;

  const _LocationStep({
    super.key,
    required this.homeName,
    required this.location,
    required this.isLocating,
    required this.error,
    required this.onUseCurrent,
    required this.onSearch,
    required this.onClear,
    required this.onCreate,
    required this.onSkip,
  });

  @override
  Widget build(BuildContext context) {
    final hasLocation = location != null;
    return Column(
      children: [
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.fromLTRB(28, 12, 28, 12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                const SizedBox(height: 12),
                const _StepGlyph(icon: Icons.location_on_rounded),
                const SizedBox(height: 28),
                const _Eyebrow(text: 'STEP 02  ·  LOCATION'),
                const SizedBox(height: 14),
                const _Title('Where is it?'),
                const SizedBox(height: 10),
                _Subtitle(
                  'Rhythm follows the local sunrise and sunset at $homeName '
                  'to time your light.',
                ),
                const SizedBox(height: 30),
                if (hasLocation)
                  _SelectedLocationCard(location: location!, onClear: onClear)
                else ...[
                  _PrimaryButton(
                    label: 'Use current location',
                    icon: Icons.my_location_rounded,
                    loading: isLocating,
                    onTap: onUseCurrent,
                  ),
                  const SizedBox(height: 14),
                  _SearchCityTile(onTap: onSearch),
                ],
                if (error != null) ...[
                  const SizedBox(height: 18),
                  _ErrorBanner(message: error!),
                ],
              ],
            ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(28, 8, 28, 20),
          child: Column(
            children: [
              _PrimaryButton(
                label: 'Create home',
                icon: Icons.check_rounded,
                filled: hasLocation,
                onTap: onCreate,
              ),
              const SizedBox(height: 10),
              if (!hasLocation)
                _GhostLink(
                  label: 'Skip for now',
                  onTap: onSkip,
                ),
            ],
          ),
        ),
      ],
    );
  }
}

class _SelectedLocationCard extends StatelessWidget {
  final PlaceResult location;
  final VoidCallback onClear;
  const _SelectedLocationCard({required this.location, required this.onClear});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 18),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(18),
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            CelestialColors.sunWarm.withValues(alpha: 0.16),
            CelestialColors.sunWarm.withValues(alpha: 0.05),
          ],
        ),
        border: Border.all(
          color: CelestialColors.sunWarm.withValues(alpha: 0.35),
        ),
      ),
      child: Row(
        children: [
          Container(
            width: 42,
            height: 42,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: CelestialColors.sunWarm.withValues(alpha: 0.18),
            ),
            child: const Icon(
              Icons.place_rounded,
              color: CelestialColors.sunWarm,
              size: 22,
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  location.shortName,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  '${location.latitude.toStringAsFixed(3)}, '
                  '${location.longitude.toStringAsFixed(3)}',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 12.5,
                  ),
                ),
              ],
            ),
          ),
          TextButton(
            onPressed: onClear,
            style: TextButton.styleFrom(
              foregroundColor: CelestialColors.textSecondary,
              padding: const EdgeInsets.symmetric(horizontal: 8),
            ),
            child: const Text('Change'),
          ),
        ],
      ),
    );
  }
}

class _SearchCityTile extends StatelessWidget {
  final VoidCallback onTap;
  const _SearchCityTile({required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(16),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 16),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            color: Colors.white.withValues(alpha: 0.02),
            border: Border.all(color: Colors.white.withValues(alpha: 0.12)),
          ),
          child: Row(
            children: [
              Icon(
                Icons.search_rounded,
                size: 20,
                color: CelestialColors.accentBlue.withValues(alpha: 0.9),
              ),
              const SizedBox(width: 14),
              const Expanded(
                child: Text(
                  'Search for a city',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              Icon(
                Icons.arrow_forward_ios_rounded,
                size: 14,
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ErrorBanner extends StatelessWidget {
  final String message;
  const _ErrorBanner({required this.message});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(13),
      decoration: BoxDecoration(
        color: const Color(0xFFE5484D).withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: const Color(0xFFE5484D).withValues(alpha: 0.3)),
      ),
      child: Row(
        children: [
          const Icon(Icons.error_outline_rounded,
              color: Color(0xFFFF9B9B), size: 19),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              message,
              style: const TextStyle(color: Color(0xFFFFC2C2), fontSize: 13.5),
            ),
          ),
        ],
      ),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// City search bottom sheet
// ═══════════════════════════════════════════════════════════════════════════

class _CitySearchSheet extends StatefulWidget {
  final GeocodingService geocoding;
  const _CitySearchSheet({required this.geocoding});

  @override
  State<_CitySearchSheet> createState() => _CitySearchSheetState();
}

class _CitySearchSheetState extends State<_CitySearchSheet> {
  final _controller = TextEditingController();
  final _focusNode = FocusNode();
  Timer? _debounce;
  bool _searching = false;
  List<PlaceResult> _results = [];

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _focusNode.requestFocus());
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _controller.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void _onChanged(String value) {
    _debounce?.cancel();
    if (value.trim().length < 2) {
      setState(() {
        _results = [];
        _searching = false;
      });
      return;
    }
    setState(() => _searching = true);
    _debounce = Timer(const Duration(milliseconds: 300), () async {
      final results = await widget.geocoding.searchPlaces(value);
      if (mounted) {
        setState(() {
          _results = results;
          _searching = false;
        });
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final bottomInset = MediaQuery.of(context).viewInsets.bottom;
    return Padding(
      padding: EdgeInsets.only(bottom: bottomInset),
      child: Container(
        margin: const EdgeInsets.all(14),
        padding: const EdgeInsets.all(20),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(24),
          border: Border.all(color: Colors.white.withValues(alpha: 0.08)),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 40,
              height: 4,
              decoration: BoxDecoration(
                color: CelestialColors.orbitRing.withValues(alpha: 0.6),
                borderRadius: BorderRadius.circular(2),
              ),
            ),
            const SizedBox(height: 20),
            const Align(
              alignment: Alignment.centerLeft,
              child: Text(
                'Search for a city',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 19,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            const SizedBox(height: 16),
            Container(
              decoration: BoxDecoration(
                color: CelestialColors.backgroundDark,
                borderRadius: BorderRadius.circular(14),
                border: Border.all(color: Colors.white.withValues(alpha: 0.1)),
              ),
              child: TextField(
                controller: _controller,
                focusNode: _focusNode,
                onChanged: _onChanged,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 16,
                ),
                cursorColor: CelestialColors.sunWarm,
                decoration: InputDecoration(
                  hintText: 'Enter a city name…',
                  hintStyle: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.55),
                  ),
                  prefixIcon: Icon(
                    Icons.search_rounded,
                    color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                  ),
                  suffixIcon: _searching
                      ? const Padding(
                          padding: EdgeInsets.all(14),
                          child: SizedBox(
                            width: 16,
                            height: 16,
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              valueColor: AlwaysStoppedAnimation<Color>(
                                CelestialColors.sunWarm,
                              ),
                            ),
                          ),
                        )
                      : null,
                  border: InputBorder.none,
                  contentPadding:
                      const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
                ),
              ),
            ),
            const SizedBox(height: 12),
            if (_results.isNotEmpty)
              ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 280),
                child: ListView.separated(
                  shrinkWrap: true,
                  itemCount: _results.length,
                  separatorBuilder: (_, __) => Divider(
                    height: 1,
                    color: Colors.white.withValues(alpha: 0.05),
                  ),
                  itemBuilder: (context, index) {
                    final place = _results[index];
                    return ListTile(
                      contentPadding: EdgeInsets.zero,
                      leading: Container(
                        width: 40,
                        height: 40,
                        decoration: BoxDecoration(
                          color: CelestialColors.sunWarm.withValues(alpha: 0.14),
                          borderRadius: BorderRadius.circular(10),
                        ),
                        child: const Icon(
                          Icons.location_city_rounded,
                          color: CelestialColors.sunWarm,
                          size: 20,
                        ),
                      ),
                      title: Text(
                        place.shortName,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                      trailing: Icon(
                        Icons.arrow_forward_ios_rounded,
                        color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                        size: 15,
                      ),
                      onTap: () => Navigator.of(context).pop(place),
                    );
                  },
                ),
              )
            else if (_controller.text.trim().length >= 2 && !_searching)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 24),
                child: Text(
                  'No matches found',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                  ),
                ),
              ),
            const SizedBox(height: 4),
          ],
        ),
      ),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared bits
// ═══════════════════════════════════════════════════════════════════════════

class _StepGlyph extends StatelessWidget {
  final IconData icon;
  const _StepGlyph({required this.icon});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Container(
        width: 88,
        height: 88,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          gradient: RadialGradient(
            colors: [
              CelestialColors.sunWarm.withValues(alpha: 0.28),
              CelestialColors.sunWarm.withValues(alpha: 0.06),
              Colors.transparent,
            ],
            stops: const [0.0, 0.6, 1.0],
          ),
        ),
        child: Center(
          child: Container(
            width: 60,
            height: 60,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: CelestialColors.backgroundCard,
              border: Border.all(
                color: CelestialColors.sunWarm.withValues(alpha: 0.45),
                width: 1.5,
              ),
            ),
            child: Icon(icon, color: CelestialColors.sunWarm, size: 28),
          ),
        ),
      ),
    );
  }
}

class _Eyebrow extends StatelessWidget {
  final String text;
  const _Eyebrow({required this.text});

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Container(
          width: 5,
          height: 5,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: CelestialColors.sunWarm,
            boxShadow: [
              BoxShadow(
                color: CelestialColors.sunWarm.withValues(alpha: 0.6),
                blurRadius: 8,
                spreadRadius: 1,
              ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        Text(
          text,
          style: TextStyle(
            color: CelestialColors.textPrimary.withValues(alpha: 0.7),
            fontSize: 10.5,
            fontWeight: FontWeight.w700,
            letterSpacing: 3.6,
          ),
        ),
      ],
    );
  }
}

class _Title extends StatelessWidget {
  final String text;
  const _Title(this.text);

  @override
  Widget build(BuildContext context) {
    return Text(
      text,
      textAlign: TextAlign.center,
      style: const TextStyle(
        color: CelestialColors.textPrimary,
        fontSize: 30,
        fontWeight: FontWeight.w300,
        letterSpacing: -0.5,
        height: 1.1,
      ),
    );
  }
}

class _Subtitle extends StatelessWidget {
  final String text;
  const _Subtitle(this.text);

  @override
  Widget build(BuildContext context) {
    return Text(
      text,
      textAlign: TextAlign.center,
      style: TextStyle(
        color: CelestialColors.textSecondary.withValues(alpha: 0.85),
        fontSize: 14.5,
        height: 1.5,
        letterSpacing: 0.1,
      ),
    );
  }
}

/// Primary action. `filled` controls the warm gradient (default true);
/// when false it renders as a quiet outlined button for secondary emphasis.
class _PrimaryButton extends StatelessWidget {
  final String label;
  final IconData icon;
  final VoidCallback onTap;
  final bool enabled;
  final bool loading;
  final bool filled;

  const _PrimaryButton({
    required this.label,
    required this.icon,
    required this.onTap,
    this.enabled = true,
    this.loading = false,
    this.filled = true,
  });

  @override
  Widget build(BuildContext context) {
    final isOn = enabled && !loading;
    final dim = !enabled;
    return Opacity(
      opacity: dim ? 0.5 : 1.0,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: isOn ? onTap : null,
          borderRadius: BorderRadius.circular(16),
          child: Container(
            height: 56,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(16),
              gradient: filled
                  ? const LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [CelestialColors.sunWarm, Color(0xFFE8821C)],
                    )
                  : null,
              color: filled ? null : Colors.white.withValues(alpha: 0.04),
              border: filled
                  ? null
                  : Border.all(color: Colors.white.withValues(alpha: 0.16)),
              boxShadow: filled
                  ? [
                      BoxShadow(
                        color: CelestialColors.sunWarm.withValues(alpha: 0.32),
                        blurRadius: 26,
                        spreadRadius: -4,
                        offset: const Offset(0, 10),
                      ),
                    ]
                  : null,
            ),
            child: loading
                ? const SizedBox(
                    width: 22,
                    height: 22,
                    child: CircularProgressIndicator(
                      strokeWidth: 2.2,
                      valueColor:
                          AlwaysStoppedAnimation<Color>(Color(0xFF1A1206)),
                    ),
                  )
                : Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        label,
                        style: TextStyle(
                          color: filled
                              ? const Color(0xFF1A1206)
                              : CelestialColors.textPrimary,
                          fontSize: 16,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.3,
                        ),
                      ),
                      const SizedBox(width: 8),
                      Icon(
                        icon,
                        size: 19,
                        color: filled
                            ? const Color(0xFF1A1206)
                            : CelestialColors.textPrimary,
                      ),
                    ],
                  ),
          ),
        ),
      ),
    );
  }
}

class _GhostLink extends StatelessWidget {
  final String label;
  final VoidCallback onTap;
  const _GhostLink({required this.label, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return TextButton(
      onPressed: onTap,
      style: TextButton.styleFrom(
        foregroundColor: CelestialColors.textSecondary,
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 12),
      ),
      child: Text(
        label,
        style: TextStyle(
          color: CelestialColors.textSecondary.withValues(alpha: 0.8),
          fontSize: 14,
          fontWeight: FontWeight.w500,
          letterSpacing: 0.3,
        ),
      ),
    );
  }
}

class _AuroraGlow extends StatelessWidget {
  const _AuroraGlow();

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: DecoratedBox(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: const Alignment(0, -0.55),
            radius: 1.1,
            colors: [
              CelestialColors.sunWarm.withValues(alpha: 0.10),
              CelestialColors.sunWarm.withValues(alpha: 0.02),
              Colors.transparent,
            ],
            stops: const [0.0, 0.4, 1.0],
          ),
        ),
      ),
    );
  }
}
