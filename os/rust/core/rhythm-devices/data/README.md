# Device data evidence

The `command_throttle_ms: 250` quirks for Matter models H6004 and H7056 are assumed compatibility values inherited from field behavior. They are not measured Bulb Audition evidence. Replace each value only after the `command_spacing` scenario measures that exact model and the result is reviewed.

H7056 uses `needs_explicit_on` so its off-to-on color restoration happens before
Rhythm sends the requested color. Staging hue/saturation while off can update
its reported attributes, then `MoveToLevelWithOnOff` restores the prior color.
The existing explicit-On strategy sends On, color, then the requested level.
This is specific to the H7056 Matter model; it preserves its existing HS white
route and temperature range. It does not provide color calibration or guarantee
that the previous color is never briefly visible during activation.
