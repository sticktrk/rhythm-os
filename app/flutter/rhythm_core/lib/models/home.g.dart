// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'home.dart';

// **************************************************************************
// TypeAdapterGenerator
// **************************************************************************

class HomeLocationAdapter extends TypeAdapter<HomeLocation> {
  @override
  final int typeId = 10;

  @override
  HomeLocation read(BinaryReader reader) {
    final numOfFields = reader.readByte();
    final fields = <int, dynamic>{
      for (int i = 0; i < numOfFields; i++) reader.readByte(): reader.read(),
    };
    return HomeLocation(
      latitude: fields[0] as double,
      longitude: fields[1] as double,
      cityName: fields[2] as String?,
    );
  }

  @override
  void write(BinaryWriter writer, HomeLocation obj) {
    writer
      ..writeByte(3)
      ..writeByte(0)
      ..write(obj.latitude)
      ..writeByte(1)
      ..write(obj.longitude)
      ..writeByte(2)
      ..write(obj.cityName);
  }

  @override
  int get hashCode => typeId.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is HomeLocationAdapter &&
          runtimeType == other.runtimeType &&
          typeId == other.typeId;
}

class SleepScheduleAdapter extends TypeAdapter<SleepSchedule> {
  @override
  final int typeId = 11;

  @override
  SleepSchedule read(BinaryReader reader) {
    final numOfFields = reader.readByte();
    final fields = <int, dynamic>{
      for (int i = 0; i < numOfFields; i++) reader.readByte(): reader.read(),
    };
    return SleepSchedule(
      bedtime: fields[0] as double,
      wakeTime: fields[1] as double,
      enabled: fields[2] as bool,
    );
  }

  @override
  void write(BinaryWriter writer, SleepSchedule obj) {
    writer
      ..writeByte(3)
      ..writeByte(0)
      ..write(obj.bedtime)
      ..writeByte(1)
      ..write(obj.wakeTime)
      ..writeByte(2)
      ..write(obj.enabled);
  }

  @override
  int get hashCode => typeId.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is SleepScheduleAdapter &&
          runtimeType == other.runtimeType &&
          typeId == other.typeId;
}

class HomeAdapter extends TypeAdapter<Home> {
  @override
  final int typeId = 12;

  @override
  Home read(BinaryReader reader) {
    final numOfFields = reader.readByte();
    final fields = <int, dynamic>{
      for (int i = 0; i < numOfFields; i++) reader.readByte(): reader.read(),
    };
    return Home(
      id: fields[0] as String,
      name: fields[1] as String,
      ownerId: fields[2] as String,
      memberIds: (fields[3] as List).cast<String>(),
      location: fields[4] as HomeLocation?,
      sleepSchedule: fields[5] as SleepSchedule,
      curveConfigJson: (fields[6] as Map?)?.cast<String, dynamic>(),
      timezone: fields[7] as String?,
      createdAt: fields[8] as DateTime,
      updatedAt: fields[9] as DateTime,
      pendingSync: fields[10] as bool,
    );
  }

  @override
  void write(BinaryWriter writer, Home obj) {
    writer
      ..writeByte(11)
      ..writeByte(0)
      ..write(obj.id)
      ..writeByte(1)
      ..write(obj.name)
      ..writeByte(2)
      ..write(obj.ownerId)
      ..writeByte(3)
      ..write(obj.memberIds)
      ..writeByte(4)
      ..write(obj.location)
      ..writeByte(5)
      ..write(obj.sleepSchedule)
      ..writeByte(6)
      ..write(obj.curveConfigJson)
      ..writeByte(7)
      ..write(obj.timezone)
      ..writeByte(8)
      ..write(obj.createdAt)
      ..writeByte(9)
      ..write(obj.updatedAt)
      ..writeByte(10)
      ..write(obj.pendingSync);
  }

  @override
  int get hashCode => typeId.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is HomeAdapter &&
          runtimeType == other.runtimeType &&
          typeId == other.typeId;
}
