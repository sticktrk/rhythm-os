#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root="$(cd "$script_dir/../.." && pwd)"
cd "$repo_root"

missing=()

check_anchor() {
  local file=$1
  local pattern=$2
  local description=$3

  if [[ ! -f "$file" ]]; then
    missing+=("$description: missing file $file")
    return
  fi
  if ! rg -Fq -- "$pattern" "$file"; then
    missing+=("$description: missing anchor '$pattern' in $file")
  fi
}

check_anchor \
  "app/flutter/rhythm_app/lib/services/analytics_service.dart" \
  "class AnalyticsService" \
  "typed Flutter analytics service"
check_anchor \
  "app/flutter/rhythm_app/lib/backend/analytics/posthog_analytics_backend.dart" \
  "class PostHogAnalyticsBackend" \
  "PostHog adapter"
check_anchor \
  "app/flutter/rhythm_app/lib/backend/backend_provider.dart" \
  "PostHogAppConfig.isConfigured" \
  "configured PostHog selection"
check_anchor \
  "app/flutter/rhythm_app/lib/config/posthog_config.dart" \
  "POSTHOG_API_KEY" \
  "PostHog build configuration"
check_anchor \
  "app/flutter/rhythm_app/lib/main.dart" \
  "await AnalyticsService().initialize();" \
  "analytics startup"
check_anchor \
  "app/flutter/rhythm_app/lib/services/account_session_service.dart" \
  "AnalyticsService().resetUser();" \
  "analytics identity reset"
check_anchor \
  "os/rust/core/rhythm-os/src/activity.rs" \
  "pub const LIGHT_ACTIVITY_HISTORY_LIMIT: usize = 2_000;" \
  "bounded Rust activity history"
check_anchor \
  "os/rust/core/rhythm-os/src/activity.rs" \
  "pub struct LightActivityRecord" \
  "Rust activity record"
check_anchor \
  "os/rust/core/rhythm-os/src/activity_cloud.rs" \
  "pub fn enqueue_light_activity_upload" \
  "device-side activity upload"
check_anchor \
  "tools/app/supabase/functions/server-activity-bootstrap/index.ts" \
  "server_light_activity_device_tokens" \
  "activity token bootstrap"
check_anchor \
  "tools/app/supabase/functions/server-activity-ingest/index.ts" \
  ".from('server_light_activity_events')" \
  "activity Edge Function ingest"
check_anchor \
  "tools/app/supabase/functions/server-activity-ingest/index.ts" \
  "onConflict: 'user_id,hub_id,event_id'" \
  "activity ingest deduplication"
check_anchor \
  "tools/app/supabase/migrations/20260702000000_add_server_light_activity_events.sql" \
  "correlation_id TEXT" \
  "Supabase correlation schema"
check_anchor \
  "tools/app/supabase/migrations/20260702000000_add_server_light_activity_events.sql" \
  "ENABLE ROW LEVEL SECURITY" \
  "Supabase activity RLS"
check_anchor \
  "tools/app/supabase/migrations/20260702000000_add_server_light_activity_events.sql" \
  "CREATE VIEW public.rhythm_support_server_light_activity_events" \
  "support-safe activity analytics view"
check_anchor \
  "tools/app/supabase/migrations/20260705010000_add_server_light_activity_event_conflict_key.sql" \
  "server_light_activity_events_user_hub_event_idx" \
  "Supabase activity conflict-key repair"

if (( ${#missing[@]} > 0 )); then
  echo "Rhythm analytics contract anchors have drifted:" >&2
  for item in "${missing[@]}"; do
    echo "  - $item" >&2
  done
  echo "Inspect the failed product anchors and update this checker with their owners." >&2
  exit 1
fi

echo "Rhythm analytics contract anchors intact."
