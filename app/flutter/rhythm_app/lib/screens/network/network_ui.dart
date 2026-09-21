import 'package:flutter/material.dart';

import '../../widgets/solar_orbit.dart';

/// Shared presentation for the saved-network screens, matching the Rhythm
/// Server settings pages: dark page, circular back control, bordered cards.
const networkTeal = Color(0xFF00BCD4);

class NetworkScaffold extends StatelessWidget {
  const NetworkScaffold({
    super.key,
    required this.title,
    required this.children,
    this.busy = false,
    this.footer,
  });
  final String title;
  final List<Widget> children;
  final bool busy;

  /// Pinned primary action, kept above the keyboard and home indicator.
  final Widget? footer;

  @override
  Widget build(BuildContext context) => Scaffold(
        backgroundColor: CelestialColors.backgroundDark,
        body: SafeArea(
          child: Column(
            children: [
              Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                child: Row(
                  children: [
                    Semantics(
                      button: true,
                      label: 'Back',
                      child: GestureDetector(
                        onTap: () => Navigator.of(context).maybePop(),
                        child: Container(
                          width: 40,
                          height: 40,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: networkTeal.withValues(alpha: 0.15),
                            border: Border.all(
                              color: networkTeal.withValues(alpha: 0.3),
                            ),
                          ),
                          child: const Icon(Icons.chevron_left,
                              color: networkTeal, size: 24),
                        ),
                      ),
                    ),
                    Expanded(
                      child: Text(
                        title,
                        textAlign: TextAlign.center,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 18,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.3,
                        ),
                      ),
                    ),
                    const SizedBox(width: 40),
                  ],
                ),
              ),
              // Reserve the height so content never jumps when work starts.
              SizedBox(
                height: 2,
                child: busy
                    ? const LinearProgressIndicator(
                        minHeight: 2,
                        color: networkTeal,
                        backgroundColor: Colors.transparent,
                      )
                    : null,
              ),
              Expanded(
                child: ListView(
                  padding: const EdgeInsets.fromLTRB(24, 10, 24, 32),
                  children: children,
                ),
              ),
              if (footer != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(24, 8, 24, 16),
                  child: footer,
                ),
            ],
          ),
        ),
      );
}

class NetworkSectionHeader extends StatelessWidget {
  const NetworkSectionHeader(this.title, {super.key});
  final String title;
  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(left: 4, top: 24, bottom: 10),
        child: Text(
          title,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.6),
            fontSize: 13,
            fontWeight: FontWeight.w500,
            letterSpacing: 0.8,
          ),
        ),
      );
}

class NetworkBodyText extends StatelessWidget {
  const NetworkBodyText(this.text, {super.key, this.color});
  final String text;
  final Color? color;
  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(
          color: color ?? CelestialColors.textSecondary,
          fontSize: 13,
          height: 1.4,
        ),
      );
}

/// A settings-style row: icon badge, title, optional subtitle, trailing.
class NetworkCard extends StatelessWidget {
  const NetworkCard({
    super.key,
    required this.icon,
    required this.title,
    this.subtitle,
    this.trailing,
    this.onTap,
    this.highlighted = false,
    this.accent = networkTeal,
  });
  final IconData icon;
  final String title;
  final String? subtitle;
  final Widget? trailing;
  final VoidCallback? onTap;
  final bool highlighted;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final tint = highlighted ? accent : CelestialColors.textSecondary;
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Material(
        color: CelestialColors.backgroundCard,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: BorderSide(
            color: highlighted
                ? accent.withValues(alpha: 0.45)
                : CelestialColors.orbitRing.withValues(alpha: 0.5),
          ),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 14, 8, 14),
            child: Row(
              children: [
                Container(
                  width: 40,
                  height: 40,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: tint.withValues(alpha: 0.16),
                  ),
                  child: Icon(icon, color: tint, size: 20),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      if (subtitle != null) ...[
                        const SizedBox(height: 4),
                        Text(
                          subtitle!,
                          style: const TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 13,
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
                if (trailing != null) trailing! else const SizedBox(width: 8),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Inline notice for errors and recovery guidance.
class NetworkNotice extends StatelessWidget {
  const NetworkNotice({super.key, required this.text, this.action});
  final String text;
  final Widget? action;
  @override
  Widget build(BuildContext context) => Container(
        margin: const EdgeInsets.only(top: 16),
        padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
        decoration: BoxDecoration(
          color: CelestialColors.warning.withValues(alpha: 0.08),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: CelestialColors.warning.withValues(alpha: 0.3),
          ),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Padding(
              padding: EdgeInsets.only(top: 1),
              child: Icon(Icons.info_outline_rounded,
                  color: CelestialColors.warning, size: 18),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: NetworkBodyText(text, color: CelestialColors.textPrimary),
            ),
            if (action != null) action!,
          ],
        ),
      );
}

final networkPrimaryButtonStyle = FilledButton.styleFrom(
  backgroundColor: networkTeal,
  foregroundColor: CelestialColors.backgroundDark,
  disabledBackgroundColor: CelestialColors.orbitRing.withValues(alpha: 0.5),
  disabledForegroundColor: CelestialColors.textSecondary,
  minimumSize: const Size.fromHeight(50),
  shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
);

final networkSecondaryButtonStyle = OutlinedButton.styleFrom(
  foregroundColor: networkTeal,
  minimumSize: const Size.fromHeight(50),
  side: BorderSide(color: networkTeal.withValues(alpha: 0.3)),
  backgroundColor: networkTeal.withValues(alpha: 0.08),
  shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
);
