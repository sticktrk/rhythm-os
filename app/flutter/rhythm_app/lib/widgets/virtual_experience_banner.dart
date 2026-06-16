import 'package:flutter/material.dart';

/// Slim banner shown at the top of [AppShell] while the user is in the
/// Virtual Experience. Makes the mode unmistakable and offers a single-tap
/// exit back to the welcome screen.
class VirtualExperienceBanner extends StatefulWidget {
  final VoidCallback onExit;

  const VirtualExperienceBanner({super.key, required this.onExit});

  @override
  State<VirtualExperienceBanner> createState() =>
      _VirtualExperienceBannerState();
}

class _VirtualExperienceBannerState extends State<VirtualExperienceBanner>
    with SingleTickerProviderStateMixin {
  static const _sunWarm = Color(0xFFF9A825);

  late final AnimationController _pulse;

  @override
  void initState() {
    super.initState();
    _pulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2400),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final topInset = MediaQuery.of(context).padding.top;

    return Material(
      color: Colors.transparent,
      child: Container(
        padding: EdgeInsets.fromLTRB(16, topInset + 6, 8, 6),
        decoration: BoxDecoration(
          color: const Color(0xFF1A1A2E),
          border: Border(
            bottom: BorderSide(
              color: _sunWarm.withValues(alpha: 0.35),
              width: 1,
            ),
          ),
          boxShadow: [
            BoxShadow(
              color: _sunWarm.withValues(alpha: 0.12),
              blurRadius: 14,
              offset: const Offset(0, 2),
            ),
          ],
        ),
        child: Row(
          children: [
            AnimatedBuilder(
              animation: _pulse,
              builder: (context, _) {
                final v = _pulse.value;
                return Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: _sunWarm,
                    boxShadow: [
                      BoxShadow(
                        color: _sunWarm.withValues(alpha: 0.4 + 0.3 * v),
                        blurRadius: 6 + 4 * v,
                        spreadRadius: 0.5,
                      ),
                    ],
                  ),
                );
              },
            ),
            const SizedBox(width: 10),
            const Expanded(
              child: Text(
                'VIRTUAL EXPERIENCE',
                style: TextStyle(
                  color: Color(0xFFE6EDF3),
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 2.2,
                ),
              ),
            ),
            TextButton.icon(
              onPressed: widget.onExit,
              style: TextButton.styleFrom(
                foregroundColor: _sunWarm,
                padding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                minimumSize: const Size(0, 32),
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(8),
                ),
              ),
              icon: const Icon(Icons.logout_rounded, size: 16),
              label: const Text(
                'Exit',
                style: TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.3,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
