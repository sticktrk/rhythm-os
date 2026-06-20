# PRD Inventory Scripts

These scripts enforce the mechanical inventories required by `PRD_FINAL_2`.
They are intentionally small, deterministic, and dependency-free so they can run
early in CI before the heavier Rust jobs.

## Quantization Inventory

`extract_quantization.py` scans `rust/core/rhythm-core/src` and
`rust/core/rhythm-os/src` for `as u8` casts, classifies them, and marks the
runtime lighting brightness conversions that must be absorbed by the Circadian
value-pipeline strangler before behavior changes land.

```bash
python3 tools/os/scripts/prd-inventory/extract_quantization.py --write
python3 tools/os/scripts/prd-inventory/extract_quantization.py --check
```

The checked-in `quantization_sites.json` is a gate. If a future edit adds,
removes, reclassifies, or reshapes a conversion site, update the implementation
and the inventory in the same change. Line-only movement is intentionally not
part of the stable artifact.

## Activity Source Classification

`extract_activity_sources.py` reads the typed `ACTIVITY_SOURCE_RULES` table in
`rhythm-core/src/activity.rs` and emits the checked-in
`activity_sources.json`. This table is load-bearing for Circadian
`only_if_untouched` schedule parity.

```bash
python3 tools/os/scripts/prd-inventory/extract_activity_sources.py --write
python3 tools/os/scripts/prd-inventory/extract_activity_sources.py --check
```

## Action Catalog

`extract_action_catalog.py` reads the typed `ACTION_CATALOG` table in
`rhythm-core/src/automation.rs` and emits the checked-in
`action_catalog.json`. The Rust table maps Python action ids and retired
aliases into stable action semantics before importer and input-binding code use
them.

```bash
python3 tools/os/scripts/prd-inventory/extract_action_catalog.py --write
python3 tools/os/scripts/prd-inventory/extract_action_catalog.py --check
```

## Axis Specification

`extract_axis_specs.py` reads the typed `AXIS_SPECS` table in
`rhythm-core/src/axis.rs` and emits the checked-in `axis_specs.json`. The Rust
table is the source of truth for scope ownership, inheritance composition,
promotion, and reset behavior.

```bash
python3 tools/os/scripts/prd-inventory/extract_axis_specs.py --write
python3 tools/os/scripts/prd-inventory/extract_axis_specs.py --check
```

## Endpoint Route Inventory

`extract_endpoint_routes.py` reads the typed `SHARED_API_ROUTES` table in
`rhythm-os/src/routes.rs` and emits the checked-in `endpoint_routes.json`.
This keeps shared API surface drift visible while platform routers continue to
consume the Rust route primitive directly.

```bash
python3 tools/os/scripts/prd-inventory/extract_endpoint_routes.py --write
python3 tools/os/scripts/prd-inventory/extract_endpoint_routes.py --check
```

## Settings Registry

`extract_settings_registry.py` reads the typed `SETTING_SPECS` table in
`rhythm-core/src/settings.rs` and emits the checked-in
`settings_registry.json`. The Rust table is the source of truth for setting
keys, defaults, value kinds, and PRD consumer ownership.

```bash
python3 tools/os/scripts/prd-inventory/extract_settings_registry.py --write
python3 tools/os/scripts/prd-inventory/extract_settings_registry.py --check
```

## Store Manifest

`extract_store_manifest.py` reads the typed `STORE_MANIFEST` table in
`rhythm-os/src/store_manifest.rs` and emits the checked-in
`store_manifest.json`. The Rust table is the source of truth for factory reset,
backup, and restore policy across legacy stores and PRD-introduced stores.

```bash
python3 tools/os/scripts/prd-inventory/extract_store_manifest.py --write
python3 tools/os/scripts/prd-inventory/extract_store_manifest.py --check
```
