#!/usr/bin/env bash
set -Eeuo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target="${WORK_HARNESS_AGENT_DISPATCH_TARGET:-/usr/local/bin/work-harness-agent-dispatch}"

if [[ "${EUID}" -ne 0 && "$target" == /usr/local/bin/* ]]; then
  echo "Installing to $target requires root; rerun with sudo or set WORK_HARNESS_AGENT_DISPATCH_TARGET." >&2
  exit 1
fi

install -m 0755 "$script_dir/work-harness-agent-dispatch.sh" "$target"
echo "Installed Work Harness agent dispatch script to $target"
