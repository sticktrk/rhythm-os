import 'package:flutter/material.dart';

import '../utils/app_color_temperature.dart';
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
        selectedColor: CelestialColors.accentBlue.withValues(alpha: 0.22),
        checkmarkColor: CelestialColors.accentBlue,
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
                backgroundColor: colors.primary.withValues(alpha: 0.2),
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
    this.canSendEmpty = false,
    this.leading,
    this.hint = 'Message',
    this.sendLabel = 'Send',
    this.maxLength,
  });
  final TextEditingController controller;
  final ValueChanged<String>? onSend;
  final String hint, sendLabel;
  final bool canSendEmpty;
  final Widget? leading;
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
          prefixIcon: leading,
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
              onPressed:
                  onSend == null || (!canSendEmpty && value.text.trim().isEmpty)
                      ? null
                      : () => onSend!(value.text),
            ),
          ),
        ),
      ),
    );
  }
}

/// The opening of a lighting conversation: one question, a few prompts to
/// start from, and what leaves the phone.
class LightingChatWelcome extends StatelessWidget {
  const LightingChatWelcome({
    super.key,
    required this.tools,
    required this.onPrompt,
    required this.disclosure,
  });

  /// Hub operations this conversation may propose. Prompts follow them.
  final Set<String> tools;
  final ValueChanged<String>? onPrompt;
  final String disclosure;

  @override
  Widget build(BuildContext context) {
    final prompts = <(IconData, String, String)>[
      (
        Icons.lightbulb_outline,
        'Check the lights',
        'What are my lights doing right now?'
      ),
      if (tools.contains('adjust_room_brightness'))
        (
          Icons.brightness_6_outlined,
          'A little brighter',
          'Make the lights a little brighter.'
        ),
      if (tools.contains('adjust_room_warmth'))
        (
          Icons.wb_twilight,
          'Warm it up',
          'Make the light a little warmer and keep the brightness.'
        ),
      if (tools.contains('set_room_color'))
        (
          Icons.palette_outlined,
          'Try a color',
          'Suggest a one-minute amber color preview.'
        ),
    ];
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      SizedBox(
          height: (MediaQuery.sizeOf(context).height * 0.12 -
                  MediaQuery.viewInsetsOf(context).bottom)
              .clamp(0.0, 96.0)),
      const Text('How should the light feel?',
          style: TextStyle(
              fontSize: 24,
              fontWeight: FontWeight.w600,
              height: 1.2,
              color: CelestialColors.textPrimary)),
      const SizedBox(height: 14),
      Wrap(spacing: 8, runSpacing: 4, children: [
        for (final p in prompts)
          ActionChip(
              avatar: Icon(p.$1),
              label: Text(p.$2),
              onPressed: onPrompt == null ? null : () => onPrompt!(p.$3)),
      ]),
      const SizedBox(height: 14),
      AssistantNote(disclosure),
    ]);
  }
}

/// Quiet supporting copy inside a conversation.
class AssistantNote extends StatelessWidget {
  const AssistantNote(this.text, {super.key});
  final String text;

  @override
  Widget build(BuildContext context) => Text(text,
      style: const TextStyle(
          color: CelestialColors.textSecondary, fontSize: 13, height: 1.45));
}

/// The in-conversation progress line.
class AssistantWorking extends StatelessWidget {
  const AssistantWorking({super.key, this.label = 'Working…'});
  final String label;

  @override
  Widget build(BuildContext context) => Row(children: [
        const SizedBox.square(
            dimension: 16, child: CircularProgressIndicator(strokeWidth: 2)),
        const SizedBox(width: 10),
        Expanded(child: Semantics(liveRegion: true, child: Text(label))),
      ]);
}

/// The bordered surface for anything the resident reviews or follows up on.
class AssistantReviewCard extends StatelessWidget {
  const AssistantReviewCard({super.key, required this.children});
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Card(
      color: CelestialColors.backgroundCard,
      margin: EdgeInsets.zero,
      elevation: 0,
      shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(16),
          side: const BorderSide(color: CelestialColors.orbitRing)),
      child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: children)));
}

/// What a reviewed change touches and what it does, led by a swatch of the
/// proposed light when the change has one.
class AssistantReviewHeader extends StatelessWidget {
  const AssistantReviewHeader({
    super.key,
    required this.target,
    required this.title,
    required this.operation,
    this.arguments = const {},
  });
  final String target, title, operation;
  final Map<String, dynamic> arguments;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final red = arguments['red'],
        green = arguments['green'],
        blue = arguments['blue'],
        kelvin = arguments['kelvin'];
    final swatch = switch (operation) {
      'set_room_color' when red is int && green is int && blue is int =>
        Color.fromRGBO(red, green, blue, 1),
      'adjust_room_warmth' when kelvin is int =>
        AppColorTemperature.toColor(kelvin),
      _ => null
    };
    final icon = operation.contains('motion')
        ? Icons.sensors
        : operation.contains('schedule') || operation == 'try_room_offset'
            ? Icons.schedule
            : operation.contains('scene')
                ? Icons.palette_outlined
                : Icons.lightbulb_outline;
    return Row(children: [
      CircleAvatar(
          backgroundColor:
              swatch ?? CelestialColors.sunWarm.withValues(alpha: 0.18),
          child: swatch == null
              ? Icon(icon, color: CelestialColors.sunWarm)
              : null),
      const SizedBox(width: 12),
      Expanded(
          child:
              Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(target, style: theme.textTheme.labelLarge),
        Text(title, style: theme.textTheme.titleLarge),
      ])),
    ]);
  }
}
