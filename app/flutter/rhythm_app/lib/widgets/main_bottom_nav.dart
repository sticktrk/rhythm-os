import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'solar_orbit.dart' show CelestialColors;

/// Tabs on the global bottom navigation bar.
///
/// Each tab swaps the body of [AppShell] while preserving its own navigator.
enum MainNavTab {
  home,
  light,
  automations,
  devices,
  settings,
}

/// Standard 5-destination bottom navigation bar.
///
/// Matches the celestial dark palette but otherwise behaves like a stock
/// Material 3 [NavigationBar] — labels visible at all times, single accent
/// indicator, no fan-out menus.
class MainBottomNav extends StatelessWidget {
  /// The body tab currently shown.
  final MainNavTab currentBodyTab;

  /// Tap handler. The owner is responsible for switching [currentBodyTab].
  final ValueChanged<MainNavTab> onTabSelected;

  /// Which tabs to render. Letting the parent decide lets us hide tabs
  /// whose dependencies aren't met (e.g. Automations with no synced server).
  final List<MainNavTab> tabs;

  /// Tabs that should render but reject taps — used to keep slots like
  /// Automations visible (so the navbar shape is stable) while their underlying
  /// data isn't ready yet.
  final Set<MainNavTab> disabledTabs;

  const MainBottomNav({
    super.key,
    required this.currentBodyTab,
    required this.onTabSelected,
    this.tabs = const [
      MainNavTab.home,
      MainNavTab.light,
      MainNavTab.automations,
      MainNavTab.devices,
      MainNavTab.settings,
    ],
    this.disabledTabs = const {},
  });

  int get _selectedIndex {
    final i = tabs.indexOf(currentBodyTab);
    return i >= 0 ? i : 0;
  }

  NavigationDestination _destinationFor(MainNavTab tab) {
    final enabled = !disabledTabs.contains(tab);
    return switch (tab) {
      MainNavTab.home => NavigationDestination(
          icon: const Icon(Icons.home_outlined),
          selectedIcon: const Icon(Icons.home_rounded),
          label: 'Home',
          enabled: enabled,
        ),
      MainNavTab.light => NavigationDestination(
          icon: const Icon(Icons.light_mode_outlined),
          selectedIcon: const Icon(Icons.light_mode_rounded),
          label: 'Light',
          enabled: enabled,
        ),
      MainNavTab.automations => NavigationDestination(
          icon: const Icon(Icons.bolt_outlined),
          selectedIcon: const Icon(Icons.bolt),
          label: 'Schedules',
          enabled: enabled,
        ),
      MainNavTab.devices => NavigationDestination(
          icon: const Icon(Icons.lightbulb_outline),
          selectedIcon: const Icon(Icons.lightbulb),
          label: 'Devices',
          enabled: enabled,
        ),
      MainNavTab.settings => NavigationDestination(
          icon: const Icon(Icons.settings_outlined),
          selectedIcon: const Icon(Icons.settings_rounded),
          label: 'Settings',
          enabled: enabled,
        ),
    };
  }

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: const BoxDecoration(
        color: CelestialColors.backgroundCard,
        border: Border(
          top: BorderSide(color: CelestialColors.orbitRing, width: 0.5),
        ),
      ),
      child: NavigationBarTheme(
        data: NavigationBarThemeData(
          backgroundColor: Colors.transparent,
          surfaceTintColor: Colors.transparent,
          shadowColor: Colors.transparent,
          elevation: 0,
          height: 64,
          indicatorColor: CelestialColors.accentBlue.withValues(alpha: 0.18),
          indicatorShape: const StadiumBorder(),
          labelBehavior: NavigationDestinationLabelBehavior.alwaysShow,
          labelTextStyle: WidgetStateProperty.resolveWith((states) {
            final selected = states.contains(WidgetState.selected);
            final disabled = states.contains(WidgetState.disabled);
            final base = selected
                ? CelestialColors.textPrimary
                : CelestialColors.textSecondary;
            return TextStyle(
              fontSize: 11,
              fontWeight: selected ? FontWeight.w600 : FontWeight.w500,
              letterSpacing: 0.2,
              color: disabled ? base.withValues(alpha: 0.38) : base,
            );
          }),
          iconTheme: WidgetStateProperty.resolveWith((states) {
            final selected = states.contains(WidgetState.selected);
            final disabled = states.contains(WidgetState.disabled);
            final base = selected
                ? CelestialColors.textPrimary
                : CelestialColors.textSecondary;
            return IconThemeData(
              size: 22,
              color: disabled ? base.withValues(alpha: 0.38) : base,
            );
          }),
        ),
        child: SafeArea(
          top: false,
          child: NavigationBar(
            selectedIndex: _selectedIndex,
            onDestinationSelected: (i) {
              HapticFeedback.selectionClick();
              onTabSelected(tabs[i]);
            },
            destinations: tabs.map(_destinationFor).toList(),
          ),
        ),
      ),
    );
  }
}
