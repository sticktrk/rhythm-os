#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"

python3 "$SCRIPT_DIR/prd-inventory/extract_quantization.py" \
  --root "$PROJECT_ROOT" \
  --check

python3 "$SCRIPT_DIR/prd-inventory/extract_activity_sources.py" \
  --root "$PROJECT_ROOT" \
  --check

python3 "$SCRIPT_DIR/prd-inventory/extract_action_catalog.py" \
  --root "$PROJECT_ROOT" \
  --check

python3 "$SCRIPT_DIR/prd-inventory/extract_axis_specs.py" \
  --root "$PROJECT_ROOT" \
  --check

python3 "$SCRIPT_DIR/prd-inventory/extract_settings_registry.py" \
  --root "$PROJECT_ROOT" \
  --check

python3 "$SCRIPT_DIR/prd-inventory/extract_endpoint_routes.py" \
  --root "$PROJECT_ROOT" \
  --check

python3 "$SCRIPT_DIR/prd-inventory/extract_store_manifest.py" \
  --root "$PROJECT_ROOT" \
  --check
