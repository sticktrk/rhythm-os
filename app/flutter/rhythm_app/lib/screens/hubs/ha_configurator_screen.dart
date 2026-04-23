import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../../widgets/beta_badge.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/hub_status_indicator.dart';
import '../../providers/home_provider.dart';
import '../../providers/room_provider.dart';
import '../../providers/server_sync_provider.dart';

/// Connection status for the Home Assistant configurator.
enum HAConnectionStatus {
  notConfigured,
  connecting,
  connected,
  error,
}

/// Full-screen modal for configuring Home Assistant connection.
///
/// Features:
/// - Manual entry of host, port, and token
/// - Network autodiscovery via mDNS
/// - Connection verification before saving
/// - Elegant celestial-themed UI
class HAConfiguratorScreen extends StatefulWidget {
  const HAConfiguratorScreen({super.key});

  /// Show the configurator as a full-screen modal.
  static Future<bool?> show(BuildContext context) {
    return Navigator.of(context).push<bool>(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const HAConfiguratorScreen();
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
  State<HAConfiguratorScreen> createState() => _HAConfiguratorScreenState();
}

class _HAConfiguratorScreenState extends State<HAConfiguratorScreen>
    with SingleTickerProviderStateMixin {
  final _formKey = GlobalKey<FormState>();
  final _hostController = TextEditingController();
  final _portController = TextEditingController(text: '8123');
  final _tokenController = TextEditingController();

  bool _useSsl = false;
  bool _obscureToken = true;
  HAConnectionStatus _status = HAConnectionStatus.notConfigured;
  String? _errorMessage;
  bool _isVerified = false;
  bool _wasAlreadyConfigured = false;

  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  @override
  void initState() {
    super.initState();
    _loadSavedConfig();

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _glowAnimation = Tween<double>(begin: 0.3, end: 0.6).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _hostController.dispose();
    _portController.dispose();
    _tokenController.dispose();
    _glowController.dispose();
    super.dispose();
  }

  Future<void> _loadSavedConfig() async {
    final homeProvider = context.read<HomeProvider>();
    final haHub = homeProvider.getFirstHubOfType(HubType.homeAssistant);

    if (haHub != null) {
      setState(() {
        _hostController.text = haHub.endpoint.host;
        _portController.text = haHub.endpoint.port.toString();
        if (haHub.token != null) _tokenController.text = haHub.token!;
        _useSsl = haHub.endpoint.useSsl;
        if (haHub.hasCredentials) {
          _isVerified = true;
          _wasAlreadyConfigured = true;
          _status = HAConnectionStatus.connected;
        }
      });
    }
  }

  Future<void> _disconnectHomeAssistant() async {
    final syncProvider = context.read<ServerSyncProvider>();
    final roomProvider = context.read<RoomProvider>();
    final homeProvider = context.read<HomeProvider>();
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(20),
        ),
        title: const Text(
          'Disconnect Home Assistant?',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 18,
            fontWeight: FontWeight.w600,
          ),
        ),
        content: const Text(
          'This will remove your Home Assistant configuration. You can reconnect at any time.',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 14,
            height: 1.5,
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(
                color: CelestialColors.textSecondary,
              ),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(
              'Disconnect',
              style: TextStyle(
                color: Colors.red.shade400,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true) return;

    // Tell server to drop hub + rooms, then clear locally
    await syncProvider.disconnectHub();
    await roomProvider.clearRoomsBySource(RoomSourceDto.homeAssistant);

    // Delete HA hub from HomeProvider
    final haHub = homeProvider.getFirstHubOfType(HubType.homeAssistant);
    if (haHub != null) {
      await homeProvider.deleteHub(haHub.id);
    }

    HapticFeedback.mediumImpact();

    if (!mounted) return;
    Navigator.of(context).pop(true);
  }

  Future<void> _verifyConnection() async {
    if (!_formKey.currentState!.validate()) return;

    setState(() {
      _status = HAConnectionStatus.connecting;
      _errorMessage = null;
      _isVerified = false;
    });

    try {
      final config = HomeAssistantConfig(
        host: _hostController.text.trim(),
        port: int.tryParse(_portController.text) ?? 8123,
        token: _tokenController.text.trim(),
        useSsl: _useSsl,
      );

      final provider = HomeAssistantProvider(config);
      final success = await provider.testConnection();
      await provider.dispose();

      setState(() {
        if (success) {
          _status = HAConnectionStatus.connected;
          _isVerified = true;
          HapticFeedback.mediumImpact();
        } else {
          _status = HAConnectionStatus.error;
          _errorMessage = 'Could not connect. Check your credentials.';
        }
      });
    } catch (e) {
      setState(() {
        _status = HAConnectionStatus.error;
        _errorMessage = 'Connection failed: ${e.toString()}';
      });
    }
  }

  Future<void> _saveConfig() async {
    if (!_isVerified) return;

    final homeProvider = context.read<HomeProvider>();
    final existingHub = homeProvider.getFirstHubOfType(HubType.homeAssistant);

    if (existingHub != null) {
      // Update existing hub
      final updatedHub = existingHub.copyWith(
        endpoint: HubEndpoint(
          host: _hostController.text.trim(),
          port: int.tryParse(_portController.text) ?? 8123,
          useSsl: _useSsl,
        ),
        token: _tokenController.text.trim(),
        updatedAt: DateTime.now(),
        pendingSync: true,
      );
      await homeProvider.updateHub(updatedHub);
    } else {
      // Create new hub
      await homeProvider.addHomeAssistantHub(
        name: 'Home Assistant',
        host: _hostController.text.trim(),
        port: int.tryParse(_portController.text) ?? 8123,
        useSsl: _useSsl,
        token: _tokenController.text.trim(),
      );
    }

    HapticFeedback.heavyImpact();

    if (mounted) {
      Navigator.of(context).pop(true);
    }
  }

  void _showDiscoverySheet() {
    showModalBottomSheet(
      context: context,
      backgroundColor: Colors.transparent,
      isScrollControlled: true,
      builder: (context) => _DiscoveryBottomSheet(
        onHubSelected: (hub) {
          setState(() {
            _hostController.text = hub.address;
            _portController.text = hub.port.toString();
            _isVerified = false;
            _status = HAConnectionStatus.notConfigured;
          });
          Navigator.of(context).pop();
        },
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: Form(
                  key: _formKey,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      const SizedBox(height: 8),
                      _buildHeroSection(),
                      const SizedBox(height: 32),
                      _buildConnectionFields(),
                      const SizedBox(height: 24),
                      _buildStatusIndicator(),
                      const SizedBox(height: 24),
                      _buildActionButtons(),
                      const SizedBox(height: 40),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.15),
                border: Border.all(
                  color: CelestialColors.accentBlue.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: const Icon(
                Icons.close,
                color: CelestialColors.accentBlue,
                size: 20,
              ),
            ),
          ),
          Expanded(
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                const Text(
                  'Home Assistant',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 18,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.3,
                  ),
                ),
                const SizedBox(width: 8),
                const BetaBadge(),
              ],
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildHeroSection() {
    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        return Container(
          padding: const EdgeInsets.all(24),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(24),
            gradient: RadialGradient(
              center: Alignment.center,
              radius: 1.2,
              colors: [
                const Color(0xFF03A9F4)
                    .withValues(alpha: _glowAnimation.value * 0.15),
                CelestialColors.backgroundCard,
              ],
            ),
            border: Border.all(
              color: const Color(0xFF03A9F4).withValues(alpha: 0.2),
              width: 1,
            ),
          ),
          child: Column(
            children: [
              Container(
                width: 72,
                height: 72,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  gradient: LinearGradient(
                    begin: Alignment.topLeft,
                    end: Alignment.bottomRight,
                    colors: [
                      const Color(0xFF03A9F4),
                      const Color(0xFF0288D1),
                    ],
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: const Color(0xFF03A9F4)
                          .withValues(alpha: _glowAnimation.value),
                      blurRadius: 24,
                      spreadRadius: 2,
                    ),
                  ],
                ),
                child: const Icon(
                  Icons.home_rounded,
                  color: Colors.white,
                  size: 36,
                ),
              ),
              const SizedBox(height: 16),
              const Text(
                'Connect to your smart home',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 16,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                'Enter your Home Assistant details or scan your network',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                  fontSize: 13,
                  height: 1.4,
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _buildConnectionFields() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Host field with scan button
        _buildTextField(
          controller: _hostController,
          label: 'Host',
          hint: 'homeassistant.local',
          keyboardType: TextInputType.url,
          prefixIcon: Icons.dns_outlined,
          suffixWidget: GestureDetector(
            onTap: _showDiscoverySheet,
            child: Container(
              margin: const EdgeInsets.only(right: 8),
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
              decoration: BoxDecoration(
                color: CelestialColors.accentBlue.withValues(alpha: 0.15),
                borderRadius: BorderRadius.circular(8),
                border: Border.all(
                  color: CelestialColors.accentBlue.withValues(alpha: 0.3),
                ),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    Icons.wifi_find_rounded,
                    size: 16,
                    color: CelestialColors.accentBlue,
                  ),
                  const SizedBox(width: 4),
                  Text(
                    'Scan',
                    style: TextStyle(
                      color: CelestialColors.accentBlue,
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ],
              ),
            ),
          ),
          validator: (value) {
            if (value == null || value.trim().isEmpty) {
              return 'Host is required';
            }
            return null;
          },
          onChanged: (_) => _resetVerification(),
        ),
        const SizedBox(height: 16),

        // Port and SSL row
        Row(
          children: [
            Expanded(
              flex: 2,
              child: _buildTextField(
                controller: _portController,
                label: 'Port',
                hint: '8123',
                keyboardType: TextInputType.number,
                prefixIcon: Icons.numbers_rounded,
                validator: (value) {
                  if (value == null || value.trim().isEmpty) {
                    return 'Required';
                  }
                  final port = int.tryParse(value);
                  if (port == null || port < 1 || port > 65535) {
                    return 'Invalid';
                  }
                  return null;
                },
                onChanged: (_) => _resetVerification(),
              ),
            ),
            const SizedBox(width: 16),
            Expanded(
              flex: 3,
              child: _buildSslToggle(),
            ),
          ],
        ),
        const SizedBox(height: 16),

        // Token field
        _buildTextField(
          controller: _tokenController,
          label: 'Long-Lived Access Token',
          hint: 'eyJ0eXAiOiJKV1QiLCJhbGci...',
          obscureText: _obscureToken,
          prefixIcon: Icons.key_rounded,
          suffixWidget: GestureDetector(
            onTap: () => setState(() => _obscureToken = !_obscureToken),
            child: Padding(
              padding: const EdgeInsets.only(right: 12),
              child: Icon(
                _obscureToken
                    ? Icons.visibility_outlined
                    : Icons.visibility_off_outlined,
                color: CelestialColors.textSecondary,
                size: 20,
              ),
            ),
          ),
          validator: (value) {
            if (value == null || value.trim().isEmpty) {
              return 'Token is required';
            }
            return null;
          },
          onChanged: (_) => _resetVerification(),
        ),

        // Token help text
        Padding(
          padding: const EdgeInsets.only(top: 8, left: 4),
          child: Text(
            'Create a token in Home Assistant: Profile > Long-Lived Access Tokens',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              fontSize: 11,
              height: 1.3,
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildTextField({
    required TextEditingController controller,
    required String label,
    required String hint,
    TextInputType keyboardType = TextInputType.text,
    bool obscureText = false,
    IconData? prefixIcon,
    Widget? suffixWidget,
    String? Function(String?)? validator,
    void Function(String)? onChanged,
  }) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 4, bottom: 8),
          child: Text(
            label,
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.5,
            ),
          ),
        ),
        TextFormField(
          controller: controller,
          keyboardType: keyboardType,
          obscureText: obscureText,
          validator: validator,
          onChanged: onChanged,
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 15,
          ),
          decoration: InputDecoration(
            hintText: hint,
            hintStyle: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            ),
            filled: true,
            fillColor: CelestialColors.backgroundCard,
            prefixIcon: prefixIcon != null
                ? Padding(
                    padding: const EdgeInsets.only(left: 16, right: 12),
                    child: Icon(
                      prefixIcon,
                      color: CelestialColors.textSecondary,
                      size: 20,
                    ),
                  )
                : null,
            prefixIconConstraints: const BoxConstraints(minWidth: 0),
            suffixIcon: suffixWidget,
            suffixIconConstraints: const BoxConstraints(minWidth: 0),
            contentPadding: EdgeInsets.symmetric(
              horizontal: prefixIcon != null ? 0 : 16,
              vertical: 16,
            ),
            border: OutlineInputBorder(
              borderRadius: BorderRadius.circular(14),
              borderSide: BorderSide(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            enabledBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(14),
              borderSide: BorderSide(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            focusedBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(14),
              borderSide: const BorderSide(
                color: CelestialColors.accentBlue,
                width: 1.5,
              ),
            ),
            errorBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(14),
              borderSide: BorderSide(
                color: Colors.red.shade400,
              ),
            ),
            focusedErrorBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(14),
              borderSide: BorderSide(
                color: Colors.red.shade400,
                width: 1.5,
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildSslToggle() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 4, bottom: 8),
          child: Text(
            'Connection',
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.5,
            ),
          ),
        ),
        GestureDetector(
          onTap: () {
            setState(() {
              _useSsl = !_useSsl;
              _resetVerification();
            });
          },
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
            decoration: BoxDecoration(
              color: CelestialColors.backgroundCard,
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            child: Row(
              children: [
                Icon(
                  _useSsl ? Icons.lock_rounded : Icons.lock_open_rounded,
                  color: _useSsl
                      ? Colors.green.shade400
                      : CelestialColors.textSecondary,
                  size: 20,
                ),
                const SizedBox(width: 10),
                Text(
                  _useSsl ? 'HTTPS' : 'HTTP',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                const Spacer(),
                _buildMiniToggle(_useSsl),
              ],
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildMiniToggle(bool value) {
    return Container(
      width: 44,
      height: 26,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(13),
        color: value
            ? Colors.green.shade400.withValues(alpha: 0.3)
            : CelestialColors.orbitRing.withValues(alpha: 0.5),
      ),
      child: Stack(
        children: [
          AnimatedPositioned(
            duration: const Duration(milliseconds: 200),
            curve: Curves.easeOut,
            left: value ? 20 : 2,
            top: 2,
            child: Container(
              width: 22,
              height: 22,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: value
                    ? Colors.green.shade400
                    : CelestialColors.textSecondary,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildStatusIndicator() {
    if (_status == HAConnectionStatus.notConfigured) {
      return const SizedBox.shrink();
    }

    Color statusColor;
    IconData statusIcon;
    String statusText;

    switch (_status) {
      case HAConnectionStatus.connecting:
        statusColor = CelestialColors.sunWarm;
        statusIcon = Icons.sync_rounded;
        statusText = 'Verifying connection...';
      case HAConnectionStatus.connected:
        statusColor = Colors.green.shade400;
        statusIcon = Icons.check_circle_rounded;
        statusText = 'Connected successfully';
      case HAConnectionStatus.error:
        statusColor = Colors.red.shade400;
        statusIcon = Icons.error_rounded;
        statusText = _errorMessage ?? 'Connection failed';
      case HAConnectionStatus.notConfigured:
        return const SizedBox.shrink();
    }

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: statusColor.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: statusColor.withValues(alpha: 0.3),
        ),
      ),
      child: Row(
        children: [
          if (_status == HAConnectionStatus.connecting)
            SizedBox(
              width: 20,
              height: 20,
              child: CircularProgressIndicator(
                strokeWidth: 2,
                valueColor: AlwaysStoppedAnimation(statusColor),
              ),
            )
          else
            Icon(statusIcon, color: statusColor, size: 20),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              statusText,
              style: TextStyle(
                color: statusColor,
                fontSize: 14,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildActionButtons() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Verify button
        GestureDetector(
          onTap: _status == HAConnectionStatus.connecting
              ? null
              : _verifyConnection,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 200),
            padding: const EdgeInsets.symmetric(vertical: 16),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              color: _isVerified
                  ? Colors.green.shade400.withValues(alpha: 0.2)
                  : CelestialColors.backgroundCard,
              border: Border.all(
                color: _isVerified
                    ? Colors.green.shade400.withValues(alpha: 0.5)
                    : CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(
                  _isVerified
                      ? Icons.check_rounded
                      : Icons.wifi_tethering_rounded,
                  color: _isVerified
                      ? Colors.green.shade400
                      : CelestialColors.textSecondary,
                  size: 20,
                ),
                const SizedBox(width: 10),
                Text(
                  _isVerified ? 'Verified' : 'Verify Connection',
                  style: TextStyle(
                    color: _isVerified
                        ? Colors.green.shade400
                        : CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 12),

        // Save button
        GestureDetector(
          onTap: _isVerified ? _saveConfig : null,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 200),
            padding: const EdgeInsets.symmetric(vertical: 16),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              gradient: _isVerified
                  ? const LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [
                        Color(0xFF03A9F4),
                        Color(0xFF0288D1),
                      ],
                    )
                  : null,
              color: _isVerified
                  ? null
                  : CelestialColors.orbitRing.withValues(alpha: 0.3),
              boxShadow: _isVerified
                  ? [
                      BoxShadow(
                        color: const Color(0xFF03A9F4).withValues(alpha: 0.3),
                        blurRadius: 12,
                        offset: const Offset(0, 4),
                      ),
                    ]
                  : null,
            ),
            child: Center(
              child: Text(
                'Save Configuration',
                style: TextStyle(
                  color: _isVerified
                      ? Colors.white
                      : CelestialColors.textSecondary.withValues(alpha: 0.5),
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.3,
                ),
              ),
            ),
          ),
        ),

        // Disconnect button (only show if already configured)
        if (_wasAlreadyConfigured) ...[
          const SizedBox(height: 24),
          GestureDetector(
            onTap: _disconnectHomeAssistant,
            child: Container(
              padding: const EdgeInsets.symmetric(vertical: 16),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(14),
                color: Colors.transparent,
                border: Border.all(
                  color: Colors.red.shade400.withValues(alpha: 0.5),
                ),
              ),
              child: Center(
                child: Text(
                  'Disconnect Home Assistant',
                  style: TextStyle(
                    color: Colors.red.shade400,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ),
          ),
        ],
      ],
    );
  }

  void _resetVerification() {
    if (_isVerified || _status != HAConnectionStatus.notConfigured) {
      setState(() {
        _isVerified = false;
        _status = HAConnectionStatus.notConfigured;
        _errorMessage = null;
      });
    }
  }
}

/// Bottom sheet for network discovery of Home Assistant instances.
class _DiscoveryBottomSheet extends StatefulWidget {
  final void Function(DiscoveredHub hub) onHubSelected;

  const _DiscoveryBottomSheet({required this.onHubSelected});

  @override
  State<_DiscoveryBottomSheet> createState() => _DiscoveryBottomSheetState();
}

class _DiscoveryBottomSheetState extends State<_DiscoveryBottomSheet>
    with SingleTickerProviderStateMixin {
  List<DiscoveredHub>? _discoveredHubs;
  bool _isScanning = false;
  String? _error;

  late AnimationController _scanAnimationController;

  @override
  void initState() {
    super.initState();
    _scanAnimationController = AnimationController(
      duration: const Duration(milliseconds: 1500),
      vsync: this,
    );
    _showPermissionDialog();
  }

  @override
  void dispose() {
    _scanAnimationController.dispose();
    super.dispose();
  }

  void _showPermissionDialog() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      showDialog(
        context: context,
        barrierDismissible: false,
        builder: (context) => AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(20),
          ),
          title: Row(
            children: [
              Container(
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: CelestialColors.accentBlue.withValues(alpha: 0.15),
                  shape: BoxShape.circle,
                ),
                child: const Icon(
                  Icons.wifi_find_rounded,
                  color: CelestialColors.accentBlue,
                  size: 24,
                ),
              ),
              const SizedBox(width: 12),
              const Text(
                'Network Scan',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 18,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          content: const Text(
            'Rhythm needs to scan your local network to find Home Assistant. '
            'This helps auto-detect your server without manual configuration.',
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 14,
              height: 1.5,
            ),
          ),
          actions: [
            TextButton(
              onPressed: () {
                Navigator.of(context).pop();
                Navigator.of(this.context).pop();
              },
              child: Text(
                'Enter Manually',
                style: TextStyle(
                  color: CelestialColors.textSecondary,
                ),
              ),
            ),
            TextButton(
              onPressed: () {
                Navigator.of(context).pop();
                _startScan();
              },
              child: const Text(
                'Scan Now',
                style: TextStyle(
                  color: CelestialColors.accentBlue,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ],
        ),
      );
    });
  }

  Future<void> _startScan() async {
    setState(() {
      _isScanning = true;
      _error = null;
      _discoveredHubs = null;
    });

    _scanAnimationController.repeat();

    try {
      final hubs = await HubDiscoveryService.discoverHomeAssistant(
        timeout: const Duration(seconds: 5),
      );

      setState(() {
        _discoveredHubs = hubs;
        _isScanning = false;
      });
      _scanAnimationController.stop();
    } catch (e) {
      setState(() {
        _error = 'Scan failed. Check network permissions.';
        _isScanning = false;
      });
      _scanAnimationController.stop();
    }
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark,
        borderRadius: const BorderRadius.vertical(top: Radius.circular(24)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          // Handle
          Container(
            margin: const EdgeInsets.only(top: 12),
            width: 40,
            height: 4,
            decoration: BoxDecoration(
              color: CelestialColors.orbitRing,
              borderRadius: BorderRadius.circular(2),
            ),
          ),
          // Header
          Padding(
            padding: const EdgeInsets.all(20),
            child: Row(
              children: [
                const Text(
                  'Discovered Instances',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 18,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const Spacer(),
                if (!_isScanning)
                  GestureDetector(
                    onTap: _startScan,
                    child: Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 12, vertical: 6),
                      decoration: BoxDecoration(
                        color:
                            CelestialColors.accentBlue.withValues(alpha: 0.15),
                        borderRadius: BorderRadius.circular(8),
                      ),
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Icon(
                            Icons.refresh_rounded,
                            size: 16,
                            color: CelestialColors.accentBlue,
                          ),
                          const SizedBox(width: 4),
                          Text(
                            'Rescan',
                            style: TextStyle(
                              color: CelestialColors.accentBlue,
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
              ],
            ),
          ),
          // Content
          Container(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.of(context).size.height * 0.4,
            ),
            child: _buildContent(),
          ),
          SizedBox(height: MediaQuery.of(context).padding.bottom + 16),
        ],
      ),
    );
  }

  Widget _buildContent() {
    if (_isScanning) {
      return Padding(
        padding: const EdgeInsets.all(40),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            RotationTransition(
              turns: _scanAnimationController,
              child: Container(
                width: 60,
                height: 60,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(
                    color: CelestialColors.accentBlue,
                    width: 3,
                  ),
                ),
                child: const Icon(
                  Icons.wifi_find_rounded,
                  color: CelestialColors.accentBlue,
                  size: 28,
                ),
              ),
            ),
            const SizedBox(height: 20),
            const Text(
              'Scanning network...',
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 16,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: 4),
            Text(
              'Looking for Home Assistant instances',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 13,
              ),
            ),
          ],
        ),
      );
    }

    if (_error != null) {
      return Padding(
        padding: const EdgeInsets.all(40),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.error_outline_rounded,
              color: Colors.red.shade400,
              size: 48,
            ),
            const SizedBox(height: 16),
            Text(
              _error!,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
              ),
            ),
          ],
        ),
      );
    }

    if (_discoveredHubs == null) {
      return const SizedBox(height: 100);
    }

    if (_discoveredHubs!.isEmpty) {
      return Padding(
        padding: const EdgeInsets.all(40),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.search_off_rounded,
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
              size: 48,
            ),
            const SizedBox(height: 16),
            const Text(
              'No instances found',
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 16,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: 4),
            Text(
              'Try entering the address manually',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 13,
              ),
            ),
          ],
        ),
      );
    }

    return ListView.builder(
      shrinkWrap: true,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      itemCount: _discoveredHubs!.length,
      itemBuilder: (context, index) {
        final hub = _discoveredHubs![index];
        return GestureDetector(
          onTap: () => widget.onHubSelected(hub),
          child: Container(
            margin: const EdgeInsets.only(bottom: 8),
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(
              color: CelestialColors.backgroundCard,
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            child: Row(
              children: [
                Container(
                  width: 44,
                  height: 44,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    gradient: const LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [
                        Color(0xFF03A9F4),
                        Color(0xFF0288D1),
                      ],
                    ),
                  ),
                  child: const Icon(
                    Icons.home_rounded,
                    color: Colors.white,
                    size: 22,
                  ),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        hub.name ?? 'Home Assistant',
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        '${hub.address}:${hub.port}',
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.8),
                          fontSize: 13,
                          fontFamily: 'monospace',
                        ),
                      ),
                    ],
                  ),
                ),
                Icon(
                  Icons.chevron_right_rounded,
                  color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                  size: 24,
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}
