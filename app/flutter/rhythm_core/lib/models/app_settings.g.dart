// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'app_settings.dart';

// **************************************************************************
// TypeAdapterGenerator
// **************************************************************************

class AppSettingsAdapter extends TypeAdapter<AppSettings> {
  @override
  final int typeId = 40;

  @override
  AppSettings read(BinaryReader reader) {
    final numOfFields = reader.readByte();
    final fields = <int, dynamic>{
      for (int i = 0; i < numOfFields; i++) reader.readByte(): reader.read(),
    };
    return AppSettings(
      use24HourFormat: fields[0] as bool? ?? false,
      onboardingComplete: fields[1] as bool? ?? false,
      notificationsEnabled: fields[2] as bool? ?? false,
      hueSseEnabled: fields[3] as bool? ?? true,
      hueDeviceRegistryJson: fields[4] as String?,
      curveConfigJson: fields[5] as String?,
      runnerStateJson: fields[6] as String?,
      rhythmWarningDismissed: fields[7] as bool? ?? false,
      hueGroupedLightMapJson: fields[8] as String?,
      electricityRate: fields[9] as double?,
    );
  }

  @override
  void write(BinaryWriter writer, AppSettings obj) {
    writer
      ..writeByte(10)
      ..writeByte(0)
      ..write(obj.use24HourFormat)
      ..writeByte(1)
      ..write(obj.onboardingComplete)
      ..writeByte(2)
      ..write(obj.notificationsEnabled)
      ..writeByte(3)
      ..write(obj.hueSseEnabled)
      ..writeByte(4)
      ..write(obj.hueDeviceRegistryJson)
      ..writeByte(5)
      ..write(obj.curveConfigJson)
      ..writeByte(6)
      ..write(obj.runnerStateJson)
      ..writeByte(7)
      ..write(obj.rhythmWarningDismissed)
      ..writeByte(8)
      ..write(obj.hueGroupedLightMapJson)
      ..writeByte(9)
      ..write(obj.electricityRate);
  }

  @override
  int get hashCode => typeId.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is AppSettingsAdapter &&
          runtimeType == other.runtimeType &&
          typeId == other.typeId;
}
