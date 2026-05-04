import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_timezone/flutter_timezone.dart';
import 'package:provider/provider.dart';
import 'package:geolocator/geolocator.dart';
import '../widgets/onboarding_orbit.dart';
import '../widgets/sun_glow_button.dart';
import '../providers/auth_provider.dart';
import '../providers/onboarding_provider.dart';
import '../services/geocoding_service.dart';
import '../../services/analytics_service.dart';
import '../../services/location_detection_service.dart';

/// Location screen for setting home location for sunrise/sunset calculations.
class LocationScreen extends StatefulWidget {
  /// Called when this is the final screen (login disabled) to complete onboarding.
  final VoidCallback? onComplete;

  const LocationScreen({super.key, this.onComplete});

  @override
  State<LocationScreen> createState() => _LocationScreenState();
}

class _LocationScreenState extends State<LocationScreen>
    with TickerProviderStateMixin {
  final _searchController = TextEditingController();
  final _searchFocusNode = FocusNode();
  final _geocodingService = GeocodingService();

  late AnimationController _pulseController;
  late AnimationController _successController;
  late Animation<double> _pulseAnimation;
  late Animation<double> _successScaleAnimation;
  late Animation<double> _successOpacityAnimation;

  bool _isLoadingGps = false;
  bool _isSearching = false;
  String? _errorMessage;
  List<PlaceResult> _searchResults = [];
  PlaceResult? _confirmedLocation;
  Timer? _debounceTimer;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 2500),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 0.2, end: 0.5).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );

    _successController = AnimationController(
      duration: const Duration(milliseconds: 600),
      vsync: this,
    );

    _successScaleAnimation = Tween<double>(begin: 0.8, end: 1.0).animate(
      CurvedAnimation(parent: _successController, curve: Curves.elasticOut),
    );

    _successOpacityAnimation = Tween<double>(begin: 0.0, end: 1.0).animate(
      CurvedAnimation(parent: _successController, curve: Curves.easeOut),
    );

    _searchController.addListener(_onSearchChanged);
  }

  @override
  void dispose() {
    _pulseController.dispose();
    _successController.dispose();
    _searchController.dispose();
    _searchFocusNode.dispose();
    _debounceTimer?.cancel();
    super.dispose();
  }

  void _onSearchChanged() {
    _debounceTimer?.cancel();

    if (_searchController.text.trim().length < 2) {
      setState(() {
        _searchResults = [];
        _isSearching = false;
      });
      return;
    }

    setState(() => _isSearching = true);

    _debounceTimer = Timer(const Duration(milliseconds: 300), () async {
      final results =
          await _geocodingService.searchPlaces(_searchController.text);
      if (mounted) {
        setState(() {
          _searchResults = results;
          _isSearching = false;
        });
      }
    });
  }

  Future<void> _selectPlace(PlaceResult place) async {
    final onboardingProvider = context.read<OnboardingProvider>();
    _searchFocusNode.unfocus();
    _searchController.clear();
    setState(() {
      _searchResults = [];
      _confirmedLocation = place;
      _errorMessage = null;
    });
    _successController.forward(from: 0);

    final timezone = (await FlutterTimezone.getLocalTimezone()).identifier;
    if (!mounted) return;
    onboardingProvider.setLocation(
      place.latitude,
      place.longitude,
      timezone,
      place.shortName,
    );

    // Track location method
    AnalyticsService().logOnboardingLocationMethod('city_search');
  }

  Future<void> _useCurrentLocation() async {
    final onboardingProvider = context.read<OnboardingProvider>();
    setState(() {
      _isLoadingGps = true;
      _errorMessage = null;
    });

    try {
      bool serviceEnabled = await Geolocator.isLocationServiceEnabled();
      if (!serviceEnabled) {
        setState(() {
          _errorMessage =
              'Location services are disabled. Please enable them in settings.';
          _isLoadingGps = false;
        });
        return;
      }

      final permission =
          await LocationDetectionService.requestPermissionIfNeeded(
        debugSource: 'OnboardingLocation',
      );

      if (permission == LocationPermission.deniedForever) {
        setState(() {
          _errorMessage =
              'Location permission permanently denied. Please enable in Settings.';
          _isLoadingGps = false;
        });
        return;
      }

      if (!LocationDetectionService.isGranted(permission)) {
        setState(() {
          _errorMessage = 'Location permission denied.';
          _isLoadingGps = false;
        });
        return;
      }

      final detected = await LocationDetectionService.getCurrentPosition(
        debugSource: 'OnboardingLocation',
      );
      final position = detected.position;

      // Try to reverse geocode for city name
      String? locationName;
      if (GeocodingService.isAvailable) {
        final place = await _geocodingService
            .reverseGeocode(position.latitude, position.longitude)
            .timeout(const Duration(seconds: 5), onTimeout: () => null);
        locationName = place?.shortName;
      }

      final timezone = (await FlutterTimezone.getLocalTimezone()).identifier;
      if (!mounted) return;

      setState(() {
        _isLoadingGps = false;
        _confirmedLocation = PlaceResult(
          city: locationName ?? 'Current Location',
          latitude: position.latitude,
          longitude: position.longitude,
        );
      });
      _successController.forward(from: 0);

      onboardingProvider.setLocation(
        position.latitude,
        position.longitude,
        timezone,
        locationName ?? 'Current Location',
      );

      // Track location method
      AnalyticsService().logOnboardingLocationMethod('gps');
    } catch (e) {
      debugPrint('OnboardingLocation: failed to detect location: $e');
      if (!mounted) return;
      setState(() {
        _errorMessage = _locationErrorMessage(e);
        _isLoadingGps = false;
      });
    }
  }

  String _locationErrorMessage(Object error) {
    if (error is LocationServiceDisabledException) {
      return 'Location services are disabled. Please enable them in settings.';
    }
    if (error is PermissionDeniedException) {
      return 'Location permission denied.';
    }
    if (error is TimeoutException) {
      return 'Could not get a location fix. Check Android Location is on, then try again.';
    }
    return 'Failed to get location. Please try again.';
  }

  Future<void> _continue() async {
    if (widget.onComplete != null) {
      // This is the final screen (login disabled) - save preferences and complete
      final onboardingProvider = context.read<OnboardingProvider>();
      await context
          .read<AuthProvider>()
          .savePreferences(onboardingProvider.preferences);
      widget.onComplete!();
    } else {
      // Continue to account screen
      context.read<OnboardingProvider>().nextPage();
    }
  }

  void _changeLocation() {
    setState(() {
      _confirmedLocation = null;
      _errorMessage = null;
    });
    _successController.reset();
  }

  void _showCitySearch() {
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (context) => _CitySearchSheet(
        searchController: _searchController,
        searchFocusNode: _searchFocusNode,
        isSearching: _isSearching,
        searchResults: _searchResults,
        onSearchChanged: (value) {
          _onSearchChanged();
          // Force rebuild of bottom sheet
          (context as Element).markNeedsBuild();
        },
        onClearSearch: () {
          _searchController.clear();
          setState(() => _searchResults = []);
        },
        onSelectPlace: (place) {
          Navigator.pop(context);
          _selectPlace(place);
        },
      ),
    );
    // Focus the search field after sheet opens
    Future.delayed(const Duration(milliseconds: 300), () {
      _searchFocusNode.requestFocus();
    });
  }

  @override
  Widget build(BuildContext context) {
    final showCitySearch = GeocodingService.isAvailable;

    return GestureDetector(
      onTap: () => FocusScope.of(context).unfocus(),
      child: Container(
        color: OnboardingColors.backgroundDark,
        child: SafeArea(
          child: _confirmedLocation != null
              ? _buildSuccessLayout()
              : _buildInputLayout(showCitySearch),
        ),
      ),
    );
  }

  Widget _buildInputLayout(bool showCitySearch) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      child: Column(
        children: [
          const Spacer(flex: 2),
          // Pulsing home icon
          AnimatedBuilder(
            animation: _pulseAnimation,
            builder: (context, child) {
              return Container(
                width: 160,
                height: 160,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  gradient: RadialGradient(
                    colors: [
                      OnboardingColors.sunWarm
                          .withValues(alpha: _pulseAnimation.value),
                      OnboardingColors.sunWarm
                          .withValues(alpha: _pulseAnimation.value * 0.3),
                      Colors.transparent,
                    ],
                  ),
                ),
                child: Center(
                  child: Container(
                    width: 90,
                    height: 90,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: OnboardingColors.backgroundCard,
                      border: Border.all(
                        color: OnboardingColors.sunWarm.withValues(alpha: 0.5),
                        width: 2,
                      ),
                    ),
                    child: const Icon(
                      Icons.home_rounded,
                      color: OnboardingColors.sunWarm,
                      size: 42,
                    ),
                  ),
                ),
              );
            },
          ),
          const SizedBox(height: 32),
          // Title
          const Text(
            'Home Location',
            style: TextStyle(
              color: OnboardingColors.textPrimary,
              fontSize: 28,
              fontWeight: FontWeight.bold,
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(height: 12),
          // Subtitle
          const Text(
            'Set your home location to calculate\nsunrise and sunset times',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: OnboardingColors.textSecondary,
              fontSize: 16,
              height: 1.5,
            ),
          ),
          // Error message
          if (_errorMessage != null) ...[
            const SizedBox(height: 24),
            _buildErrorMessage(),
          ],
          const Spacer(flex: 3),
          // Primary button
          SunGlowButton(
            text: 'Use Current Location',
            isLoading: _isLoadingGps,
            onPressed: _useCurrentLocation,
          ),
          // City search option (iOS/Android only)
          if (showCitySearch) ...[
            const SizedBox(height: 20),
            TextLinkButton(
              text: 'Search by city name',
              onPressed: _showCitySearch,
            ),
          ],
          const Spacer(),
        ],
      ),
    );
  }

  Widget _buildSuccessLayout() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      child: AnimatedBuilder(
        animation: _successController,
        builder: (context, child) {
          return Opacity(
            opacity: _successOpacityAnimation.value,
            child: Transform.scale(
              scale: _successScaleAnimation.value,
              child: child,
            ),
          );
        },
        child: Column(
          children: [
            const Spacer(flex: 2),
            // Success checkmark with glow
            Container(
              width: 160,
              height: 160,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: RadialGradient(
                  colors: [
                    OnboardingColors.sunWarm.withValues(alpha: 0.5),
                    OnboardingColors.sunWarm.withValues(alpha: 0.15),
                    Colors.transparent,
                  ],
                ),
              ),
              child: Center(
                child: Container(
                  width: 90,
                  height: 90,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: OnboardingColors.sunWarm,
                    boxShadow: [
                      BoxShadow(
                        color: OnboardingColors.sunWarm.withValues(alpha: 0.5),
                        blurRadius: 30,
                        spreadRadius: 5,
                      ),
                    ],
                  ),
                  child: const Icon(
                    Icons.check_rounded,
                    color: OnboardingColors.backgroundDark,
                    size: 48,
                  ),
                ),
              ),
            ),
            const SizedBox(height: 32),
            // Title
            const Text(
              'Home Location Set',
              style: TextStyle(
                color: OnboardingColors.textPrimary,
                fontSize: 28,
                fontWeight: FontWeight.bold,
                letterSpacing: 0.5,
              ),
            ),
            const SizedBox(height: 16),
            // Location name
            Text(
              _confirmedLocation!.shortName,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: OnboardingColors.sunWarm,
                fontSize: 22,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: 8),
            // Change link
            TextButton.icon(
              onPressed: _changeLocation,
              icon: Icon(
                Icons.edit_rounded,
                size: 14,
                color: OnboardingColors.textSecondary.withValues(alpha: 0.7),
              ),
              label: Text(
                'Change',
                style: TextStyle(
                  color: OnboardingColors.textSecondary.withValues(alpha: 0.7),
                  fontSize: 14,
                ),
              ),
            ),
            const Spacer(flex: 3),
            // Continue button
            SunGlowButton(
              text: 'Continue',
              onPressed: _continue,
            ),
            const Spacer(),
          ],
        ),
      ),
    );
  }

  Widget _buildErrorMessage() {
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Colors.red.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(
          color: Colors.red.withValues(alpha: 0.3),
        ),
      ),
      child: Row(
        children: [
          Icon(
            Icons.error_outline_rounded,
            color: Colors.red.shade300,
            size: 20,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              _errorMessage!,
              style: TextStyle(
                color: Colors.red.shade300,
                fontSize: 14,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Bottom sheet for city search.
class _CitySearchSheet extends StatefulWidget {
  final TextEditingController searchController;
  final FocusNode searchFocusNode;
  final bool isSearching;
  final List<PlaceResult> searchResults;
  final ValueChanged<String> onSearchChanged;
  final VoidCallback onClearSearch;
  final ValueChanged<PlaceResult> onSelectPlace;

  const _CitySearchSheet({
    required this.searchController,
    required this.searchFocusNode,
    required this.isSearching,
    required this.searchResults,
    required this.onSearchChanged,
    required this.onClearSearch,
    required this.onSelectPlace,
  });

  @override
  State<_CitySearchSheet> createState() => _CitySearchSheetState();
}

class _CitySearchSheetState extends State<_CitySearchSheet> {
  @override
  void initState() {
    super.initState();
    widget.searchController.addListener(_onChanged);
  }

  void _onChanged() {
    setState(() {});
    widget.onSearchChanged(widget.searchController.text);
  }

  @override
  Widget build(BuildContext context) {
    final bottomInset = MediaQuery.of(context).viewInsets.bottom;

    return Padding(
      padding: EdgeInsets.only(bottom: bottomInset),
      child: Container(
        margin: const EdgeInsets.all(16),
        padding: const EdgeInsets.all(20),
        decoration: BoxDecoration(
          color: OnboardingColors.backgroundCard,
          borderRadius: BorderRadius.circular(24),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            // Handle
            Container(
              width: 40,
              height: 4,
              decoration: BoxDecoration(
                color: OnboardingColors.orbitRing.withValues(alpha: 0.5),
                borderRadius: BorderRadius.circular(2),
              ),
            ),
            const SizedBox(height: 20),
            // Title
            const Text(
              'Search City',
              style: TextStyle(
                color: OnboardingColors.textPrimary,
                fontSize: 20,
                fontWeight: FontWeight.bold,
              ),
            ),
            const SizedBox(height: 20),
            // Search field
            Container(
              decoration: BoxDecoration(
                color: OnboardingColors.backgroundDark,
                borderRadius: BorderRadius.circular(16),
                border: Border.all(
                  color: OnboardingColors.orbitRing.withValues(alpha: 0.5),
                  width: 1.5,
                ),
              ),
              child: TextField(
                controller: widget.searchController,
                focusNode: widget.searchFocusNode,
                style: const TextStyle(
                  color: OnboardingColors.textPrimary,
                  fontSize: 16,
                ),
                decoration: InputDecoration(
                  hintText: 'Enter city name...',
                  hintStyle: TextStyle(
                    color:
                        OnboardingColors.textSecondary.withValues(alpha: 0.6),
                    fontSize: 16,
                  ),
                  prefixIcon: Icon(
                    Icons.search_rounded,
                    color:
                        OnboardingColors.textSecondary.withValues(alpha: 0.6),
                  ),
                  suffixIcon: widget.searchController.text.isNotEmpty
                      ? IconButton(
                          icon: Icon(
                            Icons.clear_rounded,
                            color: OnboardingColors.textSecondary
                                .withValues(alpha: 0.6),
                          ),
                          onPressed: widget.onClearSearch,
                        )
                      : null,
                  border: InputBorder.none,
                  contentPadding: const EdgeInsets.symmetric(
                    horizontal: 16,
                    vertical: 16,
                  ),
                ),
              ),
            ),
            const SizedBox(height: 16),
            // Results
            if (widget.searchResults.isNotEmpty)
              ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 250),
                child: ListView.builder(
                  shrinkWrap: true,
                  itemCount: widget.searchResults.length,
                  itemBuilder: (context, index) {
                    final place = widget.searchResults[index];
                    return ListTile(
                      leading: Container(
                        width: 40,
                        height: 40,
                        decoration: BoxDecoration(
                          color:
                              OnboardingColors.sunWarm.withValues(alpha: 0.15),
                          borderRadius: BorderRadius.circular(10),
                        ),
                        child: const Icon(
                          Icons.location_city_rounded,
                          color: OnboardingColors.sunWarm,
                          size: 20,
                        ),
                      ),
                      title: Text(
                        place.shortName,
                        style: const TextStyle(
                          color: OnboardingColors.textPrimary,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                      trailing: Icon(
                        Icons.arrow_forward_ios_rounded,
                        color: OnboardingColors.textSecondary
                            .withValues(alpha: 0.4),
                        size: 16,
                      ),
                      onTap: () => widget.onSelectPlace(place),
                    );
                  },
                ),
              )
            else if (widget.searchController.text.length >= 2)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 24),
                child: Text(
                  'No cities found',
                  style: TextStyle(
                    color:
                        OnboardingColors.textSecondary.withValues(alpha: 0.6),
                    fontSize: 14,
                  ),
                ),
              ),
            const SizedBox(height: 8),
          ],
        ),
      ),
    );
  }
}
