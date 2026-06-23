import 'package:flutter/material.dart';

/// Slim warning banner shown at the top of [AppShell] while autonomous light
/// control ("Automatic Lighting") is off. Makes the paused state unmistakable
/// so the user doesn't forget their lights aren't following the rhythm, and
/// offers a single-tap re-enable.
class DisabledModeBanner extends StatefulWidget {
  /// Re-enables autonomous light control.
  final VoidCallback onEnable;

  /// Whether to pad for the status-bar inset. True when this banner sits at
  /// the very top of the shell (the common case).
  final bool consumeTopInset;

  const DisabledModeBanner({
    super.key,
    required this.onEnable,
    this.consumeTopInset = true,
  });

  @override
  State<DisabledModeBanner> createState() => _DisabledModeBannerState();
}

class _DisabledModeBannerState extends State<DisabledModeBanner>
    with SingleTickerProviderStateMixin {
  static const _warnRed = Color(0xFFEF4444);

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
    final topInset =
        widget.consumeTopInset ? MediaQuery.of(context).padding.top : 0.0;

    return Material(
      color: Colors.transparent,
      child: Container(
        padding: EdgeInsets.fromLTRB(16, topInset + 6, 8, 6),
        decoration: BoxDecoration(
          color: const Color(0xFF1A1A2E),
          border: Border(
            bottom: BorderSide(
              color: _warnRed.withValues(alpha: 0.35),
              width: 1,
            ),
          ),
          boxShadow: [
            BoxShadow(
              color: _warnRed.withValues(alpha: 0.12),
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
                    color: _warnRed,
                    boxShadow: [
                      BoxShadow(
                        color: _warnRed.withValues(alpha: 0.4 + 0.3 * v),
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
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'AUTOMATIC LIGHTING OFF',
                    style: TextStyle(
                      color: Color(0xFFE6EDF3),
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 2.2,
                    ),
                  ),
                  SizedBox(height: 2),
                  Text(
                    'Rhythm, switches & motion are paused',
                    style: TextStyle(
                      color: Color(0xFF9DA7B3),
                      fontSize: 11,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
              ),
            ),
            TextButton.icon(
              onPressed: widget.onEnable,
              style: TextButton.styleFrom(
                foregroundColor: _warnRed,
                padding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                minimumSize: const Size(0, 32),
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(8),
                ),
              ),
              icon: const Icon(Icons.wb_sunny_rounded, size: 16),
              label: const Text(
                'Turn on',
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
