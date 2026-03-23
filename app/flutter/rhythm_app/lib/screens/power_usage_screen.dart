/// Power Usage estimation screen.
///
/// Uses room data from the connected server (light counts per room) and
/// integrates the Rhythm brightness curve over 24 hours to estimate daily
/// power consumption. Assumes all lights are on all the time at 9W each.
library;

import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../models/config_model.dart';
import '../providers/home_provider.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/server_http_client.dart';
import '../services/settings_service.dart';

// =============================================================================
// Data Models
// =============================================================================

class _LightInfo {
  final String id;
  final String name;
  final double estimatedWatts;

  const _LightInfo({
    required this.id,
    required this.name,
    required this.estimatedWatts,
  });
}

class _RoomPowerData {
  final String roomId;
  final String roomName;
  final List<_LightInfo> lights;
  final double dailyKwh;

  const _RoomPowerData({
    required this.roomId,
    required this.roomName,
    required this.lights,
    required this.dailyKwh,
  });

  double get totalWatts => lights.fold(0.0, (s, l) => s + l.estimatedWatts);
}

class _PowerUsageData {
  final List<_RoomPowerData> rooms;
  final List<double> hourlyWatts;
  final List<double> hourlyHours;
  final double totalDailyKwh;
  final int totalLights;

  const _PowerUsageData({
    required this.rooms,
    required this.hourlyWatts,
    required this.hourlyHours,
    required this.totalDailyKwh,
    required this.totalLights,
  });
}

// =============================================================================
// Power Usage Screen
// =============================================================================

class PowerUsageScreen extends StatefulWidget {
  const PowerUsageScreen({super.key});

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const PowerUsageScreen();
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
  State<PowerUsageScreen> createState() => _PowerUsageScreenState();
}

class _PowerUsageScreenState extends State<PowerUsageScreen> {
  static const _defaultWattsPerLight = 9.0;

  _PowerUsageData? _data;
  bool _loading = true;
  String? _error;
  double? _rate;
  late final TextEditingController _rateController;

  @override
  void initState() {
    super.initState();
    _rate = SettingsService.instance.electricityRate;
    _rateController = TextEditingController(
      text: _rate != null ? _rate!.toStringAsFixed(2) : '',
    );
    _loadData();
  }

  @override
  void dispose() {
    _rateController.dispose();
    super.dispose();
  }

  Future<void> _loadData() async {
    setState(() {
      _loading = true;
      _error = null;
    });

    try {
      final serverSync = context.read<ServerSyncProvider>();
      final homeProvider = context.read<HomeProvider>();
      final config = context.read<ConfigModel>().config;

      if (!serverSync.synced) {
        setState(() {
          _error = 'Connect to a server to view power usage';
          _loading = false;
        });
        return;
      }

      // Use cached hello rooms (direct from server registry, includes typed devices
      // when the hub is connected). Fall back to RoomProvider if hello hasn't arrived.
      var serverRooms = serverSync.helloRooms;
      if (serverRooms.isEmpty) {
        final roomProvider = context.read<RoomProvider>();
        final providerRooms = roomProvider.rooms;
        // Convert RoomDto → lightweight records for the same code path
        serverRooms = providerRooms
            .map((r) => ServerRoom(
                  id: r.id,
                  name: r.name,
                  groupedLightId: '',
                  rhythmEnabled: r.rhythmEnabled,
                  disabled: r.disabled,
                  timeOffset: r.timeOffsetMinutes,
                  brightnessOffset: r.brightnessOffset,
                  softOff: false,
                  deviceIds: r.deviceIds,
                ))
            .toList();
      }

      if (serverRooms.isEmpty) {
        setState(() {
          _error = 'No rooms found';
          _loading = false;
        });
        return;
      }

      final location = await homeProvider.resolveLocation();

      // Build light info from device_ids.
      // device_ids contains all devices; typed devices are buttons/motion only.
      // The difference is the light count.
      final roomDataList = <({String id, String name, List<_LightInfo> lights})>[];
      for (final room in serverRooms) {
        final lightCount = room.lightCount > 0 ? room.lightCount : 1;
        // Extract light device IDs: everything in deviceIds that isn't a typed device
        final typedIds = room.devices.map((d) => d.id).toSet();
        final lightIds = room.deviceIds.where((id) => !typedIds.contains(id)).toList();
        final lights = <_LightInfo>[];
        for (int i = 0; i < lightCount; i++) {
          lights.add(_LightInfo(
            id: i < lightIds.length ? lightIds[i] : '${room.id}_$i',
            name: '${room.name} light ${i + 1}',
            estimatedWatts: _defaultWattsPerLight,
          ));
        }
        roomDataList.add((id: room.id, name: room.name, lights: lights));
      }

      final data = _calculatePower(
        roomDataList: roomDataList,
        curveConfig: config,
        latitude: location.latitude,
        longitude: location.longitude,
        timezone: location.timezone,
      );

      if (mounted) {
        setState(() {
          _data = data;
          _loading = false;
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = e.toString();
          _loading = false;
        });
      }
    }
  }

  _PowerUsageData _calculatePower({
    required List<({String id, String name, List<_LightInfo> lights})> roomDataList,
    required CurveConfigDto curveConfig,
    required double latitude,
    required double longitude,
    required String timezone,
  }) {
    final now = DateTime.now();

    // Generate global brightness curve
    final curveData = generateCurveDataWithSunTimes(
      config: curveConfig,
      latitude: latitude,
      longitude: longitude,
      year: now.year,
      month: now.month,
      day: now.day,
      timezone: timezone,
    );

    // Calculate per-room power
    final roomPowerList = <_RoomPowerData>[];
    var totalWattsAtPeak = 0.0;
    var totalLights = 0;

    for (final room in roomDataList) {
      final roomWatts =
          room.lights.fold(0.0, (s, l) => s + l.estimatedWatts);
      totalWattsAtPeak += roomWatts;
      totalLights += room.lights.length;

      // Integrate brightness curve * wattage
      var energyWh = 0.0;
      for (int i = 1; i < curveData.hours.length; i++) {
        final dt = curveData.hours[i] - curveData.hours[i - 1];
        final avgBri =
            (curveData.brightness[i] + curveData.brightness[i - 1]) / 2.0;
        energyWh += (avgBri / 100.0) * roomWatts * dt;
      }

      roomPowerList.add(_RoomPowerData(
        roomId: room.id,
        roomName: room.name,
        lights: room.lights,
        dailyKwh: energyWh / 1000.0,
      ));
    }

    roomPowerList.sort((a, b) => b.dailyKwh.compareTo(a.dailyKwh));

    // Build hourly watts for chart
    final hourlyWatts = <double>[];
    final hourlyHours = <double>[];
    for (int i = 0; i < curveData.hours.length; i++) {
      hourlyHours.add(curveData.hours[i]);
      hourlyWatts
          .add((curveData.brightness[i] / 100.0) * totalWattsAtPeak);
    }

    final totalDailyKwh =
        roomPowerList.fold(0.0, (s, r) => s + r.dailyKwh);

    return _PowerUsageData(
      rooms: roomPowerList,
      hourlyWatts: hourlyWatts,
      hourlyHours: hourlyHours,
      totalDailyKwh: totalDailyKwh,
      totalLights: totalLights,
    );
  }

  void _onRateChanged(String text) {
    final parsed = double.tryParse(text);
    setState(() => _rate = parsed);
    SettingsService.instance.setElectricityRate(parsed);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _P.bg,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(child: _buildBody()),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
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
                color: _P.accent.withValues(alpha: 0.12),
                border: Border.all(
                  color: _P.accent.withValues(alpha: 0.25),
                ),
              ),
              child: const Icon(Icons.close, color: _P.accent, size: 20),
            ),
          ),
          const Expanded(
            child: Text(
              'Power Usage',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _P.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildBody() {
    if (_loading) {
      return const Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircularProgressIndicator(
              strokeWidth: 2,
              valueColor: AlwaysStoppedAnimation(_P.accent),
            ),
            SizedBox(height: 16),
            Text(
              'Calculating...',
              style: TextStyle(color: _P.textSecondary, fontSize: 14),
            ),
          ],
        ),
      );
    }

    if (_error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                  Icons.bolt, color: _P.warm.withValues(alpha: 0.5), size: 48),
              const SizedBox(height: 16),
              Text(
                _error!,
                textAlign: TextAlign.center,
                style: const TextStyle(color: _P.textSecondary, fontSize: 14),
              ),
              const SizedBox(height: 24),
              GestureDetector(
                onTap: _loadData,
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
                  decoration: BoxDecoration(
                    color: _P.accent.withValues(alpha: 0.15),
                    borderRadius: BorderRadius.circular(10),
                    border: Border.all(
                      color: _P.accent.withValues(alpha: 0.3),
                    ),
                  ),
                  child: const Text(
                    'Retry',
                    style: TextStyle(
                      color: _P.accent,
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      );
    }

    final data = _data!;

    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(20, 4, 20, 40),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _buildSummaryCard(data),
          const SizedBox(height: 12),
          _buildCostCard(data),
          const SizedBox(height: 16),
          _buildChartCard(data),
          const SizedBox(height: 16),
          _buildRoomBreakdownHeader(),
          const SizedBox(height: 8),
          ...data.rooms.map(_buildRoomCard),
          const SizedBox(height: 16),
          _buildDisclaimer(),
        ],
      ),
    );
  }

  Widget _buildSummaryCard(_PowerUsageData data) {
    final peakWatts = data.hourlyWatts.isEmpty
        ? 0.0
        : data.hourlyWatts.reduce((a, b) => a > b ? a : b);

    return Container(
      padding: const EdgeInsets.all(20),
      decoration: BoxDecoration(
        color: _P.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _P.border),
      ),
      child: Row(
        children: [
          Expanded(
            child: _StatColumn(
              value: data.totalDailyKwh.toStringAsFixed(2),
              unit: 'kWh/day',
              icon: Icons.bolt_rounded,
              iconColor: _P.green,
            ),
          ),
          Container(width: 1, height: 44, color: _P.border),
          Expanded(
            child: _StatColumn(
              value: peakWatts.round().toString(),
              unit: 'W peak',
              icon: Icons.show_chart_rounded,
              iconColor: _P.warm,
            ),
          ),
          Container(width: 1, height: 44, color: _P.border),
          Expanded(
            child: _StatColumn(
              value: data.totalLights.toString(),
              unit: data.totalLights == 1 ? 'light' : 'lights',
              icon: Icons.lightbulb_outline_rounded,
              iconColor: _P.accent,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildCostCard(_PowerUsageData data) {
    final dailyCost = _rate != null ? data.totalDailyKwh * _rate! : null;
    final monthlyCost = dailyCost != null ? dailyCost * 30 : null;
    final yearlyCost = dailyCost != null ? dailyCost * 365 : null;

    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: _P.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _P.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Rate input row
          Row(
            children: [
              Icon(
                Icons.attach_money_rounded,
                color: _P.green.withValues(alpha: 0.7),
                size: 18,
              ),
              const SizedBox(width: 8),
              Text(
                'Electricity Rate',
                style: TextStyle(
                  color: _P.textSecondary,
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const Spacer(),
              // Input field
              SizedBox(
                width: 100,
                height: 34,
                child: TextField(
                  controller: _rateController,
                  onChanged: _onRateChanged,
                  keyboardType:
                      const TextInputType.numberWithOptions(decimal: true),
                  inputFormatters: [
                    FilteringTextInputFormatter.allow(RegExp(r'[\d.]')),
                  ],
                  textAlign: TextAlign.right,
                  style: const TextStyle(
                    color: _P.textPrimary,
                    fontSize: 14,
                    fontFeatures: [FontFeature.tabularFigures()],
                  ),
                  decoration: InputDecoration(
                    hintText: '0.12',
                    hintStyle: TextStyle(
                      color: _P.textSecondary.withValues(alpha: 0.4),
                    ),
                    suffixText: '/kWh',
                    suffixStyle: TextStyle(
                      color: _P.textSecondary,
                      fontSize: 11,
                    ),
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 6,
                    ),
                    isDense: true,
                    filled: true,
                    fillColor: _P.bg,
                    border: OutlineInputBorder(
                      borderRadius: BorderRadius.circular(8),
                      borderSide: BorderSide(color: _P.border),
                    ),
                    enabledBorder: OutlineInputBorder(
                      borderRadius: BorderRadius.circular(8),
                      borderSide: BorderSide(color: _P.border),
                    ),
                    focusedBorder: OutlineInputBorder(
                      borderRadius: BorderRadius.circular(8),
                      borderSide: BorderSide(
                        color: _P.accent.withValues(alpha: 0.5),
                      ),
                    ),
                  ),
                ),
              ),
            ],
          ),
          // Cost breakdown (only if rate is set)
          if (dailyCost != null) ...[
            const SizedBox(height: 14),
            Container(
              height: 1,
              color: _P.border,
            ),
            const SizedBox(height: 14),
            Row(
              children: [
                _CostChip(
                    label: 'Daily', value: '\$${dailyCost.toStringAsFixed(2)}'),
                const SizedBox(width: 10),
                _CostChip(
                    label: 'Monthly',
                    value: '\$${monthlyCost!.toStringAsFixed(2)}'),
                const SizedBox(width: 10),
                _CostChip(
                    label: 'Yearly',
                    value: '\$${yearlyCost!.toStringAsFixed(0)}'),
              ],
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildChartCard(_PowerUsageData data) {
    return Container(
      padding: const EdgeInsets.fromLTRB(8, 16, 16, 8),
      decoration: BoxDecoration(
        color: _P.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _P.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(left: 12, bottom: 12),
            child: Text(
              'Estimated Load (24h)',
              style: TextStyle(
                color: _P.textSecondary,
                fontSize: 12,
                fontWeight: FontWeight.w500,
                letterSpacing: 0.5,
              ),
            ),
          ),
          SizedBox(
            height: 200,
            child: _PowerChart(data: data),
          ),
        ],
      ),
    );
  }

  Widget _buildRoomBreakdownHeader() {
    return Padding(
      padding: const EdgeInsets.only(left: 4, top: 8),
      child: Text(
        'BY ROOM',
        style: TextStyle(
          color: _P.textSecondary.withValues(alpha: 0.7),
          fontSize: 11,
          fontWeight: FontWeight.w600,
          letterSpacing: 1.2,
        ),
      ),
    );
  }

  Widget _buildRoomCard(_RoomPowerData room) {
    final fraction = _data!.totalDailyKwh > 0
        ? room.dailyKwh / _data!.totalDailyKwh
        : 0.0;

    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: _P.card,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _P.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  room.roomName,
                  style: const TextStyle(
                    color: _P.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w500,
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              Text(
                '${room.dailyKwh.toStringAsFixed(2)} kWh',
                style: const TextStyle(
                  color: _P.green,
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                  fontFeatures: [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          Row(
            children: [
              Text(
                '${room.lights.length} ${room.lights.length == 1 ? 'light' : 'lights'}  ·  ${room.totalWatts.round()}W',
                style: TextStyle(color: _P.textSecondary, fontSize: 12),
              ),
              const Spacer(),
              if (_rate != null)
                Text(
                  '\$${(room.dailyKwh * _rate! * 30).toStringAsFixed(2)}/mo',
                  style: TextStyle(
                    color: _P.textSecondary.withValues(alpha: 0.7),
                    fontSize: 12,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                )
              else
                Text(
                  '${(fraction * 100).round()}%',
                  style: TextStyle(
                    color: _P.textSecondary.withValues(alpha: 0.7),
                    fontSize: 12,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
            ],
          ),
          const SizedBox(height: 8),
          ClipRRect(
            borderRadius: BorderRadius.circular(3),
            child: Container(
              height: 4,
              color: _P.border,
              alignment: Alignment.centerLeft,
              child: FractionallySizedBox(
                widthFactor: fraction.clamp(0.02, 1.0),
                child: Container(
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(3),
                    gradient: const LinearGradient(
                      colors: [_P.warm, _P.green],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildDisclaimer() {
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: _P.warm.withValues(alpha: 0.04),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: _P.warm.withValues(alpha: 0.08)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            Icons.info_outline_rounded,
            color: _P.warm.withValues(alpha: 0.5),
            size: 16,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              'Estimates assume all lights run the full Rhythm brightness curve. '
              'Actual usage depends on which lights are on and for how long. '
              'Wattage estimated at 9W per light. Actual usage varies by bulb type and model.',
              style: TextStyle(
                color: _P.textSecondary.withValues(alpha: 0.8),
                fontSize: 11,
                height: 1.5,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// =============================================================================
// Small Widgets
// =============================================================================

class _StatColumn extends StatelessWidget {
  final String value;
  final String unit;
  final IconData icon;
  final Color iconColor;

  const _StatColumn({
    required this.value,
    required this.unit,
    required this.icon,
    required this.iconColor,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Icon(icon, color: iconColor.withValues(alpha: 0.7), size: 18),
        const SizedBox(height: 6),
        Text(
          value,
          style: const TextStyle(
            color: _P.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w700,
            fontFeatures: [FontFeature.tabularFigures()],
          ),
        ),
        const SizedBox(height: 2),
        Text(
          unit,
          style: TextStyle(color: _P.textSecondary, fontSize: 11),
        ),
      ],
    );
  }
}

class _CostChip extends StatelessWidget {
  final String label;
  final String value;

  const _CostChip({required this.label, required this.value});

  @override
  Widget build(BuildContext context) {
    return Expanded(
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 8),
        decoration: BoxDecoration(
          color: _P.green.withValues(alpha: 0.06),
          borderRadius: BorderRadius.circular(10),
          border: Border.all(color: _P.green.withValues(alpha: 0.12)),
        ),
        child: Column(
          children: [
            Text(
              value,
              style: const TextStyle(
                color: _P.green,
                fontSize: 15,
                fontWeight: FontWeight.w700,
                fontFeatures: [FontFeature.tabularFigures()],
              ),
            ),
            const SizedBox(height: 2),
            Text(
              label,
              style: TextStyle(
                color: _P.textSecondary.withValues(alpha: 0.7),
                fontSize: 10,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// =============================================================================
// Power Chart
// =============================================================================

class _PowerChart extends StatelessWidget {
  final _PowerUsageData data;

  const _PowerChart({required this.data});

  @override
  Widget build(BuildContext context) {
    if (data.hourlyWatts.isEmpty) {
      return const Center(
        child: Text('No data', style: TextStyle(color: _P.textSecondary)),
      );
    }

    final maxW = data.hourlyWatts.reduce((a, b) => a > b ? a : b);
    final yMax = _niceMax(maxW);

    final spots = <FlSpot>[];
    for (int i = 0; i < data.hourlyWatts.length; i++) {
      spots.add(FlSpot(data.hourlyHours[i], data.hourlyWatts[i]));
    }
    if (spots.isNotEmpty) {
      spots.add(FlSpot(24, data.hourlyWatts[0]));
    }

    final now = DateTime.now();
    final currentHour = now.hour + now.minute / 60.0;

    return LineChart(
      LineChartData(
        minX: 0,
        maxX: 24,
        minY: 0,
        maxY: yMax,
        clipData: const FlClipData.all(),
        gridData: FlGridData(
          show: true,
          drawVerticalLine: true,
          horizontalInterval: yMax / 4,
          verticalInterval: 6,
          getDrawingHorizontalLine: (value) => const FlLine(
            color: Color(0x15FFFFFF),
            strokeWidth: 1,
          ),
          getDrawingVerticalLine: (value) => const FlLine(
            color: Color(0x15FFFFFF),
            strokeWidth: 1,
          ),
        ),
        titlesData: FlTitlesData(
          leftTitles: AxisTitles(
            sideTitles: SideTitles(
              showTitles: true,
              reservedSize: 44,
              interval: yMax / 4,
              getTitlesWidget: (value, meta) {
                if (value == 0 || value == yMax) {
                  return const SizedBox.shrink();
                }
                return Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: Text(
                    '${value.round()}W',
                    style: TextStyle(
                      color: _P.textSecondary.withValues(alpha: 0.6),
                      fontSize: 10,
                    ),
                  ),
                );
              },
            ),
          ),
          bottomTitles: AxisTitles(
            sideTitles: SideTitles(
              showTitles: true,
              reservedSize: 28,
              interval: 6,
              getTitlesWidget: (value, meta) {
                final hour = value.toInt();
                if (hour % 6 != 0 || hour > 24) {
                  return const SizedBox.shrink();
                }
                String label;
                if (!MediaQuery.alwaysUse24HourFormatOf(context)) {
                  if (hour == 0 || hour == 24) {
                    label = '12a';
                  } else if (hour == 12) {
                    label = '12p';
                  } else if (hour < 12) {
                    label = '${hour}a';
                  } else {
                    label = '${hour - 12}p';
                  }
                } else {
                  label = '${hour.toString().padLeft(2, '0')}:00';
                }
                return Padding(
                  padding: const EdgeInsets.only(top: 6),
                  child: Text(
                    label,
                    style: TextStyle(
                      color: _P.textSecondary.withValues(alpha: 0.6),
                      fontSize: 10,
                    ),
                  ),
                );
              },
            ),
          ),
          rightTitles: const AxisTitles(
            sideTitles: SideTitles(showTitles: false),
          ),
          topTitles: const AxisTitles(
            sideTitles: SideTitles(showTitles: false),
          ),
        ),
        borderData: FlBorderData(show: false),
        lineBarsData: [
          LineChartBarData(
            spots: spots,
            isCurved: true,
            curveSmoothness: 0.3,
            gradient: const LinearGradient(
              colors: [_P.warm, Color(0xFFE8B730), _P.green],
              begin: Alignment.centerLeft,
              end: Alignment.centerRight,
            ),
            barWidth: 2.5,
            dotData: const FlDotData(show: false),
            belowBarData: BarAreaData(
              show: true,
              gradient: LinearGradient(
                colors: [
                  _P.warm.withValues(alpha: 0.2),
                  const Color(0xFFE8B730).withValues(alpha: 0.12),
                  _P.green.withValues(alpha: 0.08),
                ],
                begin: Alignment.centerLeft,
                end: Alignment.centerRight,
              ),
            ),
          ),
        ],
        extraLinesData: ExtraLinesData(
          verticalLines: [
            VerticalLine(
              x: currentHour,
              color: _P.accent.withValues(alpha: 0.4),
              strokeWidth: 1,
              dashArray: [4, 4],
            ),
          ],
        ),
        lineTouchData: LineTouchData(
          enabled: true,
          touchTooltipData: LineTouchTooltipData(
            getTooltipColor: (_) => _P.card.withValues(alpha: 0.95),
            tooltipBorder: BorderSide(color: _P.border),
            getTooltipItems: (touchedSpots) {
              return touchedSpots.map((spot) {
                final h = spot.x.floor();
                final m = ((spot.x - h) * 60).round();
                final time =
                    '${h.toString().padLeft(2, '0')}:${m.toString().padLeft(2, '0')}';
                return LineTooltipItem(
                  '${spot.y.round()}W\n$time',
                  const TextStyle(
                    color: _P.textPrimary,
                    fontSize: 12,
                    fontWeight: FontWeight.w500,
                  ),
                );
              }).toList();
            },
          ),
          handleBuiltInTouches: true,
        ),
      ),
    );
  }

  double _niceMax(double value) {
    if (value <= 0) return 100;
    if (value <= 100) return ((value / 25).ceil() * 25).toDouble();
    if (value <= 500) return ((value / 50).ceil() * 50).toDouble();
    return ((value / 100).ceil() * 100).toDouble();
  }
}

// =============================================================================
// Palette
// =============================================================================

class _P {
  static const bg = Color(0xFF0B0E13);
  static const card = Color(0xFF13171E);
  static const border = Color(0xFF232A35);
  static const textPrimary = Color(0xFFE8EDF4);
  static const textSecondary = Color(0xFF8A919C);
  static const accent = Color(0xFF58A6FF);
  static const warm = Color(0xFFF9A825);
  static const green = Color(0xFF4ADE80);
}
