import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:url_launcher/url_launcher.dart';
import '../../../widgets/solar_orbit.dart'; // For CelestialColors

/// Feedback dialog for collecting user feedback via email.
///
/// Features a warm, inviting design with:
/// - Quick feedback type selection (chips)
/// - Optional message input
/// - Direct email launch
class FeedbackDialog extends StatefulWidget {
  const FeedbackDialog({super.key});

  static void show(BuildContext context) {
    showGeneralDialog(
      context: context,
      barrierDismissible: true,
      barrierLabel: 'Dismiss feedback',
      barrierColor: Colors.black87,
      transitionDuration: const Duration(milliseconds: 400),
      pageBuilder: (context, animation, secondaryAnimation) {
        return const FeedbackDialog();
      },
      transitionBuilder: (context, animation, secondaryAnimation, child) {
        final curve = CurvedAnimation(
          parent: animation,
          curve: Curves.easeOutBack,
          reverseCurve: Curves.easeInQuart,
        );
        return ScaleTransition(
          scale: Tween<double>(begin: 0.85, end: 1.0).animate(curve),
          child: FadeTransition(
            opacity: Tween<double>(begin: 0.0, end: 1.0).animate(
              CurvedAnimation(parent: animation, curve: Curves.easeOut),
            ),
            child: child,
          ),
        );
      },
    );
  }

  @override
  State<FeedbackDialog> createState() => _FeedbackDialogState();
}

class _FeedbackDialogState extends State<FeedbackDialog>
    with SingleTickerProviderStateMixin {
  final _messageController = TextEditingController();
  final _focusNode = FocusNode();
  String? _selectedType;
  bool _isSending = false;
  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  static const _feedbackTypes = [
    ('love', 'Love it', Icons.favorite_rounded, Color(0xFFFF6B9D)),
    ('idea', 'Feature idea', Icons.lightbulb_rounded, Color(0xFFFFC857)),
    ('bug', 'Bug report', Icons.bug_report_rounded, Color(0xFF7C83FD)),
    ('other', 'Other', Icons.chat_bubble_rounded, Color(0xFF58A6FF)),
  ];

  @override
  void initState() {
    super.initState();
    _glowController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2000),
    )..repeat(reverse: true);
    _glowAnimation = Tween<double>(begin: 0.3, end: 0.7).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _messageController.dispose();
    _focusNode.dispose();
    _glowController.dispose();
    super.dispose();
  }

  Future<void> _sendFeedback() async {
    if (_selectedType == null) {
      HapticFeedback.mediumImpact();
      return;
    }

    setState(() => _isSending = true);
    HapticFeedback.lightImpact();

    // Build email subject and body
    final typeLabel = _feedbackTypes
        .firstWhere((t) => t.$1 == _selectedType)
        .$2;
    final subject = Uri.encodeComponent('Rhythm Feedback: $typeLabel');
    final body = Uri.encodeComponent(_messageController.text.isEmpty
        ? 'Type: $typeLabel\n\n[Please describe your feedback here]'
        : 'Type: $typeLabel\n\n${_messageController.text}');

    final emailUri = Uri.parse(
      'mailto:feedback@rhythm.lighting?subject=$subject&body=$body',
    );

    try {
      final canLaunch = await canLaunchUrl(emailUri);
      if (canLaunch) {
        final launched = await launchUrl(
          emailUri,
          mode: LaunchMode.externalApplication,
        );
        if (launched && mounted) {
          Navigator.of(context).pop();
        } else if (mounted) {
          _showCopyFallback();
        }
      } else if (mounted) {
        _showCopyFallback();
      }
    } catch (e) {
      // Show fallback with copy option
      if (mounted) {
        _showCopyFallback();
      }
    } finally {
      if (mounted) {
        setState(() => _isSending = false);
      }
    }
  }

  void _showCopyFallback() {
    showDialog(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(20)),
        title: const Text(
          'Email app not available',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 17,
            fontWeight: FontWeight.w600,
          ),
        ),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(
              'Send your feedback to:',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
            const SizedBox(height: 12),
            GestureDetector(
              onTap: () {
                Clipboard.setData(
                  const ClipboardData(text: 'feedback@rhythm.lighting'),
                );
                HapticFeedback.mediumImpact();
                ScaffoldMessenger.of(context).showSnackBar(
                  SnackBar(
                    content: const Text('Email copied!'),
                    backgroundColor: CelestialColors.accentBlue,
                    behavior: SnackBarBehavior.floating,
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(10),
                    ),
                  ),
                );
              },
              child: Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 12,
                ),
                decoration: BoxDecoration(
                  color: CelestialColors.backgroundDark,
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(
                    color: CelestialColors.accentBlue.withValues(alpha: 0.3),
                  ),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Text(
                      'feedback@rhythm.lighting',
                      style: TextStyle(
                        color: CelestialColors.accentBlue,
                        fontSize: 15,
                        fontWeight: FontWeight.w500,
                        letterSpacing: -0.3,
                      ),
                    ),
                    const SizedBox(width: 8),
                    Icon(
                      Icons.copy_rounded,
                      color: CelestialColors.accentBlue.withValues(alpha: 0.7),
                      size: 18,
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text(
              'Done',
              style: TextStyle(color: CelestialColors.accentBlue),
            ),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Center(
      child: SingleChildScrollView(
        child: Padding(
          padding: EdgeInsets.only(
            left: 24,
            right: 24,
            bottom: MediaQuery.of(context).viewInsets.bottom + 24,
          ),
          child: Material(
            color: Colors.transparent,
            child: AnimatedBuilder(
              animation: _glowAnimation,
              builder: (context, child) {
                return Container(
                  constraints: const BoxConstraints(maxWidth: 380),
                  decoration: BoxDecoration(
                    color: CelestialColors.backgroundCard,
                    borderRadius: BorderRadius.circular(28),
                    boxShadow: [
                      // Outer glow
                      BoxShadow(
                        color: const Color(0xFFFFC857)
                            .withValues(alpha: _glowAnimation.value * 0.15),
                        blurRadius: 40,
                        spreadRadius: 0,
                      ),
                      // Inner shadow for depth
                      BoxShadow(
                        color: Colors.black.withValues(alpha: 0.4),
                        blurRadius: 20,
                        offset: const Offset(0, 10),
                      ),
                    ],
                  ),
                  child: child,
                );
              },
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _buildHeader(),
                    const SizedBox(height: 24),
                    _buildTypeSelector(),
                    const SizedBox(height: 20),
                    _buildMessageInput(),
                    const SizedBox(height: 24),
                    _buildSendButton(),
                    const SizedBox(height: 8),
                    _buildHelpLinks(),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Column(
      children: [
        // Close button row
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            GestureDetector(
              onTap: () => Navigator.of(context).pop(),
              child: Container(
                width: 32,
                height: 32,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: CelestialColors.orbitRing.withValues(alpha: 0.3),
                ),
                child: const Icon(
                  Icons.close_rounded,
                  color: CelestialColors.textSecondary,
                  size: 18,
                ),
              ),
            ),
          ],
        ),
        const SizedBox(height: 8),
        // Icon with glow
        AnimatedBuilder(
          animation: _glowAnimation,
          builder: (context, child) {
            return Container(
              width: 72,
              height: 72,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: RadialGradient(
                  colors: [
                    const Color(0xFFFFC857),
                    const Color(0xFFFFA726).withValues(alpha: 0.8),
                    const Color(0xFFFF8A65).withValues(alpha: 0.4),
                    Colors.transparent,
                  ],
                  stops: const [0.0, 0.3, 0.6, 1.0],
                ),
                boxShadow: [
                  BoxShadow(
                    color: const Color(0xFFFFC857)
                        .withValues(alpha: _glowAnimation.value * 0.5),
                    blurRadius: 30,
                    spreadRadius: 5,
                  ),
                ],
              ),
              child: const Icon(
                Icons.mail_rounded,
                color: Color(0xFF1A1A1A),
                size: 32,
              ),
            );
          },
        ),
        const SizedBox(height: 20),
        // Title
        const Text(
          'Share your thoughts',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
            letterSpacing: -0.5,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          'Help us make Rhythm better',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.8),
            fontSize: 15,
            letterSpacing: -0.2,
          ),
        ),
      ],
    );
  }

  Widget _buildTypeSelector() {
    return Wrap(
      spacing: 10,
      runSpacing: 10,
      alignment: WrapAlignment.center,
      children: _feedbackTypes.map((type) {
        final isSelected = _selectedType == type.$1;
        return GestureDetector(
          onTap: () {
            HapticFeedback.selectionClick();
            setState(() => _selectedType = type.$1);
          },
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 200),
            curve: Curves.easeOutCubic,
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
            decoration: BoxDecoration(
              color: isSelected
                  ? type.$4.withValues(alpha: 0.2)
                  : CelestialColors.backgroundDark,
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                color:
                    isSelected ? type.$4 : CelestialColors.orbitRing.withValues(alpha: 0.3),
                width: isSelected ? 1.5 : 1,
              ),
              boxShadow: isSelected
                  ? [
                      BoxShadow(
                        color: type.$4.withValues(alpha: 0.3),
                        blurRadius: 12,
                        spreadRadius: 0,
                      ),
                    ]
                  : null,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  type.$3,
                  color: isSelected
                      ? type.$4
                      : CelestialColors.textSecondary.withValues(alpha: 0.7),
                  size: 18,
                ),
                const SizedBox(width: 8),
                Text(
                  type.$2,
                  style: TextStyle(
                    color: isSelected
                        ? type.$4
                        : CelestialColors.textSecondary.withValues(alpha: 0.9),
                    fontSize: 14,
                    fontWeight: isSelected ? FontWeight.w600 : FontWeight.w500,
                    letterSpacing: -0.2,
                  ),
                ),
              ],
            ),
          ),
        );
      }).toList(),
    );
  }

  Widget _buildMessageInput() {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 200),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: _focusNode.hasFocus
              ? CelestialColors.accentBlue.withValues(alpha: 0.5)
              : CelestialColors.orbitRing.withValues(alpha: 0.2),
        ),
      ),
      child: TextField(
        controller: _messageController,
        focusNode: _focusNode,
        maxLines: 4,
        minLines: 3,
        style: const TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 15,
          height: 1.5,
        ),
        decoration: InputDecoration(
          hintText: 'Tell us more (optional)...',
          hintStyle: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            fontSize: 15,
          ),
          border: InputBorder.none,
          contentPadding: const EdgeInsets.all(16),
        ),
        onTap: () => setState(() {}),
        onChanged: (_) => setState(() {}),
      ),
    );
  }

  Widget _buildSendButton() {
    final isEnabled = _selectedType != null;
    return GestureDetector(
      onTap: isEnabled && !_isSending ? _sendFeedback : null,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        height: 56,
        decoration: BoxDecoration(
          gradient: isEnabled
              ? const LinearGradient(
                  colors: [Color(0xFFFFC857), Color(0xFFFFB347)],
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                )
              : null,
          color: isEnabled ? null : CelestialColors.orbitRing.withValues(alpha: 0.3),
          borderRadius: BorderRadius.circular(16),
          boxShadow: isEnabled
              ? [
                  BoxShadow(
                    color: const Color(0xFFFFC857).withValues(alpha: 0.4),
                    blurRadius: 20,
                    offset: const Offset(0, 8),
                  ),
                ]
              : null,
        ),
        child: Center(
          child: _isSending
              ? const SizedBox(
                  width: 24,
                  height: 24,
                  child: CircularProgressIndicator(
                    strokeWidth: 2.5,
                    valueColor: AlwaysStoppedAnimation(Color(0xFF1A1A1A)),
                  ),
                )
              : Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      Icons.send_rounded,
                      color: isEnabled
                          ? const Color(0xFF1A1A1A)
                          : CelestialColors.textSecondary.withValues(alpha: 0.5),
                      size: 20,
                    ),
                    const SizedBox(width: 10),
                    Text(
                      'Send Feedback',
                      style: TextStyle(
                        color: isEnabled
                            ? const Color(0xFF1A1A1A)
                            : CelestialColors.textSecondary.withValues(alpha: 0.5),
                        fontSize: 16,
                        fontWeight: FontWeight.w600,
                        letterSpacing: -0.3,
                      ),
                    ),
                  ],
                ),
        ),
      ),
    );
  }

  Widget _buildHelpLinks() {
    return Column(
      children: [
        const SizedBox(height: 8),
        // Help tips section
        GestureDetector(
          onTap: () {
            Navigator.of(context).pop();
            _showHelpTips(context);
          },
          child: Container(
            padding: const EdgeInsets.symmetric(vertical: 12),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(
                  Icons.help_outline_rounded,
                  color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                  size: 18,
                ),
                const SizedBox(width: 8),
                Text(
                  'View help tips',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }

  static void _showHelpTips(BuildContext context) {
    showDialog(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(24)),
        title: Row(
          children: [
            Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: LinearGradient(
                  colors: [
                    const Color(0xFFFFC857),
                    const Color(0xFFFFB347).withValues(alpha: 0.8),
                  ],
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                ),
              ),
              child: const Icon(
                Icons.tips_and_updates_rounded,
                color: Color(0xFF1A1A1A),
                size: 22,
              ),
            ),
            const SizedBox(width: 14),
            const Text(
              'Quick Tips',
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 20,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
        content: const Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _HelpTipItem(
              icon: Icons.touch_app_rounded,
              title: 'Drag the sun',
              description: 'Preview lighting at any time of day',
            ),
            _HelpTipItem(
              icon: Icons.radio_button_checked,
              title: 'Tap the sun',
              description: 'Toggle your lights on or off',
            ),
            _HelpTipItem(
              icon: Icons.tune_rounded,
              title: 'Long-press the sun',
              description: 'Enter tune mode for curve adjustments',
            ),
            _HelpTipItem(
              icon: Icons.bedtime_rounded,
              title: 'Sleep schedule',
              description: 'Set wake/sleep times for smooth transitions',
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text(
              'Got it',
              style: TextStyle(
                color: CelestialColors.accentBlue,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _HelpTipItem extends StatelessWidget {
  final IconData icon;
  final String title;
  final String description;

  const _HelpTipItem({
    required this.icon,
    required this.title,
    required this.description,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: CelestialColors.accentBlue.withValues(alpha: 0.15),
            ),
            child: Icon(
              icon,
              color: CelestialColors.accentBlue,
              size: 16,
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  description,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 13,
                    height: 1.3,
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
