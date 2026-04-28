# Custom Light Profile Design Guide

This guide explains how to design and implement custom lighting light profiles for the Rhythm OS system. Whether you're building linear rhythm lighting, manual control modes, or schedule-based automation, this document covers everything you need to know.

## Table of Contents

1. [Overview](#1-overview)
2. [Quick Start](#2-quick-start)
3. [The LightProfileModule Trait](#3-the-lightcurvemodule-trait)
4. [Understanding CurveContext](#4-understanding-curvecontext)
5. [Key Output Types](#5-key-output-types)
6. [Step Operations](#6-step-operations)
7. [Configuration Patterns](#7-configuration-patterns)
8. [Integration with Registry](#8-integration-with-registry)
9. [Best Practices](#9-best-practices)
10. [Reference: LightProfile](#10-reference-rhythmcurvemodule)
11. [Complete Example: SimpleProfile](#11-complete-example-simplecurvemodule)

---

## 1. Overview

### What is a LightProfileModule?

A `LightProfileModule` is a trait that defines how lighting values (brightness and color temperature) are calculated based on the time of day and solar position. Each module encapsulates a specific lighting algorithm, allowing the system to switch between different lighting behaviors without changing the core infrastructure.

The trait provides a clean abstraction for:
- **Calculating lighting values** at any given time
- **Step-based dimming** for manual adjustments along the curve
- **Boundary detection** to know when the curve has reached its limits
- **Configuration access** for range limits and stepping parameters

### When to Create a Custom Module

Create a custom module when you need:

| Use Case | Description |
|----------|-------------|
| **linear Rhythm** | Lighting that follows human linear cycles with research-backed color temperatures |
| **Manual Control** | Fixed or user-defined lighting values independent of time |
| **Schedule-Based** | Lighting transitions based on specific times or events |
| **Sensor-Driven** | Lighting that responds to ambient light sensors or occupancy |
| **Astronomical** | Lighting synchronized to actual sunrise/sunset for a location |

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    LightProfileRegistry                       │
│  ┌─────────────────┬─────────────────┬─────────────────┐   │
│  │ RhythmModule    │ linearModule │ ManualModule    │   │
│  │ (default)       │                 │                 │   │
│  └────────┬────────┴────────┬────────┴────────┬────────┘   │
│           │                 │                 │             │
│           ▼                 ▼                 ▼             │
│      ┌─────────────────────────────────────────────┐       │
│      │           LightProfileModule Trait            │       │
│      │  • calculate(ctx) → LightingValues          │       │
│      │  • calculate_step(ctx, action) → StepResult │       │
│      │  • is_at_maximum/minimum(ctx) → bool        │       │
│      └─────────────────────────────────────────────┘       │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
                    ┌───────────────────┐
                    │  LightingValues   │
                    │  • brightness: u8 │
                    │  • kelvin: u16    │
                    │  • rgb: Rgb       │
                    │  • xy: XyColor    │
                    └───────────────────┘
```

---

## 2. Quick Start

### Minimal Implementation

Here's the simplest possible light profile that returns fixed lighting values:

```rust
use std::sync::Arc;
use rhythm_core::light_profile::{CurveContext, LightProfileModule};
use rhythm_core::adaptive::LightingValues;
use rhythm_core::steps::{StepAction, StepResult};

pub struct FixedLightModule {
    brightness: u8,
    color_temp: u16,
}

impl FixedLightModule {
    pub fn new(brightness: u8, color_temp: u16) -> Self {
        Self { brightness, color_temp }
    }
}

impl LightProfileModule for FixedLightModule {
    fn id(&self) -> &str { "fixed" }
    fn name(&self) -> &str { "Fixed Light" }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        LightingValues::new(
            self.color_temp,
            self.brightness,
            ctx.solar_time(),
            0.0,
        )
    }

    fn calculate_brightness(&self, _ctx: &CurveContext) -> u8 {
        self.brightness
    }

    fn calculate_color_temperature(&self, _ctx: &CurveContext) -> u16 {
        self.color_temp
    }

    fn calculate_step(&self, ctx: &CurveContext, _action: StepAction) -> StepResult {
        StepResult {
            values: self.calculate(ctx),
            time_offset_minutes: 0.0,
            at_boundary: true, // Fixed values are always at boundary
        }
    }

    fn is_at_maximum(&self, _ctx: &CurveContext) -> bool { true }
    fn is_at_minimum(&self, _ctx: &CurveContext) -> bool { true }

    fn min_brightness(&self) -> u8 { self.brightness }
    fn max_brightness(&self) -> u8 { self.brightness }
    fn min_color_temp(&self) -> u16 { self.color_temp }
    fn max_color_temp(&self) -> u16 { self.color_temp }
}
```

### Registering Your Module

```rust
use rhythm_core::light_profile::LightProfileRegistry;

fn main() {
    let mut registry = LightProfileRegistry::new();

    // Create and register the module
    let fixed_module = Arc::new(FixedLightModule::new(75, 4000));
    registry.register(fixed_module);

    // Set as active
    registry.set_active_profile("fixed");

    // Use it
    let ctx = CurveContext::default();
    let values = registry.active_profile().calculate(&ctx);
    println!("Brightness: {}%, Color: {}K", values.brightness, values.kelvin);
}
```

### Testing Your Module

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::light_profile::CurveContext;

    #[test]
    fn test_fixed_module_returns_configured_values() {
        let module = FixedLightModule::new(80, 3500);
        let ctx = CurveContext::default();

        let values = module.calculate(&ctx);

        assert_eq!(values.brightness, 80);
        assert_eq!(values.kelvin, 3500);
    }

    #[test]
    fn test_fixed_module_is_always_at_boundary() {
        let module = FixedLightModule::new(50, 4000);
        let ctx = CurveContext::default();

        assert!(module.is_at_maximum(&ctx));
        assert!(module.is_at_minimum(&ctx));
    }
}
```

---

## 3. The LightProfileModule Trait

The `LightProfileModule` trait is the core abstraction for all lighting curve implementations. Here's the complete trait definition with explanations for each method:

```rust
pub trait LightProfileModule: Send + Sync {
    // Identity methods
    fn id(&self) -> &str;
    fn name(&self) -> &str;

    // Core calculation methods
    fn calculate(&self, ctx: &CurveContext) -> LightingValues;
    fn calculate_brightness(&self, ctx: &CurveContext) -> u8;
    fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16;

    // Time offset support (has default implementation)
    fn calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues;

    // Step operations
    fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult;

    // Boundary detection
    fn is_at_maximum(&self, ctx: &CurveContext) -> bool;
    fn is_at_minimum(&self, ctx: &CurveContext) -> bool;

    // Configuration accessors
    fn min_brightness(&self) -> u8;
    fn max_brightness(&self) -> u8;
    fn min_color_temp(&self) -> u16;
    fn max_color_temp(&self) -> u16;
}
```

### Identity Methods

#### `id(&self) -> &str`

Returns a unique identifier for this module. This ID is used:
- As the key in the registry's module map
- For serialization and configuration references
- For logging and debugging

**Convention**: Use lowercase snake_case (e.g., `"rhythm"`, `"linear"`, `"manual_override"`).

```rust
fn id(&self) -> &str {
    "my_custom_curve"
}
```

#### `name(&self) -> &str`

Returns a human-readable display name for the module. This is shown in user interfaces and logs.

```rust
fn name(&self) -> &str {
    "My Custom Curve"
}
```

### Core Calculation Methods

#### `calculate(&self, ctx: &CurveContext) -> LightingValues`

The main entry point for lighting calculations. Returns complete lighting values based on the provided context.

**Responsibilities**:
- Calculate both brightness and color temperature
- Construct a complete `LightingValues` with all derived properties
- Use context's solar time for curve positioning

```rust
fn calculate(&self, ctx: &CurveContext) -> LightingValues {
    let brightness = self.calculate_brightness(ctx);
    let color_temp = self.calculate_color_temperature(ctx);

    LightingValues::new(
        color_temp,
        brightness,
        ctx.solar_time(),
        self.calculate_sun_position(ctx), // If applicable
    )
}
```

#### `calculate_brightness(&self, ctx: &CurveContext) -> u8`

Calculates the brightness value (1-100%) for the current context. Should be a pure function that produces consistent results for the same input.

```rust
fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
    // Example: Linear interpolation based on time
    let t = ctx.solar_time() / 24.0;
    let range = self.max_brightness() - self.min_brightness();
    self.min_brightness() + (range as f32 * t) as u8
}
```

#### `calculate_color_temperature(&self, ctx: &CurveContext) -> u16`

Calculates the color temperature in Kelvin for the current context. Lower values (2700K) produce warm light; higher values (6500K) produce cool/blue light.

```rust
fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16 {
    // Example: Warmer in evening, cooler during day
    if ctx.is_morning() {
        self.max_color_temp()
    } else {
        self.min_color_temp()
    }
}
```

### Time Offset Support

#### `calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues`

Calculates lighting values at a time offset from the current context. This is used for step dimming to preview what values would be at a different time position on the curve.

**Default Implementation**:

```rust
fn calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues {
    let offset_ctx = ctx.with_offset(offset_minutes);
    self.calculate(&offset_ctx)
}
```

The default implementation is usually sufficient. Only override if your module needs special handling for time offsets.

### Step Operations

#### `calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult`

Calculates the result of a step dimming operation. Step dimming allows users to manually adjust lighting along the curve in discrete increments.

**Parameters**:
- `ctx`: Current curve context
- `action`: Either `StepAction::Brighten` (step toward maximum) or `StepAction::Dim` (step toward minimum)

**Returns**: A `StepResult` containing:
- Target lighting values
- Time offset in minutes from current position
- Whether the step hit a curve boundary

See [Section 6: Step Operations](#6-step-operations) for detailed implementation guidance.

### Boundary Detection

#### `is_at_maximum(&self, ctx: &CurveContext) -> bool`

Returns `true` if the current context is at or near the maximum brightness plateau of the curve. Used to determine when step-up operations should stop.

```rust
fn is_at_maximum(&self, ctx: &CurveContext) -> bool {
    let brightness = self.calculate_brightness(ctx);
    brightness >= self.max_brightness() - 1
}
```

#### `is_at_minimum(&self, ctx: &CurveContext) -> bool`

Returns `true` if the current context is at or near the minimum brightness plateau of the curve.

```rust
fn is_at_minimum(&self, ctx: &CurveContext) -> bool {
    let brightness = self.calculate_brightness(ctx);
    brightness <= self.min_brightness() + 1
}
```

### Configuration Accessors

These methods expose the module's configured brightness and color temperature ranges:

| Method | Description | Typical Default |
|--------|-------------|-----------------|
| `min_brightness()` | Minimum brightness percentage | 20 |
| `max_brightness()` | Maximum brightness percentage | 100 |
| `min_color_temp()` | Warmest color temperature | 500K |
| `max_color_temp()` | Coolest color temperature | 6500K |

These values are used by the system to:
- Calculate step sizes for dimming
- Validate boundary conditions
- Display available ranges to users

### Thread Safety: `Send + Sync`

The trait requires `Send + Sync` because modules may be:
- Shared across threads via `Arc<dyn LightProfileModule>`
- Accessed concurrently by multiple light controllers
- Stored in thread-safe registries

**Implications for your implementation**:
- Use immutable configuration where possible
- If mutable state is required, use interior mutability (`RwLock`, `Mutex`)
- Avoid storing thread-local data

```rust
// Good: Immutable config
pub struct MyModule {
    config: MyConfig,  // Immutable after construction
}

// If mutation needed: Use interior mutability
pub struct StatefulModule {
    config: RwLock<MyConfig>,
}
```

---

## 4. Understanding CurveContext

`CurveContext` provides all the temporal and astronomical data your module needs to calculate lighting values.

### Structure

```rust
#[derive(Debug, Clone)]
pub struct CurveContext {
    pub current_hour: f32,           // 0.0-24.0 (e.g., 14.5 = 2:30 PM)
    pub solar: SolarTime,            // Solar reference data
    pub sun_times: Option<SunTimes>, // Optional sunrise/sunset times
}
```

### Fields Explained

#### `current_hour: f32`

The local time of day as a floating-point hour value:
- `0.0` = midnight
- `6.5` = 6:30 AM
- `12.0` = noon
- `18.75` = 6:45 PM
- `23.99` = just before midnight

#### `solar: SolarTime`

Contains solar reference data including the noon offset used to calculate solar time:

```rust
pub struct SolarTime {
    pub noon_offset: f32,  // Hours from clock noon to solar noon
}
```

For example, if solar noon is at 12:30 PM local time, `noon_offset` would be `0.5`.

#### `sun_times: Option<SunTimes>`

Optional actual sunrise and sunset times for the current location:

```rust
pub struct SunTimes {
    pub sunrise: f32,  // Hour of sunrise (e.g., 6.5)
    pub sunset: f32,   // Hour of sunset (e.g., 18.25)
}
```

This is `None` if sun times are unavailable (no location configured, polar regions, etc.).

### Key Methods

#### `solar_time(&self) -> f32`

Returns the solar-adjusted time for the current hour. This shifts clock time to align with the sun's position:

```rust
impl CurveContext {
    pub fn solar_time(&self) -> f32 {
        let adjusted = self.current_hour - self.solar.noon_offset;
        if adjusted < 0.0 {
            adjusted + 24.0
        } else if adjusted >= 24.0 {
            adjusted - 24.0
        } else {
            adjusted
        }
    }
}
```

**Why use solar time?** Curves based on solar time naturally adapt to seasonal daylight changes and geographic location without reconfiguration.

#### `is_morning(&self) -> bool`

Returns `true` if the solar time is before noon (ascending portion of daily cycle):

```rust
impl CurveContext {
    pub fn is_morning(&self) -> bool {
        self.solar_time() < 12.0
    }
}
```

Use this to switch between morning (brightening) and evening (dimming) curve behaviors.

#### `with_offset(&self, offset_minutes: f32) -> Self`

Creates a new context with the time shifted by the specified number of minutes. Handles wrapping around midnight:

```rust
let ctx = CurveContext::new(23.5, solar, sun_times); // 11:30 PM
let offset_ctx = ctx.with_offset(60.0);              // 12:30 AM next day
assert!((offset_ctx.current_hour - 0.5).abs() < 0.001);
```

This is essential for step dimming calculations where you need to find the time position corresponding to a target brightness.

### Usage Examples

```rust
fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
    let solar_t = ctx.solar_time();

    if ctx.is_morning() {
        // Morning: brightness increases
        self.morning_curve(solar_t)
    } else {
        // Evening: brightness decreases
        self.evening_curve(solar_t)
    }
}

fn calculate_with_sunrise(&self, ctx: &CurveContext) -> u8 {
    if let Some(sun_times) = &ctx.sun_times {
        // Use actual sunrise/sunset
        let sunrise = sun_times.sunrise;
        let sunset = sun_times.sunset;
        // ... curve calculation using actual times
    } else {
        // Fallback to fixed times
        self.calculate_with_defaults(ctx)
    }
}
```

---

## 5. Key Output Types

### LightingValues

The primary output type for lighting calculations:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct LightingValues {
    pub kelvin: u16,        // Color temperature (500-6500K typical)
    pub brightness: u8,     // Brightness percentage (1-100)
    pub rgb: Rgb,           // Derived RGB color
    pub xy: XyColor,        // Derived CIE xy coordinates
    pub solar_time: f32,    // Solar time when calculated
    pub sun_position: f32,  // Sun position (-1 to +1)
}
```

#### Constructor

```rust
impl LightingValues {
    pub fn new(kelvin: u16, brightness: u8, solar_time: f32, sun_position: f32) -> Self
}
```

The constructor automatically derives `rgb` and `xy` from the Kelvin temperature, so you only need to provide the core values.

#### Fields

| Field | Type | Description |
|-------|------|-------------|
| `kelvin` | `u16` | Color temperature in Kelvin. 2700K is warm/orange, 6500K is cool/blue |
| `brightness` | `u8` | Brightness as percentage 1-100 |
| `rgb` | `Rgb` | RGB representation derived from Kelvin |
| `xy` | `XyColor` | CIE 1931 xy chromaticity coordinates for Hue-compatible systems |
| `solar_time` | `f32` | The solar time at which these values were calculated |
| `sun_position` | `f32` | Normalized sun position: -1 (midnight), 0 (horizon), +1 (noon) |

### StepResult

Returned by `calculate_step()` to describe the outcome of a step operation:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub values: LightingValues,       // The target lighting values
    pub time_offset_minutes: f32,     // Minutes from current time
    pub at_boundary: bool,            // Whether at plateau limit
}
```

| Field | Description |
|-------|-------------|
| `values` | The lighting values at the step target position |
| `time_offset_minutes` | How far (in minutes) from current position. Positive = forward in time (brighter in morning), negative = backward |
| `at_boundary` | `true` if this step reached a curve boundary (plateau) and no further steps are possible in this direction |

### StepAction

Enum specifying the direction of a step operation:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepAction {
    Brighten,  // Step toward maximum brightness
    Dim,       // Step toward minimum brightness
}

impl StepAction {
    pub fn direction(&self) -> i8 {
        match self {
            StepAction::Brighten => 1,
            StepAction::Dim => -1,
        }
    }
}
```

### CurveBoundaries

Describes the time positions where the curve reaches its brightness limits:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct CurveBoundaries {
    pub min_brightness_morning: f32,  // Hour when morning min reached
    pub min_brightness_evening: f32,  // Hour when evening min reached
    pub max_brightness_morning: f32,  // Hour when morning max reached
    pub max_brightness_evening: f32,  // Hour when evening max reached
}
```

This is used internally for efficient boundary detection but can be useful if your module pre-calculates plateau positions.

---

## 6. Step Operations

Step dimming allows users to manually adjust lighting in discrete increments along the curve. This section explains the concepts and implementation patterns.

### How Step Dimming Works

Instead of setting an absolute brightness, step dimming moves along the lighting curve:

```
Brightness
100% │         ████████████████
     │       ██                ██
     │     ██                    ██
     │   ██        ← Step up       ██
     │  █                            █
     │ █              Current → ●     █
     │█                Step down →     █
  1% │█                                 █
     └────────────────────────────────────
        6AM       12PM       6PM      12AM
```

When stepping:
- **Brighten**: Moves to where the curve would be at a later time (toward noon)
- **Dim**: Moves to where the curve would be at an earlier/later time (away from noon)

### Implementing `calculate_step()`

Here's a pattern for implementing step calculation:

```rust
fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult {
    // 1. Check if already at boundary
    match action {
        StepAction::Brighten if self.is_at_maximum(ctx) => {
            return StepResult {
                values: self.calculate(ctx),
                time_offset_minutes: 0.0,
                at_boundary: true,
            };
        }
        StepAction::Dim if self.is_at_minimum(ctx) => {
            return StepResult {
                values: self.calculate(ctx),
                time_offset_minutes: 0.0,
                at_boundary: true,
            };
        }
        _ => {}
    }

    // 2. Calculate step size
    let step_size = self.brightness_step_size();
    let current_brightness = self.calculate_brightness(ctx);

    // 3. Calculate target brightness
    let target_brightness = match action {
        StepAction::Brighten => {
            (current_brightness as f32 + step_size)
                .min(self.max_brightness() as f32) as u8
        }
        StepAction::Dim => {
            (current_brightness as f32 - step_size)
                .max(self.min_brightness() as f32) as u8
        }
    };

    // 4. Find the time offset where curve reaches target brightness
    let offset_minutes = self.find_time_for_brightness(ctx, target_brightness, action);

    // 5. Calculate values at that offset
    let target_values = self.calculate_with_offset(ctx, offset_minutes);

    // 6. Determine if we hit a boundary
    let at_boundary = match action {
        StepAction::Brighten => target_brightness >= self.max_brightness(),
        StepAction::Dim => target_brightness <= self.min_brightness(),
    };

    StepResult {
        values: target_values,
        time_offset_minutes: offset_minutes,
        at_boundary,
    }
}
```

### Finding Time for Target Brightness

Since most curves are non-linear, you need to find the time corresponding to a target brightness. A sampling approach works well:

```rust
fn find_time_for_brightness(
    &self,
    ctx: &CurveContext,
    target: u8,
    action: StepAction,
) -> f32 {
    let search_direction = match (action, ctx.is_morning()) {
        (StepAction::Brighten, true) => 1.0,   // Forward toward noon
        (StepAction::Brighten, false) => -1.0, // Backward toward noon
        (StepAction::Dim, true) => -1.0,       // Backward toward morning
        (StepAction::Dim, false) => 1.0,       // Forward toward evening
    };

    // Sample at 1-minute intervals up to 6 hours
    let max_offset = 360.0; // 6 hours in minutes
    let mut best_offset = 0.0;
    let mut best_diff = f32::MAX;

    for i in 0..=360 {
        let offset = i as f32 * search_direction;
        let test_ctx = ctx.with_offset(offset);
        let brightness = self.calculate_brightness(&test_ctx);

        let diff = (brightness as f32 - target as f32).abs();
        if diff < best_diff {
            best_diff = diff;
            best_offset = offset;
        }

        // Stop if we found exact match or passed target
        if brightness == target {
            return offset;
        }
    }

    best_offset
}
```

### The Interpolation Pattern

For smoother step calculations, you can interpolate between sample points:

```rust
fn find_time_for_brightness_interpolated(
    &self,
    ctx: &CurveContext,
    target: u8,
) -> f32 {
    // Binary search with interpolation
    let mut low = -360.0;
    let mut high = 360.0;

    for _ in 0..20 { // 20 iterations for precision
        let mid = (low + high) / 2.0;
        let mid_brightness = self.calculate_brightness(&ctx.with_offset(mid));

        if mid_brightness == target {
            return mid;
        } else if mid_brightness < target {
            low = mid;
        } else {
            high = mid;
        }
    }

    (low + high) / 2.0
}
```

---

## 7. Configuration Patterns

### Using CommonCurveConfig

`CommonCurveConfig` provides shared settings that apply to most light profiles:

```rust
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CommonCurveConfig {
    pub min_color_temp: u16,   // Default: 500K
    pub max_color_temp: u16,   // Default: 6500K
    pub min_brightness: u8,    // Default: 20%
    pub max_brightness: u8,    // Default: 100%
    pub max_dim_steps: u8,     // Default: 6
}

impl CommonCurveConfig {
    pub fn brightness_step_size(&self) -> f32 {
        (self.max_brightness - self.min_brightness) as f32 / self.max_dim_steps as f32
    }
}
```

Use it in your module:

```rust
pub struct MyModule {
    common: CommonCurveConfig,
    // Module-specific fields...
}

impl LightProfileModule for MyModule {
    fn min_brightness(&self) -> u8 { self.common.min_brightness }
    fn max_brightness(&self) -> u8 { self.common.max_brightness }
    fn min_color_temp(&self) -> u16 { self.common.min_color_temp }
    fn max_color_temp(&self) -> u16 { self.common.max_color_temp }

    // Use step size in calculate_step
    fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult {
        let step_size = self.common.brightness_step_size();
        // ...
    }
}
```

### Creating Module-Specific Config

Define configuration specific to your module's algorithm:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct linearConfig {
    /// Common settings (brightness/color ranges)
    #[serde(flatten)]
    pub common: CommonCurveConfig,

    /// Target wake time (0-24 hours)
    pub wake_time: f32,

    /// Target sleep time (0-24 hours)
    pub sleep_time: f32,

    /// Minutes before sleep to start dimming
    pub wind_down_minutes: u32,

    /// Enable blue light reduction in evening
    pub reduce_blue_light: bool,
}

impl Default for linearConfig {
    fn default() -> Self {
        Self {
            common: CommonCurveConfig::default(),
            wake_time: 7.0,
            sleep_time: 22.0,
            wind_down_minutes: 60,
            reduce_blue_light: true,
        }
    }
}
```

### Registering Profile Configs

The registry no longer uses a `LightProfileModuleConfig` enum. It stores full
`LightProfileConfig` values keyed by profile ID, and materializes runtime
profiles directly from those stored configs:

```rust
use rhythm_core::{LightProfileConfig, LightProfileRegistry};

let mut registry = LightProfileRegistry::new();

let custom = LightProfileConfig {
    id: "linear".into(),
    name: "Linear".into(),
    curve: /* your LightCurveShape */,
    min_brightness: 5,
    max_brightness: 90,
    min_color_temp: 2200,
    max_color_temp: 5500,
    max_dim_steps: 8,
    fade_ms: None,
    motion_timeout_secs: None,
    rhythm_interval_secs: None,
};

registry.register_config(custom);
```

---

## 8. Integration with Registry

### Registering Your Module

The `LightProfileRegistry` manages all available light profiles:

```rust
use std::sync::Arc;
use rhythm_core::light_profile::{LightProfileRegistry, LightProfileModule};

fn setup_registry() -> LightProfileRegistry {
    let mut registry = LightProfileRegistry::new();

    // Register custom modules
    let linear = Arc::new(linearModule::new(linearConfig::default()));
    registry.register(linear);

    let manual = Arc::new(ManualModule::new(50, 3000));
    registry.register(manual);

    registry
}
```

### Setting the Active Module

```rust
// Set by ID
if registry.set_active_profile("linear") {
    println!("Switched to linear module");
} else {
    println!("Module 'linear' not found");
}

// Get the active profile
let active = registry.active_profile();
println!("Using: {} ({})", active.name(), active.id());

// Reset to default
registry.reset_to_default();
```

### Listing Available Modules

```rust
// Get all registered modules
for (id, name) in registry.available_profiles() {
    println!("  {}: {}", id, name);
}
// Output:
//   rhythm: Rhythm Profile
//   linear: linear Rhythm
//   manual: Manual Control
```

### Registry Constraints

The registry enforces these rules:

1. **Cannot remove the active profile**: Attempting to unregister the currently active profile returns `false`

2. **Cannot remove the default module**: The default module (rhythm) is protected from removal

3. **Active profile always exists**: If you call `active_profile()` and it doesn't exist, the code panics (invariant violation)

```rust
// These will fail (return false)
registry.set_active_profile("rhythm");
registry.unregister("rhythm");  // false: is default

registry.set_active_profile("linear");
registry.unregister("linear");  // false: is active

// This works
registry.set_active_profile("rhythm");
registry.unregister("linear");  // true: not active, not default
```

---

## 9. Best Practices

### Immutability and Pure Functions

Curve modules should be stateless and produce consistent outputs:

```rust
// Good: Pure function, same input = same output
fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
    let t = ctx.solar_time();
    self.curve_function(t)
}

// Avoid: Mutable state affects output
fn calculate_brightness(&mut self, ctx: &CurveContext) -> u8 {
    self.last_brightness = self.curve_function(ctx.solar_time());
    self.last_brightness  // Same input might give different output!
}
```

If you need to track state (e.g., for transitions), keep it separate from the calculation logic.

### Fallback Values for Missing Sun Times

Always handle the case where `sun_times` is `None`:

```rust
fn get_sunrise(&self, ctx: &CurveContext) -> f32 {
    ctx.sun_times
        .as_ref()
        .map(|st| st.sunrise)
        .unwrap_or(6.0)  // Default to 6 AM
}

fn get_sunset(&self, ctx: &CurveContext) -> f32 {
    ctx.sun_times
        .as_ref()
        .map(|st| st.sunset)
        .unwrap_or(18.0)  // Default to 6 PM
}
```

### Logging with Tracing

Use the `tracing` crate for structured logging:

```rust
use tracing::{debug, trace, instrument};

impl LightProfileModule for MyModule {
    #[instrument(skip(self, ctx), fields(module = %self.id()))]
    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let brightness = self.calculate_brightness(ctx);
        let color_temp = self.calculate_color_temperature(ctx);

        debug!(
            brightness = brightness,
            color_temp = color_temp,
            solar_time = ctx.solar_time(),
            "Calculated lighting values"
        );

        LightingValues::new(color_temp, brightness, ctx.solar_time(), 0.0)
    }

    fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult {
        trace!(?action, "Processing step request");
        // ...
    }
}
```

### Testing Strategies

#### Unit Tests for Calculations

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_context(hour: f32) -> CurveContext {
        CurveContext::new(hour, SolarTime::default(), None)
    }

    #[test]
    fn test_brightness_at_noon_is_maximum() {
        let module = MyModule::default();
        let ctx = make_context(12.0);

        assert_eq!(module.calculate_brightness(&ctx), module.max_brightness());
    }

    #[test]
    fn test_brightness_at_midnight_is_minimum() {
        let module = MyModule::default();
        let ctx = make_context(0.0);

        assert_eq!(module.calculate_brightness(&ctx), module.min_brightness());
    }
}
```

#### Property-Based Tests

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn brightness_always_in_range(hour in 0.0f32..24.0) {
        let module = MyModule::default();
        let ctx = make_context(hour);
        let brightness = module.calculate_brightness(&ctx);

        prop_assert!(brightness >= module.min_brightness());
        prop_assert!(brightness <= module.max_brightness());
    }

    #[test]
    fn color_temp_always_in_range(hour in 0.0f32..24.0) {
        let module = MyModule::default();
        let ctx = make_context(hour);
        let temp = module.calculate_color_temperature(&ctx);

        prop_assert!(temp >= module.min_color_temp());
        prop_assert!(temp <= module.max_color_temp());
    }
}
```

#### Integration Tests

```rust
#[test]
fn test_module_works_with_registry() {
    let mut registry = LightProfileRegistry::new();
    let module = Arc::new(MyModule::default());

    registry.register(module);
    assert!(registry.set_active_profile("my_module"));

    let active = registry.active_profile();
    assert_eq!(active.id(), "my_module");

    let ctx = CurveContext::default();
    let values = active.calculate(&ctx);
    assert!(values.brightness >= 1 && values.brightness <= 100);
}
```

---

## 10. Reference: LightProfile

The `LightProfile` is the default implementation and serves as a reference for building custom modules. Understanding its approach helps when designing your own.

### Logistic Curve Approach

The module uses logistic (sigmoid) curves for smooth transitions:

```rust
// Simplified logistic function
fn logistic(t: f32, midpoint: f32, steepness: f32) -> f32 {
    1.0 / (1.0 + (-steepness * (t - midpoint)).exp())
}
```

The curve produces an S-shaped transition:

```
Output
1.0 │                    ████████████
    │                ████
    │             ███
    │           ██
    │         ██
    │        █
    │      ██
    │    ██
0.0 │████
    └─────────────────────────────────
              midpoint
```

### Morning/Evening Asymmetry

The module supports different curves for morning and evening:

```rust
fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
    if ctx.is_morning() {
        // Morning: Use up-curve parameters
        self.map_half(
            ctx.solar_time(),
            self.config.mid_bri_up.resolve(...),
            self.config.steep_bri_up,
        )
    } else {
        // Evening: Use down-curve parameters
        self.map_half(
            ctx.solar_time(),
            self.config.mid_bri_dn.resolve(...),
            self.config.steep_bri_dn,
        )
    }
}
```

This allows:
- **Faster morning ramp-up** (steepness 3.0) for alertness
- **Gentler evening wind-down** (steepness 2.0) for relaxation

### Mirror Flags Pattern

Mirror flags allow the color temperature curve to follow the brightness curve:

```rust
pub struct MirrorProfileConfig {
    // When true, CCT uses brightness curve parameters
    pub mirror_up: bool,  // Default: true
    pub mirror_dn: bool,  // Default: false

    // Independent CCT curve parameters (used when not mirroring)
    pub mid_cct_up: MidpointValue,
    pub steep_cct_up: f32,
    pub mid_cct_dn: MidpointValue,
    pub steep_cct_dn: f32,
}
```

Usage in color temperature calculation:

```rust
fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16 {
    let (midpoint, steepness) = if ctx.is_morning() {
        if self.config.mirror_up {
            // Mirror: use brightness curve
            (self.config.mid_bri_up, self.config.steep_bri_up)
        } else {
            // Independent curve
            (self.config.mid_cct_up, self.config.steep_cct_up)
        }
    } else {
        if self.config.mirror_dn {
            (self.config.mid_bri_dn, self.config.steep_bri_dn)
        } else {
            (self.config.mid_cct_dn, self.config.steep_cct_dn)
        }
    };

    self.calculate_cct_with_params(ctx, midpoint, steepness)
}
```

---

## 11. Complete Example: SimpleProfile

Here's a complete, working implementation of a simple linear interpolation module that you can use as a starting template.

### Config Struct

```rust
use serde::{Deserialize, Serialize};
use rhythm_core::light_profile::CommonCurveConfig;

/// Configuration for the simple linear light profile.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SimpleProfileConfig {
    /// Common brightness/color temperature settings
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub common: CommonCurveConfig,

    /// Hour when brightness starts increasing (0-24)
    pub dawn_hour: f32,

    /// Hour when brightness reaches maximum (0-24)
    pub peak_hour: f32,

    /// Hour when brightness starts decreasing (0-24)
    pub dusk_hour: f32,

    /// Hour when brightness reaches minimum (0-24)
    pub night_hour: f32,
}

impl Default for SimpleProfileConfig {
    fn default() -> Self {
        Self {
            common: CommonCurveConfig::default(),
            dawn_hour: 6.0,
            peak_hour: 9.0,
            dusk_hour: 18.0,
            night_hour: 21.0,
        }
    }
}
```

### Module Implementation

```rust
use std::sync::Arc;
use rhythm_core::adaptive::LightingValues;
use rhythm_core::light_profile::{CurveContext, LightProfileModule};
use rhythm_core::steps::{StepAction, StepResult};

/// A simple light profile using linear interpolation between fixed time points.
///
/// The curve follows this pattern:
/// - Night (midnight to dawn): minimum brightness, warm color
/// - Dawn to Peak: linear ramp up
/// - Peak to Dusk: maximum brightness, cool color
/// - Dusk to Night: linear ramp down
/// - Night (to midnight): minimum brightness, warm color
#[derive(Debug, Clone)]
pub struct SimpleProfile {
    config: SimpleProfileConfig,
}

impl SimpleProfile {
    pub const ID: &'static str = "simple";
    pub const NAME: &'static str = "Simple Linear Curve";

    pub fn new(config: SimpleProfileConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(SimpleProfileConfig::default())
    }

    /// Linear interpolation helper
    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t.clamp(0.0, 1.0)
    }

    /// Calculate brightness as a normalized value (0.0 to 1.0)
    fn normalized_brightness(&self, hour: f32) -> f32 {
        let dawn = self.config.dawn_hour;
        let peak = self.config.peak_hour;
        let dusk = self.config.dusk_hour;
        let night = self.config.night_hour;

        if hour < dawn {
            // Before dawn: minimum
            0.0
        } else if hour < peak {
            // Dawn to peak: ramp up
            (hour - dawn) / (peak - dawn)
        } else if hour < dusk {
            // Peak to dusk: maximum
            1.0
        } else if hour < night {
            // Dusk to night: ramp down
            1.0 - (hour - dusk) / (night - dusk)
        } else {
            // After night: minimum
            0.0
        }
    }

    /// Find the time offset (in minutes) to reach a target brightness
    fn find_offset_for_brightness(
        &self,
        ctx: &CurveContext,
        target_brightness: u8,
        action: StepAction,
    ) -> f32 {
        let current = ctx.current_hour;
        let target_normalized = (target_brightness - self.min_brightness()) as f32
            / (self.max_brightness() - self.min_brightness()) as f32;

        // Search in the appropriate direction
        let step = match action {
            StepAction::Brighten => 1.0 / 60.0,  // +1 minute in hours
            StepAction::Dim => -1.0 / 60.0,      // -1 minute in hours
        };

        let mut offset_hours = 0.0;
        let max_offset = 12.0; // Search up to 12 hours

        while offset_hours.abs() < max_offset {
            let test_hour = (current + offset_hours).rem_euclid(24.0);
            let brightness = self.normalized_brightness(test_hour);

            match action {
                StepAction::Brighten if brightness >= target_normalized => {
                    return offset_hours * 60.0; // Convert to minutes
                }
                StepAction::Dim if brightness <= target_normalized => {
                    return offset_hours * 60.0;
                }
                _ => {}
            }

            offset_hours += step;
        }

        offset_hours * 60.0
    }
}

impl LightProfileModule for SimpleProfile {
    fn id(&self) -> &str {
        Self::ID
    }

    fn name(&self) -> &str {
        Self::NAME
    }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let brightness = self.calculate_brightness(ctx);
        let color_temp = self.calculate_color_temperature(ctx);

        // Calculate sun position (-1 at midnight, 0 at dawn/dusk, +1 at noon)
        let normalized = self.normalized_brightness(ctx.solar_time());
        let sun_position = (normalized * 2.0) - 1.0;

        LightingValues::new(
            color_temp,
            brightness,
            ctx.solar_time(),
            sun_position,
        )
    }

    fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
        let normalized = self.normalized_brightness(ctx.solar_time());
        let range = (self.max_brightness() - self.min_brightness()) as f32;
        self.min_brightness() + (normalized * range) as u8
    }

    fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16 {
        // Color temperature follows brightness: brighter = cooler
        let normalized = self.normalized_brightness(ctx.solar_time());
        let range = (self.max_color_temp() - self.min_color_temp()) as f32;
        self.min_color_temp() + (normalized * range) as u16
    }

    fn calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues {
        let offset_ctx = ctx.with_offset(offset_minutes);
        self.calculate(&offset_ctx)
    }

    fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult {
        // Check boundaries first
        match action {
            StepAction::Brighten if self.is_at_maximum(ctx) => {
                return StepResult {
                    values: self.calculate(ctx),
                    time_offset_minutes: 0.0,
                    at_boundary: true,
                };
            }
            StepAction::Dim if self.is_at_minimum(ctx) => {
                return StepResult {
                    values: self.calculate(ctx),
                    time_offset_minutes: 0.0,
                    at_boundary: true,
                };
            }
            _ => {}
        }

        // Calculate target brightness
        let step_size = self.config.common.brightness_step_size();
        let current = self.calculate_brightness(ctx) as f32;

        let target = match action {
            StepAction::Brighten => {
                (current + step_size).min(self.max_brightness() as f32) as u8
            }
            StepAction::Dim => {
                (current - step_size).max(self.min_brightness() as f32) as u8
            }
        };

        // Find offset for target
        let offset = self.find_offset_for_brightness(ctx, target, action);
        let values = self.calculate_with_offset(ctx, offset);

        // Check if we hit boundary
        let at_boundary = match action {
            StepAction::Brighten => values.brightness >= self.max_brightness(),
            StepAction::Dim => values.brightness <= self.min_brightness(),
        };

        StepResult {
            values,
            time_offset_minutes: offset,
            at_boundary,
        }
    }

    fn is_at_maximum(&self, ctx: &CurveContext) -> bool {
        self.calculate_brightness(ctx) >= self.max_brightness() - 1
    }

    fn is_at_minimum(&self, ctx: &CurveContext) -> bool {
        self.calculate_brightness(ctx) <= self.min_brightness() + 1
    }

    fn min_brightness(&self) -> u8 {
        self.config.common.min_brightness
    }

    fn max_brightness(&self) -> u8 {
        self.config.common.max_brightness
    }

    fn min_color_temp(&self) -> u16 {
        self.config.common.min_color_temp
    }

    fn max_color_temp(&self) -> u16 {
        self.config.common.max_color_temp
    }
}
```

### Test Suite

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::light_profile::CurveContext;
    use rhythm_core::solar::SolarTime;

    fn make_context(hour: f32) -> CurveContext {
        CurveContext::new(hour, SolarTime::default(), None)
    }

    #[test]
    fn test_module_identity() {
        let module = SimpleProfile::with_defaults();
        assert_eq!(module.id(), "simple");
        assert_eq!(module.name(), "Simple Linear Curve");
    }

    #[test]
    fn test_minimum_brightness_before_dawn() {
        let module = SimpleProfile::with_defaults();
        let ctx = make_context(4.0); // 4 AM, before default dawn (6 AM)

        assert_eq!(module.calculate_brightness(&ctx), module.min_brightness());
    }

    #[test]
    fn test_maximum_brightness_at_peak() {
        let module = SimpleProfile::with_defaults();
        let ctx = make_context(12.0); // Noon, between peak (9 AM) and dusk (6 PM)

        assert_eq!(module.calculate_brightness(&ctx), module.max_brightness());
    }

    #[test]
    fn test_minimum_brightness_after_night() {
        let module = SimpleProfile::with_defaults();
        let ctx = make_context(23.0); // 11 PM, after default night (9 PM)

        assert_eq!(module.calculate_brightness(&ctx), module.min_brightness());
    }

    #[test]
    fn test_brightness_ramps_up_during_dawn() {
        let module = SimpleProfile::with_defaults();

        let early = make_context(6.5);
        let late = make_context(8.5);

        let early_brightness = module.calculate_brightness(&early);
        let late_brightness = module.calculate_brightness(&late);

        assert!(late_brightness > early_brightness);
    }

    #[test]
    fn test_color_temp_follows_brightness() {
        let module = SimpleProfile::with_defaults();

        let night = make_context(3.0);
        let noon = make_context(12.0);

        let night_temp = module.calculate_color_temperature(&night);
        let noon_temp = module.calculate_color_temperature(&noon);

        // Night should be warmer (lower K), noon cooler (higher K)
        assert!(night_temp < noon_temp);
    }

    #[test]
    fn test_step_brighten_increases_brightness() {
        let module = SimpleProfile::with_defaults();
        let ctx = make_context(7.0); // During ramp up

        let current = module.calculate_brightness(&ctx);
        let result = module.calculate_step(&ctx, StepAction::Brighten);

        assert!(result.values.brightness > current);
        assert!(!result.at_boundary);
    }

    #[test]
    fn test_step_dim_decreases_brightness() {
        let module = SimpleProfile::with_defaults();
        let ctx = make_context(19.0); // During ramp down

        let current = module.calculate_brightness(&ctx);
        let result = module.calculate_step(&ctx, StepAction::Dim);

        assert!(result.values.brightness < current);
    }

    #[test]
    fn test_step_at_maximum_returns_boundary() {
        let module = SimpleProfile::with_defaults();
        let ctx = make_context(12.0); // At maximum

        let result = module.calculate_step(&ctx, StepAction::Brighten);

        assert!(result.at_boundary);
        assert_eq!(result.time_offset_minutes, 0.0);
    }

    #[test]
    fn test_step_at_minimum_returns_boundary() {
        let module = SimpleProfile::with_defaults();
        let ctx = make_context(3.0); // At minimum

        let result = module.calculate_step(&ctx, StepAction::Dim);

        assert!(result.at_boundary);
        assert_eq!(result.time_offset_minutes, 0.0);
    }

    #[test]
    fn test_brightness_always_in_configured_range() {
        let module = SimpleProfile::with_defaults();

        for hour in 0..24 {
            let ctx = make_context(hour as f32);
            let brightness = module.calculate_brightness(&ctx);

            assert!(brightness >= module.min_brightness());
            assert!(brightness <= module.max_brightness());
        }
    }

    #[test]
    fn test_color_temp_always_in_configured_range() {
        let module = SimpleProfile::with_defaults();

        for hour in 0..24 {
            let ctx = make_context(hour as f32);
            let temp = module.calculate_color_temperature(&ctx);

            assert!(temp >= module.min_color_temp());
            assert!(temp <= module.max_color_temp());
        }
    }

    #[test]
    fn test_custom_config() {
        let config = SimpleProfileConfig {
            common: CommonCurveConfig {
                min_brightness: 10,
                max_brightness: 90,
                min_color_temp: 2700,
                max_color_temp: 5000,
                max_dim_steps: 4,
            },
            dawn_hour: 7.0,
            peak_hour: 10.0,
            dusk_hour: 17.0,
            night_hour: 20.0,
        };

        let module = SimpleProfile::new(config);

        assert_eq!(module.min_brightness(), 10);
        assert_eq!(module.max_brightness(), 90);
        assert_eq!(module.min_color_temp(), 2700);
        assert_eq!(module.max_color_temp(), 5000);
    }
}
```

### Registration and Usage Example

```rust
use std::sync::Arc;
use rhythm_core::light_profile::{CurveContext, LightProfileRegistry};
use rhythm_core::solar::SolarTime;

fn main() {
    // Create registry with default Rhythm module
    let mut registry = LightProfileRegistry::new();

    // Create and register our simple module
    let simple = Arc::new(SimpleProfile::with_defaults());
    registry.register(simple);

    // List available modules
    println!("Available modules:");
    for (id, name) in registry.available_profiles() {
        println!("  - {}: {}", id, name);
    }

    // Switch to simple module
    registry.set_active_profile("simple");
    println!("\nActive profile: {}", registry.active_profile().name());

    // Calculate values throughout the day
    println!("\nLighting values throughout the day:");
    println!("{:>6} {:>12} {:>8}", "Hour", "Brightness", "Color K");
    println!("{:-<6} {:-<12} {:-<8}", "", "", "");

    for hour in (0..24).step_by(3) {
        let ctx = CurveContext::new(hour as f32, SolarTime::default(), None);
        let values = registry.active_profile().calculate(&ctx);

        println!(
            "{:>5}h {:>11}% {:>7}K",
            hour, values.brightness, values.kelvin
        );
    }

    // Demonstrate stepping
    println!("\nStep dimming from 7 AM:");
    let mut ctx = CurveContext::new(7.0, SolarTime::default(), None);
    let mut offset = 0.0f32;

    for i in 0..6 {
        let result = registry.active_profile().calculate_step(&ctx, StepAction::Brighten);
        println!(
            "  Step {}: {}% @ {}K (offset: {:.1} min, boundary: {})",
            i + 1,
            result.values.brightness,
            result.values.kelvin,
            result.time_offset_minutes,
            result.at_boundary
        );

        if result.at_boundary {
            break;
        }

        offset += result.time_offset_minutes;
        ctx = ctx.with_offset(result.time_offset_minutes);
    }
}
```

### Expected Output

```
Available modules:
  - rhythm: Rhythm Profile
  - simple: Simple Linear Curve

Active profile: Simple Linear Curve

Lighting values throughout the day:
  Hour   Brightness  Color K
------ ------------ --------
    0h           1%     500K
    3h           1%     500K
    6h           1%     500K
    9h         100%    6500K
   12h         100%    6500K
   15h         100%    6500K
   18h         100%    6500K
   21h           1%     500K

Step dimming from 7 AM:
  Step 1: 17% @ 1500K (offset: 30.0 min, boundary: false)
  Step 2: 34% @ 2500K (offset: 30.0 min, boundary: false)
  Step 3: 51% @ 3500K (offset: 30.0 min, boundary: false)
  Step 4: 67% @ 4500K (offset: 30.0 min, boundary: false)
  Step 5: 84% @ 5500K (offset: 30.0 min, boundary: false)
  Step 6: 100% @ 6500K (offset: 30.0 min, boundary: true)
```

---

## Summary

Creating a custom `LightProfileModule` involves:

1. **Implementing the trait**: Provide all 11 required methods plus use the default `calculate_with_offset()`
2. **Defining your algorithm**: How brightness and color temperature change over time
3. **Supporting step operations**: Enable manual dimming along your curve
4. **Creating configuration**: Define adjustable parameters with serde support
5. **Registering with the system**: Add to the registry and set as active when needed

The `LightProfile` provides a sophisticated reference implementation using logistic curves, while the `SimpleProfile` example shows a straightforward linear approach.

Start simple, test thoroughly, and iterate on your algorithm based on real-world feedback.
