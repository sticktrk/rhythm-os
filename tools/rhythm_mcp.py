"""Client for Rhythm's stateless JSON responses over MCP Streamable HTTP."""

import argparse
import json
from pathlib import Path
import sys
import urllib.error
import urllib.request
import uuid


VERSION = "2025-06-18"


class McpError(RuntimeError):
    pass


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


class RhythmMcp:
    def __init__(self, url, token, server_name, request_id=None):
        if not token.strip():
            raise McpError("The selected MCP bearer is empty")
        self.url = url
        self.headers = {
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if request_id:
            self.headers["X-Request-Id"] = request_id
        initialized = self._rpc(
            "initialize",
            {
                "protocolVersion": VERSION,
                "capabilities": {},
                "clientInfo": {"name": "rhythm-workflows", "version": "1"},
            },
        )
        if (
            initialized.get("protocolVersion") != VERSION
            or initialized.get("serverInfo", {}).get("name") != server_name
        ):
            raise McpError("Unexpected Rhythm MCP server or protocol version")
        self.headers["MCP-Protocol-Version"] = VERSION
        self._rpc("notifications/initialized", {}, notification=True)
        self.tools = {
            t["name"]: t for t in self._rpc("tools/list", {}).get("tools", [])
        }

    def _rpc(self, method, params, notification=False):
        message = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notification:
            message["id"] = uuid.uuid4().hex
        request = urllib.request.Request(
            self.url,
            data=json.dumps(message).encode(),
            headers=self.headers,
            method="POST",
        )
        try:
            with urllib.request.build_opener(_NoRedirect).open(
                request, timeout=30
            ) as response:
                if notification and response.status == 202:
                    return
                reply = json.load(response)
        except urllib.error.HTTPError as error:
            raise McpError(f"{method} returned HTTP {error.code}") from None
        except (OSError, ValueError) as error:
            raise McpError(f"{method} returned no valid MCP response") from error
        if (
            not isinstance(reply, dict)
            or reply.get("jsonrpc") != "2.0"
            or reply.get("id") != message.get("id")
            or "error" in reply
            or not isinstance(reply.get("result"), dict)
        ):
            raise McpError(f"{method} returned a JSON-RPC error or invalid response")
        return reply["result"]

    def call(self, name, arguments=None):
        if name not in self.tools:
            raise McpError(f"{name} is unavailable in the connected catalog")
        reply = self._rpc("tools/call", {"name": name, "arguments": arguments or {}})
        content = reply.get("content")
        text = None
        if isinstance(content, list) and content and isinstance(content[0], dict):
            if content[0].get("type") == "text" and isinstance(
                content[0].get("text"), str
            ):
                text = content[0]["text"]
        if reply.get("isError"):
            # The staff endpoint projects its errors to bounded, actionable guidance.
            reason = text or "Tool error"
            raise McpError(f"{name} failed: {reason[:300]}")
        value = reply.get("structuredContent")
        if value is None and text is not None:
            try:
                value = json.loads(text)
            except ValueError:
                pass
        if not isinstance(value, dict):
            raise McpError(f"{name} returned no object result")
        return value

    def contract(self, uri):
        resources = self._rpc("resources/list", {}).get("resources", [])
        if not any(resource.get("uri") == uri for resource in resources):
            raise McpError("The expected Rhythm contract is unavailable")
        contents = self._rpc("resources/read", {"uri": uri}).get("contents", [])
        for item in contents:
            if item.get("uri") == uri:
                return json.loads(item["text"])
        raise McpError("Missing Rhythm contract resource")


def add_commands(parser):
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list")
    sub.add_parser("contract")
    call = sub.add_parser("call", help="Read a JSON arguments object from stdin")
    call.add_argument("tool")


def run_command(client, args, contract_uri):
    if args.command == "list":
        value = list(client.tools.values())
    elif args.command == "contract":
        value = client.contract(contract_uri)
    else:
        arguments = json.load(sys.stdin)
        if not isinstance(arguments, dict):
            raise ValueError("Expected a JSON object on stdin")
        value = client.call(args.tool, arguments)
    print(json.dumps(value, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--surface", choices=["hub", "account"], required=True)
    parser.add_argument("--url", required=True, help="Full MCP endpoint URL")
    parser.add_argument(
        "--token-file", type=Path, required=True, help="Bearer issued for this surface"
    )
    add_commands(parser)
    args = parser.parse_args()
    name, uri = {
        "hub": ("Rhythm", "rhythm://hub/assistant/contract"),
        "account": ("Rhythm account", "rhythm://cloud/customer/contract"),
    }[args.surface]
    try:
        run_command(
            RhythmMcp(args.url, args.token_file.read_text().strip(), name), args, uri
        )
    except (OSError, McpError, ValueError) as error:
        sys.exit(str(error))
