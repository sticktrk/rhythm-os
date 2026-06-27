import '../json_parsing.dart';
import 'rhythm_input_binding.dart' show RhythmButtonAction;

class RhythmInputEventRoute {
  final String value;

  const RhythmInputEventRoute(this.value);

  static const inputBinding = RhythmInputEventRoute('input_binding');
  static const nodeControl = RhythmInputEventRoute('node_control');
  static const unroutable = RhythmInputEventRoute('unroutable');
  static const unresolved = RhythmInputEventRoute('unresolved');

  static RhythmInputEventRoute fromString(String? value) {
    return switch (value) {
      'input_binding' => inputBinding,
      'node_control' => nodeControl,
      'unroutable' => unroutable,
      'unresolved' => unresolved,
      _ => RhythmInputEventRoute(value ?? 'unresolved'),
    };
  }

  String get wireValue => value;

  @override
  bool operator ==(Object other) =>
      other is RhythmInputEventRoute && other.value == value;

  @override
  int get hashCode => value.hashCode;

  @override
  String toString() => value;
}

sealed class RhythmInputEvent {
  final String kind;
  final int epochMs;
  final RhythmInputEventRoute route;
  final String? hubType;
  final String? address;
  final String? sourceNodeId;
  final String? targetNodeId;
  final String? sourceRoomId;

  const RhythmInputEvent({
    required this.kind,
    required this.epochMs,
    required this.route,
    this.hubType,
    this.address,
    this.sourceNodeId,
    this.targetNodeId,
    this.sourceRoomId,
  });

  bool get isButton => this is RhythmButtonInputEvent;
  bool get isMotion => this is RhythmMotionInputEvent;
  bool get isContact => this is RhythmContactInputEvent;

  factory RhythmInputEvent.fromJson(Map<String, dynamic> json) {
    return switch (json['kind'] as String? ?? '') {
      'button' => RhythmButtonInputEvent.fromJson(json),
      'motion' => RhythmMotionInputEvent.fromJson(json),
      'contact' => RhythmContactInputEvent.fromJson(json),
      _ => RhythmRawInputEvent.fromJson(json),
    };
  }
}

class RhythmButtonInputEvent extends RhythmInputEvent {
  final String? nativeDeviceId;
  final String? nativeButtonId;
  final RhythmButtonAction? buttonAction;

  const RhythmButtonInputEvent({
    required super.epochMs,
    required super.route,
    super.hubType,
    super.address,
    super.sourceNodeId,
    super.targetNodeId,
    super.sourceRoomId,
    this.nativeDeviceId,
    this.nativeButtonId,
    this.buttonAction,
  }) : super(kind: 'button');

  factory RhythmButtonInputEvent.fromJson(Map<String, dynamic> json) {
    return RhythmButtonInputEvent(
      epochMs:
          jsonInt(json['epoch_ms'], preferredKeys: const ['epoch_ms']) ?? 0,
      route: RhythmInputEventRoute.fromString(json['route'] as String?),
      hubType: json['hub_type'] as String?,
      address: json['address'] as String?,
      sourceNodeId: json['source_node_id'] as String?,
      targetNodeId: json['target_node_id'] as String?,
      sourceRoomId: json['source_room_id'] as String?,
      nativeDeviceId: json['native_device_id'] as String?,
      nativeButtonId: json['native_button_id'] as String?,
      buttonAction:
          RhythmButtonAction.fromString(json['button_action'] as String?),
    );
  }
}

class RhythmMotionInputEvent extends RhythmInputEvent {
  final String nativeSensorId;
  final bool detected;

  const RhythmMotionInputEvent({
    required super.epochMs,
    required super.route,
    required this.nativeSensorId,
    required this.detected,
    super.hubType,
    super.address,
    super.sourceNodeId,
    super.targetNodeId,
    super.sourceRoomId,
  }) : super(kind: 'motion');

  factory RhythmMotionInputEvent.fromJson(Map<String, dynamic> json) {
    return RhythmMotionInputEvent(
      epochMs:
          jsonInt(json['epoch_ms'], preferredKeys: const ['epoch_ms']) ?? 0,
      route: RhythmInputEventRoute.fromString(json['route'] as String?),
      hubType: json['hub_type'] as String?,
      address: json['address'] as String?,
      sourceNodeId: json['source_node_id'] as String?,
      targetNodeId: json['target_node_id'] as String?,
      sourceRoomId: json['source_room_id'] as String?,
      nativeSensorId: json['native_sensor_id'] as String? ?? '',
      detected: json['detected'] as bool? ?? false,
    );
  }
}

class RhythmContactInputEvent extends RhythmInputEvent {
  final String nativeSensorId;
  final bool open;

  const RhythmContactInputEvent({
    required super.epochMs,
    required super.route,
    required this.nativeSensorId,
    required this.open,
    super.hubType,
    super.address,
    super.sourceNodeId,
    super.targetNodeId,
    super.sourceRoomId,
  }) : super(kind: 'contact');

  factory RhythmContactInputEvent.fromJson(Map<String, dynamic> json) {
    return RhythmContactInputEvent(
      epochMs:
          jsonInt(json['epoch_ms'], preferredKeys: const ['epoch_ms']) ?? 0,
      route: RhythmInputEventRoute.fromString(json['route'] as String?),
      hubType: json['hub_type'] as String?,
      address: json['address'] as String?,
      sourceNodeId: json['source_node_id'] as String?,
      targetNodeId: json['target_node_id'] as String?,
      sourceRoomId: json['source_room_id'] as String?,
      nativeSensorId: json['native_sensor_id'] as String? ?? '',
      open: json['open'] as bool? ?? false,
    );
  }
}

class RhythmRawInputEvent extends RhythmInputEvent {
  final Map<String, dynamic> rawJson;

  RhythmRawInputEvent({
    required super.kind,
    required super.epochMs,
    required super.route,
    required this.rawJson,
    super.hubType,
    super.address,
    super.sourceNodeId,
    super.targetNodeId,
    super.sourceRoomId,
  });

  factory RhythmRawInputEvent.fromJson(Map<String, dynamic> json) {
    return RhythmRawInputEvent(
      kind: json['kind'] as String? ?? 'unknown',
      epochMs:
          jsonInt(json['epoch_ms'], preferredKeys: const ['epoch_ms']) ?? 0,
      route: RhythmInputEventRoute.fromString(json['route'] as String?),
      hubType: json['hub_type'] as String?,
      address: json['address'] as String?,
      sourceNodeId: json['source_node_id'] as String?,
      targetNodeId: json['target_node_id'] as String?,
      sourceRoomId: json['source_room_id'] as String?,
      rawJson: Map<String, dynamic>.from(json),
    );
  }
}
