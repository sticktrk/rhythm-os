#!/usr/bin/env bash
set -euo pipefail
root=$(git rev-parse --show-toplevel)
ios="$root/app/flutter/rhythm_app/ios"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/phone-matter-ios.XXXXXX")
server_pid=""
cleanup() {
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT
mkdir -p "$tmp/Probe.app/Contents/MacOS"
python3 - "$ios/MatterCommissioningExtension/Info.plist" "$tmp/Probe.app/Contents/Info.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'rb') as f:
    info = plistlib.load(f)
ats = info['NSAppTransportSecurity']
assert ats['NSAllowsLocalNetworking'] is True
assert ats.get('NSAllowsArbitraryLoads', False) is False
for network in ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', 'fe80::/10']:
    assert ats['NSExceptionDomains'][network]['NSExceptionAllowsInsecureHTTPLoads'] is True
info.update(CFBundleExecutable='Probe', CFBundleIdentifier='lighting.rhythm.test.phone-matter',
            CFBundlePackageType='APPL', CFBundleVersion='1', CFBundleShortVersionString='1.0')
info.pop('NSExtension', None)
with open(sys.argv[2], 'wb') as f:
    plistlib.dump(info, f)
PY
python3 - "$tmp/port" <<'PY' &
from http.server import HTTPServer, BaseHTTPRequestHandler
from pathlib import Path
import sys
class Echo(BaseHTTPRequestHandler):
    def do_POST(self):
        assert self.path == '/api/devices/pair'
        body = self.rfile.read(int(self.headers['Content-Length']))
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_): pass
server = HTTPServer(('127.0.0.1', 0), Echo)
Path(sys.argv[1]).write_text(str(server.server_port))
server.serve_forever()
PY
server_pid=$!
for _ in {1..100}; do
  [[ -s "$tmp/port" ]] && break
  sleep 0.1
done
test -s "$tmp/port"
xcrun swiftc "$ios/PhoneMatterRequestContext.swift" \
  "$ios/Tests/PhoneMatterRequestContextTests.swift" -o "$tmp/Probe.app/Contents/MacOS/Probe"
"$tmp/Probe.app/Contents/MacOS/Probe" "$(cat "$tmp/port")" "$root/tools/app/testdata/phone_matter_contract.json"
