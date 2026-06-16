import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'solar_orbit.dart' show CelestialColors;

/// Celebratory success modal for hub pairing completion.
///
/// Features:
/// - Animated scale + fade entrance
/// - Golden checkmark with pulsing glow
/// - Room count display
/// - "Let's Go" button to dismiss
class SuccessModal extends StatefulWidget {
  final int roomCount;
  final String hubType;

  const SuccessModal({
    super.key,
    required this.roomCount,
    required this.hubType,
  });

  /// Show the success modal as a dialog overlay.
  static Future<void> show(
    BuildContext context, {
    required int roomCount,
    required String hubType,
  }) {
    return showGeneralDialog(
      context: context,
      barrierDismissible: true,
      barrierLabel: 'Success Modal',
      barrierColor: Colors.black54,
      transitionDuration: const Duration(milliseconds: 300),
      pageBuilder: (context, animation, secondaryAnimation) {
        return SuccessModal(
          roomCount: roomCount,
          hubType: hubType,
        );
      },
      transitionBuilder: (context, animation, secondaryAnimation, child) {
        final curvedAnimation = CurvedAnimation(
          parent: animation,
          curve: Curves.easeOutBack,
        );
        return ScaleTransition(
          scale: Tween<double>(begin: 0.8, end: 1.0).animate(curvedAnimation),
          child: FadeTransition(
            opacity: animation,
            child: child,
          ),
        );
      },
    );
  }

  @override
  State<SuccessModal> createState() => _SuccessModalState();
}

class _SuccessModalState extends State<SuccessModal>
    with SingleTickerProviderStateMixin {
  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  @override
  void initState() {
    super.initState();
    _glowController = AnimationController(
      duration: const Duration(milliseconds: 1500),
      vsync: this,
    )..repeat(reverse: true);

    _glowAnimation = Tween<double>(begin: 0.4, end: 0.8).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );

    // Haptic feedback on show
    HapticFeedback.heavyImpact();
  }

  @override
  void dispose() {
    _glowController.dispose();
    super.dispose();
  }

  void _dismiss() {
    HapticFeedback.lightImpact();
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: _dismiss,
      child: Material(
        color: Colors.transparent,
        child: Center(
          child: GestureDetector(
            // Prevent tap-through on the card itself
            onTap: () {},
            child: Container(
              width: 320,
              margin: const EdgeInsets.symmetric(horizontal: 24),
              padding: const EdgeInsets.all(32),
              decoration: BoxDecoration(
                color: CelestialColors.backgroundCard,
                borderRadius: BorderRadius.circular(24),
                border: Border.all(
                  color: const Color(0xFFFFB900).withValues(alpha: 0.3),
                  width: 1,
                ),
                boxShadow: [
                  BoxShadow(
                    color: const Color(0xFFFFB900).withValues(alpha: 0.2),
                    blurRadius: 32,
                    spreadRadius: 4,
                  ),
                ],
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  _buildCheckmark(),
                  const SizedBox(height: 24),
                  _buildText(),
                  const SizedBox(height: 24),
                  _buildButton(),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildCheckmark() {
    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        return Container(
          width: 80,
          height: 80,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            gradient: const LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                Color(0xFFFFB900),
                Color(0xFFFF8C00),
              ],
            ),
            boxShadow: [
              BoxShadow(
                color: const Color(0xFFFFB900).withValues(alpha: _glowAnimation.value),
                blurRadius: 24,
                spreadRadius: 4,
              ),
            ],
          ),
          child: const Icon(
            Icons.check_rounded,
            color: Colors.white,
            size: 48,
          ),
        );
      },
    );
  }

  Widget _buildText() {
    final isHubPairing = widget.hubType == 'RhythmServer' || widget.hubType == 'Philips Hue';
    final title = isHubPairing ? 'Successfully paired' : 'Success!';
    final subtitleText = isHubPairing
        ? null
        : widget.roomCount == 0
            ? 'No rooms found'
            : '${widget.roomCount} room${widget.roomCount == 1 ? '' : 's'} synced';

    return Column(
      children: [
        Text(
          title,
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 24,
            fontWeight: FontWeight.bold,
          ),
        ),
        if (subtitleText != null) ...[
          const SizedBox(height: 8),
          Text(
            subtitleText,
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 14,
            ),
          ),
        ],
      ],
    );
  }

  Widget _buildButton() {
    final buttonLabel = (widget.hubType == 'RhythmServer' || widget.hubType == 'Philips Hue' || widget.roomCount > 0) ? "Let's Go" : 'Continue';

    return GestureDetector(
      onTap: _dismiss,
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          gradient: const LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              Color(0xFFFFB900),
              Color(0xFFFF8C00),
            ],
          ),
          boxShadow: [
            BoxShadow(
              color: const Color(0xFFFFB900).withValues(alpha: 0.3),
              blurRadius: 12,
              offset: const Offset(0, 4),
            ),
          ],
        ),
        child: Text(
          buttonLabel,
          textAlign: TextAlign.center,
          style: const TextStyle(
            color: Colors.white,
            fontSize: 15,
            fontWeight: FontWeight.w600,
          ),
        ),
      ),
    );
  }
}
