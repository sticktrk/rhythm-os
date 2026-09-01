import 'package:flutter/material.dart';

/// Named-schedule solar offsets are intentionally a small adjustment around
/// the selected anchor. The SDK keeps a wider range for wire compatibility.
const int maxLightScheduleOffsetSliderMinutes = 60;

class LightScheduleOffsetSlider extends StatelessWidget {
  const LightScheduleOffsetSlider({
    super.key,
    required this.sliderKey,
    required this.offsetMinutes,
    required this.onChanged,
  });

  final Key sliderKey;
  final int offsetMinutes;
  final ValueChanged<int>? onChanged;

  @override
  Widget build(BuildContext context) {
    final displayedOffset = offsetMinutes.clamp(
      -maxLightScheduleOffsetSliderMinutes,
      maxLightScheduleOffsetSliderMinutes,
    );
    return Slider(
      key: sliderKey,
      min: -maxLightScheduleOffsetSliderMinutes.toDouble(),
      max: maxLightScheduleOffsetSliderMinutes.toDouble(),
      divisions: maxLightScheduleOffsetSliderMinutes * 2,
      value: displayedOffset.toDouble(),
      label: '$displayedOffset min',
      onChanged:
          onChanged == null ? null : (value) => onChanged!(value.round()),
    );
  }
}
