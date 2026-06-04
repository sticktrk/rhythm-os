// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'hub.dart';

// **************************************************************************
// TypeAdapterGenerator
// **************************************************************************

class HubEndpointAdapter extends TypeAdapter<HubEndpoint> {
  @override
  final int typeId = 21;

  @override
  HubEndpoint read(BinaryReader reader) {
    final numOfFields = reader.readByte();
    final fields = <int, dynamic>{
      for (int i = 0; i < numOfFields; i++) reader.readByte(): reader.read(),
    };
    return HubEndpoint(
      host: fields[0] as String,
      port: fields[1] as int,
      useSsl: fields[2] as bool,
    );
  }

  @override
  void write(BinaryWriter writer, HubEndpoint obj) {
    writer
      ..writeByte(3)
      ..writeByte(0)
      ..write(obj.host)
      ..writeByte(1)
      ..write(obj.port)
      ..writeByte(2)
      ..write(obj.useSsl);
  }

  @override
  int get hashCode => typeId.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is HubEndpointAdapter &&
          runtimeType == other.runtimeType &&
          typeId == other.typeId;
}

class HubAdapter extends TypeAdapter<Hub> {
  @override
  final int typeId = 22;

  @override
  Hub read(BinaryReader reader) {
    final numOfFields = reader.readByte();
    final fields = <int, dynamic>{
      for (int i = 0; i < numOfFields; i++) reader.readByte(): reader.read(),
    };
    return Hub(
      id: fields[0] as String,
      homeId: fields[1] as String,
      type: fields[2] as HubType,
      name: fields[3] as String,
      endpoint: fields[4] as HubEndpoint,
      enabled: fields[5] as bool,
      requiresCredentials: fields[6] as bool,
      token: fields[7] as String?,
      lastConnected: fields[8] as DateTime?,
      createdAt: fields[9] as DateTime,
      updatedAt: fields[10] as DateTime,
      pendingSync: fields[11] as bool,
      remoteEndpoint: fields[12] as HubEndpoint?,
    );
  }

  @override
  void write(BinaryWriter writer, Hub obj) {
    writer
      ..writeByte(13)
      ..writeByte(0)
      ..write(obj.id)
      ..writeByte(1)
      ..write(obj.homeId)
      ..writeByte(2)
      ..write(obj.type)
      ..writeByte(3)
      ..write(obj.name)
      ..writeByte(4)
      ..write(obj.endpoint)
      ..writeByte(5)
      ..write(obj.enabled)
      ..writeByte(6)
      ..write(obj.requiresCredentials)
      ..writeByte(7)
      ..write(obj.token)
      ..writeByte(8)
      ..write(obj.lastConnected)
      ..writeByte(9)
      ..write(obj.createdAt)
      ..writeByte(10)
      ..write(obj.updatedAt)
      ..writeByte(11)
      ..write(obj.pendingSync)
      ..writeByte(12)
      ..write(obj.remoteEndpoint);
  }

  @override
  int get hashCode => typeId.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is HubAdapter &&
          runtimeType == other.runtimeType &&
          typeId == other.typeId;
}

class HubTypeAdapter extends TypeAdapter<HubType> {
  @override
  final int typeId = 20;

  @override
  HubType read(BinaryReader reader) {
    switch (reader.readByte()) {
      case 0:
        return HubType.homeAssistant;
      case 1:
        return HubType.hue;
      case 2:
        return HubType.server;
      default:
        return HubType.homeAssistant;
    }
  }

  @override
  void write(BinaryWriter writer, HubType obj) {
    switch (obj) {
      case HubType.homeAssistant:
        writer.writeByte(0);
        break;
      case HubType.hue:
        writer.writeByte(1);
        break;
      case HubType.server:
        writer.writeByte(2);
        break;
    }
  }

  @override
  int get hashCode => typeId.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is HubTypeAdapter &&
          runtimeType == other.runtimeType &&
          typeId == other.typeId;
}
