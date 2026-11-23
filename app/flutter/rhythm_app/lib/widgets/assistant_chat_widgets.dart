import 'package:flutter/material.dart';

import 'solar_orbit.dart' show CelestialColors;

/// The shared navigation and surfaces for Pro and its conversations.
class RhythmAssistantPage extends StatelessWidget {
  const RhythmAssistantPage({
    super.key,
    required this.title,
    required this.body,
    this.actions = const [],
  });

  final String title;
  final Widget body;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    const colors = ColorScheme.dark(
      primary: CelestialColors.accentBlue,
      onPrimary: CelestialColors.backgroundDark,
      secondary: CelestialColors.sunWarm,
      surface: CelestialColors.backgroundDark,
      onSurface: CelestialColors.textPrimary,
      onSurfaceVariant: CelestialColors.textSecondary,
      surfaceContainer: CelestialColors.backgroundCard,
      surfaceContainerLow: CelestialColors.backgroundCard,
      outline: CelestialColors.orbitRing,
      outlineVariant: CelestialColors.orbitRing,
    );
    final theme = Theme.of(context).copyWith(
      colorScheme: colors,
      scaffoldBackgroundColor: colors.surface,
      textTheme: Theme.of(context).textTheme.apply(
            bodyColor: colors.onSurface,
            displayColor: colors.onSurface,
          ),
      chipTheme: ChipThemeData(
        backgroundColor: CelestialColors.backgroundCard,
        side: const BorderSide(color: CelestialColors.orbitRing),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
        labelStyle: Theme.of(context)
            .textTheme
            .labelLarge
            ?.copyWith(color: CelestialColors.textPrimary),
        iconTheme:
            const IconThemeData(color: CelestialColors.sunWarm, size: 18),
      ),
    );
    return Theme(
      data: theme,
      child: Scaffold(
        appBar: AppBar(
          backgroundColor: colors.surface,
          surfaceTintColor: Colors.transparent,
          centerTitle: true,
          leadingWidth: 64,
          leading: Center(
            child: IconButton(
              tooltip: MaterialLocalizations.of(context).backButtonTooltip,
              onPressed: () => Navigator.maybePop(context),
              style: IconButton.styleFrom(
                backgroundColor: colors.primary.withValues(alpha: 0.15),
                foregroundColor: colors.primary,
              ),
              icon: const Icon(Icons.chevron_left_rounded),
            ),
          ),
          title: Text(title,
              style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 18,
                  fontWeight: FontWeight.w600)),
          actions: actions,
        ),
        body: SafeArea(
            top: false,
            child: Center(
              child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 640),
                  child: SizedBox.expand(child: body)),
            )),
      ),
    );
  }
}

class AssistantMessageBubble extends StatelessWidget {
  const AssistantMessageBubble(
      {super.key, required this.resident, required this.text});
  final bool resident;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: resident ? Alignment.centerRight : Alignment.centerLeft,
      child: Container(
        constraints: const BoxConstraints(maxWidth: 490),
        margin: EdgeInsets.only(
            left: resident ? 32 : 0, right: resident ? 0 : 16, bottom: 14),
        padding: resident
            ? const EdgeInsets.symmetric(horizontal: 15, vertical: 11)
            : const EdgeInsets.symmetric(vertical: 4),
        decoration: resident
            ? BoxDecoration(
                color: CelestialColors.accentBlue.withValues(alpha: 0.16),
                borderRadius: BorderRadius.circular(16),
              )
            : null,
        child: SelectableText(text,
            style: const TextStyle(
                color: CelestialColors.textPrimary, fontSize: 16, height: 1.4)),
      ),
    );
  }
}

class AssistantComposer extends StatelessWidget {
  const AssistantComposer({
    super.key,
    required this.controller,
    required this.onSend,
    this.hint = 'Message',
    this.sendLabel = 'Send',
    this.maxLength,
  });
  final TextEditingController controller;
  final ValueChanged<String>? onSend;
  final String hint, sendLabel;
  final int? maxLength;

  @override
  Widget build(BuildContext context) {
    OutlineInputBorder border(Color color) => OutlineInputBorder(
          borderRadius: BorderRadius.circular(24),
          borderSide: BorderSide(color: color),
        );
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 6, 16, 12),
      child: TextField(
        controller: controller,
        enabled: onSend != null,
        maxLength: maxLength,
        minLines: 1,
        maxLines: 4,
        textCapitalization: TextCapitalization.sentences,
        textInputAction: TextInputAction.send,
        onSubmitted: onSend,
        style: const TextStyle(color: CelestialColors.textPrimary),
        decoration: InputDecoration(
          hintText: hint,
          counterText: '',
          hintStyle: const TextStyle(color: CelestialColors.textSecondary),
          filled: true,
          fillColor: CelestialColors.backgroundCard,
          contentPadding:
              const EdgeInsets.symmetric(horizontal: 18, vertical: 13),
          enabledBorder: border(CelestialColors.orbitRing),
          disabledBorder: border(CelestialColors.orbitRing),
          focusedBorder: border(CelestialColors.accentBlue),
          suffixIcon: ValueListenableBuilder<TextEditingValue>(
            valueListenable: controller,
            builder: (context, value, child) => IconButton(
              tooltip: sendLabel,
              color: CelestialColors.accentBlue,
              disabledColor: CelestialColors.textSecondary,
              icon: const Icon(Icons.arrow_circle_up_rounded, size: 30),
              onPressed: onSend == null || value.text.trim().isEmpty
                  ? null
                  : () => onSend!(value.text),
            ),
          ),
        ),
      ),
    );
  }
}
