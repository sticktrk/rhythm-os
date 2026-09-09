#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
shared_root="os/rust/integrations/rhythm-ble/"
exceptions_file="${repo_root}/tools/ci/references/bluez-owner-exceptions.tsv"
today="$(date -u +%F)"

if [[ ! -f "$exceptions_file" ]]; then
  echo "missing BlueZ ownership exception manifest: $exceptions_file" >&2
  exit 1
fi

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-bluez-ownership.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
observed="$tmp_dir/observed.tsv"
violations="$tmp_dir/violations.tsv"
: >"$observed"
: >"$violations"

valid_category() {
  case "$1" in
    dependency | session | discovery | runtime) return 0 ;;
    *) return 1 ;;
  esac
}

exception_allows() {
  local target_path="$1"
  local target_category="$2"
  awk -F '\t' -v path="$target_path" -v category="$target_category" '
    $0 !~ /^[[:space:]]*(#|$)/ && $1 == path {
      count++
      n = split($2, categories, ",")
      for (i = 1; i <= n; i++) {
        if (categories[i] == category) allowed = 1
      }
    }
    END {
      if (count > 1) exit 2
      exit allowed ? 0 : 1
    }
  ' "$exceptions_file"
}

manifest_failed=0
duplicate_paths="$(awk -F '\t' '
  $0 !~ /^[[:space:]]*(#|$)/ { count[$1]++ }
  END { for (path in count) if (count[path] > 1) print path }
' "$exceptions_file")"
if [[ -n "$duplicate_paths" ]]; then
  while IFS= read -r path; do
    echo "duplicate BlueZ exception path: $path" >&2
  done <<<"$duplicate_paths"
  manifest_failed=1
fi

while IFS=$'\t' read -r path categories owner expires reason extra; do
  [[ -z "${path//[[:space:]]/}" || "$path" == \#* ]] && continue

  if [[ -n "${extra:-}" || -z "$categories" || -z "$owner" || -z "$expires" || -z "$reason" ]]; then
    echo "invalid BlueZ exception record (expected five tab-separated fields): $path" >&2
    manifest_failed=1
    continue
  fi
  if [[ "$path" = /* || "/$path/" == *'/../'* || "$path" == *'*'* || "$path" == *'?'* || "$path" == *'['* ]]; then
    echo "BlueZ exception must name one exact repo-relative source or manifest: $path" >&2
    manifest_failed=1
  elif [[ ",$categories," == *,dependency,* && "$categories" != "dependency" ]]; then
    echo "BlueZ dependency exceptions cannot mix source ownership categories: $path" >&2
    manifest_failed=1
  elif [[ "$categories" == "dependency" && "$path" != */Cargo.toml ]]; then
    echo "BlueZ dependency exception must name one exact Cargo.toml: $path" >&2
    manifest_failed=1
  elif [[ "$categories" != "dependency" && "$path" != *.rs ]]; then
    echo "BlueZ ownership exception must name one exact Rust source: $path" >&2
    manifest_failed=1
  elif [[ "$path/" == "$shared_root"* ]]; then
    echo "shared rhythm-ble sources cannot be exception records: $path" >&2
    manifest_failed=1
  elif [[ ! -f "$repo_root/$path" ]]; then
    echo "stale BlueZ exception path does not exist: $path" >&2
    manifest_failed=1
  fi
  if [[ ! "$owner" =~ ^[A-Za-z0-9._/-]+$ ]]; then
    echo "BlueZ exception owner must be a stable team/component token: $path ($owner)" >&2
    manifest_failed=1
  fi
  if [[ ! "$expires" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
    echo "BlueZ exception expiry must be YYYY-MM-DD: $path ($expires)" >&2
    manifest_failed=1
  elif [[ "$expires" < "$today" ]]; then
    echo "expired BlueZ ownership exception: $path (expired $expires, owner $owner)" >&2
    manifest_failed=1
  fi
  if (( ${#reason} < 30 )); then
    echo "BlueZ exception needs a specific reason (30+ characters): $path" >&2
    manifest_failed=1
  fi

  IFS=',' read -r -a category_list <<<"$categories"
  if (( ${#category_list[@]} == 0 )); then
    echo "BlueZ exception has no categories: $path" >&2
    manifest_failed=1
  fi
  for category in "${category_list[@]}"; do
    if ! valid_category "$category"; then
      echo "unknown BlueZ exception category '$category' for $path" >&2
      manifest_failed=1
    fi
  done
done <"$exceptions_file"

if (( manifest_failed != 0 )); then
  exit 1
fi

# Direct dependency ownership is the primary boundary: import aliases cannot
# hide a Cargo dependency that could recreate a session, scanner, or Tokio
# owner. Source checks also cover inferred raw Adapter handles received through
# the shared client API, which require no downstream bluer dependency at all.
while IFS= read -r -d '' manifest; do
  relative="${manifest#"$repo_root/"}"
  [[ "$relative/" == "$shared_root"* ]] && continue

  if grep -Eq "^[[:space:]]*(bluer|\"bluer\"|'bluer')[[:space:]]*=" "$manifest" \
    || grep -Eq "^[[:space:]]*\[[^]]*dependencies\.(bluer|\"bluer\"|'bluer')\][[:space:]]*$" "$manifest" \
    || grep -Eq "(^|[,{[:space:]])package[[:space:]]*=[[:space:]]*(\"bluer\"|'bluer')" "$manifest"; then
    printf '%s\tdependency\n' "$relative" >>"$observed"
    if exception_allows "$relative" dependency; then
      printf 'allowed BlueZ dependency exception: %s\n' "$relative"
    else
      printf '%s\tdependency\n' "$relative" >>"$violations"
    fi
  fi
done < <(find "$repo_root/os/rust" -type f -name 'Cargo.toml' -print0)

while IFS= read -r -d '' source; do
  relative="${source#"$repo_root/"}"
  [[ "$relative/" == "$shared_root"* ]] && continue

  categories=""
  names_bluer=0
  if grep -Eq '(^|[^[:alnum:]_])(use[[:space:]]+bluer|bluer::)' "$source"; then
    names_bluer=1
  fi

  # Session and runtime type names are too generic to inspect without a
  # direct bluer spelling. Discovery ownership is different: a downstream
  # closure can receive an inferred Adapter from BluezClient and call scanner
  # methods without importing or depending directly on bluer. Scan those
  # unambiguous APIs in every Rust source, and treat discover_devices as BlueZ
  # only when the same file enters a BluezClient adapter-operation callback.
  if (( names_bluer != 0 )); then
    if grep -Eq '(^|[^[:alnum:]_:])([[:alpha:]_][[:alnum:]_]*::)?Session::new[[:space:]]*\(' "$source" \
      || grep -Eq '(^|[^[:alnum:]_:])[[:alpha:]_][[:alnum:]_]*Session::new[[:space:]]*\(' "$source"; then
      categories="$categories session"
    fi
    if grep -Eq '(tokio::runtime::(Builder|Runtime)|(^|[^[:alnum:]_:])(Builder|Runtime)::new(_current_thread|_multi_thread)?[[:space:]]*\()' "$source"; then
      categories="$categories runtime"
    fi
  fi
  if grep -Eq '\.[[:space:]]*(discover_devices_with_changes|set_discovery_filter)[[:space:]]*\(' "$source" \
    || { grep -Eq '(run|try_run)_adapter_operation' "$source" \
      && grep -Eq '\.[[:space:]]*discover_devices[[:space:]]*\(' "$source"; } \
    || { (( names_bluer != 0 )) && grep -Eq '\.[[:space:]]*monitor[[:space:]]*\(' "$source"; }; then
    categories="$categories discovery"
  fi

  for category in $categories; do
    printf '%s\t%s\n' "$relative" "$category" >>"$observed"
    if exception_allows "$relative" "$category"; then
      printf 'allowed BlueZ %s exception: %s\n' "$category" "$relative"
    else
      printf '%s\t%s\n' "$relative" "$category" >>"$violations"
    fi
  done
done < <(find "$repo_root/os/rust" -type f -name '*.rs' -print0)

stale_failed=0
while IFS=$'\t' read -r path categories owner expires reason extra; do
  [[ -z "${path//[[:space:]]/}" || "$path" == \#* ]] && continue
  IFS=',' read -r -a category_list <<<"$categories"
  for category in "${category_list[@]}"; do
    if ! awk -F '\t' -v path="$path" -v category="$category" \
      '$1 == path && $2 == category { found = 1 } END { exit found ? 0 : 1 }' "$observed"; then
      echo "stale BlueZ exception category is no longer observed: $path ($category, owner $owner, expires $expires)" >&2
      stale_failed=1
    fi
  done
done <"$exceptions_file"

if (( stale_failed != 0 )); then
  exit 1
fi

if [[ -s "$violations" ]]; then
  echo "direct BlueZ ownership is restricted to $shared_root" >&2
  while IFS=$'\t' read -r path category; do
    echo "  $path: $category ownership" >&2
  done <"$violations"
  echo "central-role drivers must use rhythm-ble's public client API" >&2
  echo "for a genuinely separate platform role, add one exact path/category/owner/expiry/reason record to:" >&2
  echo "  ${exceptions_file#"$repo_root/"}" >&2
  exit 1
fi

echo "shared BlueZ ownership invariant passed"
