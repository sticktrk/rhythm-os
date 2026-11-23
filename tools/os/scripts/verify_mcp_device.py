#!/usr/bin/env python3
"""Collect bench evidence for verify-beta-release.sh through the hub MCP catalog."""

import argparse
import json
import os
from pathlib import Path
import sys
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from rhythm_mcp import McpError, RhythmMcp


def verify(client, url, expected, scenario):
    hub = client.call("read_hub_status")
    if not hub.get("server_instance_id"):
        raise McpError("Hub did not return its installation identity")
    ota = client.call("read_appliance_update_status")
    if ota.get("status") == "handoff":
        raise McpError("This installation delegates its update status to its host")
    installed = ota.get("current_package_version") or ota.get("current_version")
    if installed != expected or ota.get("stage") not in (
        "idle",
        "ready",
        "checking",
        "updating",
        "restarting",
    ):
        raise McpError(f"Installed package or OTA stage does not verify {expected}")
    result = {
        "url": url,
        "scenario": scenario,
        "health": hub,
        "ota": ota,
        "state": None,
        "auth": None,
        "remote_access": None,
    }
    if scenario in ("state", "onboarding"):
        result["state"] = client.call("read_lighting_state")
    if scenario == "onboarding":
        result["integrations"] = client.call("read_integrations")
        result["owner_handoff"] = client.call("get_handoff", {"step": "owner_access"})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", required=True)
    parser.add_argument(
        "--scenario", required=True, choices=["smoke", "state", "onboarding", "ota"]
    )
    parser.add_argument("--expected", required=True)
    parser.add_argument("--journey-id", required=True)
    parser.add_argument("--token-file")
    args = parser.parse_args()
    parsed = urlsplit(args.device)
    if (
        parsed.scheme not in ("http", "https")
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.path.strip("/")
        or parsed.query
        or parsed.fragment
    ):
        parser.error("Supply a device base URL without credentials, query or fragment")
    token = (
        Path(args.token_file).read_text().strip()
        if args.token_file
        else os.environ.get("RHYTHM_DEVICE_TOKEN", "").strip()
    )
    if not token:
        parser.error(
            "Supply a scoped hub assistant bearer in --token-file or RHYTHM_DEVICE_TOKEN"
        )
    client = RhythmMcp(
        args.device.rstrip("/") + "/api/assistant/mcp", token, "Rhythm", args.journey_id
    )
    print(json.dumps(verify(client, args.device, args.expected, args.scenario)))


if __name__ == "__main__":
    try:
        main()
    except (McpError, OSError, ValueError) as error:
        sys.exit(str(error))
