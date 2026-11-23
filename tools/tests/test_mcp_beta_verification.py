"""Exercise the release verifier with a local Git remote, OTA feed and MCP hub."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]


class BetaVerificationTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.repo = Path(temp.name)
        self.verifier = Path("tools/os/scripts/verify-beta-release.sh")
        for path in [
            self.verifier,
            "tools/os/scripts/verify_mcp_device.py",
            "tools/os/scripts/lib/version.sh",
            "tools/rhythm_mcp.py",
        ]:
            dest = self.repo / path
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / path, dest)
        for args in (
            ["init", "-q"],
            [
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
            ["tag", "v1.2.3-beta"],
            ["remote", "add", "origin", "."],
        ):
            subprocess.run(["git", *args], cwd=self.repo, check=True)
        self.calls = []
        self.installed = "1.2.3-beta"
        self.fail = False
        self.tools = [
            "read_hub_status",
            "read_appliance_update_status",
            "read_lighting_state",
            "read_integrations",
            "get_handoff",
        ]
        testcase = self

        class Handler(BaseHTTPRequestHandler):
            def reply(self, status, value):
                self.send_response(status)
                self.end_headers()
                if value is not None:
                    self.wfile.write(json.dumps(value).encode())

            def do_GET(self):
                assert self.path.startswith("/feed/rpiz/manifest.json?")
                self.reply(
                    200, {"version": "1.2.3-beta", "package": {"url": "fixture"}}
                )

            def do_POST(self):
                assert self.path == "/api/assistant/mcp"
                assert self.headers["User-Agent"] == "rhythm-workflows/1"
                assert (
                    self.headers["Authorization"] == "Bearer test-token"
                    and self.headers["X-Request-Id"] == "journey"
                )
                rpc = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                method = rpc["method"]
                if method == "notifications/initialized":
                    self.reply(202, None)
                    return
                if method == "initialize":
                    result = {
                        "protocolVersion": "2025-06-18",
                        "serverInfo": {"name": "Rhythm"},
                    }
                elif method == "tools/list":
                    result = {"tools": [{"name": t} for t in testcase.tools]}
                else:
                    assert method == "tools/call"
                    name = rpc["params"]["name"]
                    testcase.calls.append(name)
                    payload = {
                        "read_hub_status": {"server_instance_id": "hub-one"},
                        "read_appliance_update_status": {
                            "current_version": testcase.installed,
                            "stage": "idle",
                        },
                        "read_lighting_state": {"nodes": []},
                        "read_integrations": {"integrations": []},
                        "get_handoff": {"status": "handoff", "actor": "owner"},
                    }[name]
                    result = {
                        "content": [{"type": "text", "text": json.dumps(payload)}],
                        "isError": testcase.fail,
                    }
                self.reply(200, {"jsonrpc": "2.0", "id": rpc["id"], "result": result})

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.url = f"http://127.0.0.1:{server.server_port}"
        self.receipt = self.repo / "receipt.json"

    def verify(self, *args, ok=True):
        self.receipt.unlink(missing_ok=True)
        result = subprocess.run(
            [
                "bash",
                str(self.verifier),
                "--version",
                "1.2.3",
                "--updates-base-url",
                self.url + "/feed",
                "--receipt",
                str(self.receipt),
                "--journey-id",
                "journey",
                *args,
            ],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env={**os.environ, "RHYTHM_DEVICE_TOKEN": "test-token"},
        )
        self.assertEqual(result.returncode == 0, ok, result.stderr)
        self.assertNotIn("test-token", result.stdout + result.stderr)
        return json.loads(self.receipt.read_text()) if self.receipt.exists() else None

    def test_feed_and_live_scenarios(self):
        self.assertIsNone(self.verify()["device"])
        self.assertFalse(self.calls)
        state = self.verify("--device", self.url, "--scenario", "state")
        self.assertEqual(state["device"]["state"], {"nodes": []})
        self.assertEqual(
            self.calls,
            ["read_hub_status", "read_appliance_update_status", "read_lighting_state"],
        )
        handoff = self.verify(
            "--device", self.url, "--scenario", "onboarding", ok=False
        )
        self.assertEqual(handoff["result"], "awaiting_owner_verification")

    def test_wrong_version_tool_failure_and_older_catalog(self):
        self.installed = "old"
        self.assertIsNone(self.verify("--device", self.url, ok=False))
        self.fail = True
        self.assertIsNone(self.verify("--device", self.url, ok=False))
        self.fail = False
        self.tools.remove("read_appliance_update_status")
        self.assertIsNone(self.verify("--device", self.url, ok=False))


if __name__ == "__main__":
    unittest.main()
