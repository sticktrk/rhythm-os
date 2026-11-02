#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
ios="$root/app/flutter/rhythm_app/ios"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/apple-health-ios.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
for test in AppleHealthIntervalTests AppleHealthSampleTests; do
  xcrun swiftc "$ios/Runner/AppleHealthInterval.swift" "$ios/Tests/$test.swift" -o "$tmp/$test"
  "$tmp/$test"
done
