"""Unix-socket RPC server for the Rhythm CHIP controller sidecar."""

from __future__ import annotations

import argparse
import json
import os
import signal
import socket
from pathlib import Path
from typing import Optional

from .service import ChipControllerService, OfficialChipBackend


def _serve_forever(socket_path: Path, service: ChipControllerService) -> int:
    if socket_path.exists():
        socket_path.unlink()

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.bind(str(socket_path))
    sock.listen(16)

    stopping = False

    def _handle_signal(_signum: int, _frame: object) -> None:
        nonlocal stopping
        stopping = True
        try:
            sock.close()
        except OSError:
            pass

    old_int = signal.signal(signal.SIGINT, _handle_signal)
    old_term = signal.signal(signal.SIGTERM, _handle_signal)
    try:
        while not stopping:
            try:
                conn, _ = sock.accept()
            except OSError:
                if stopping:
                    break
                raise
            with conn:
                data = b""
                while not data.endswith(b"\n"):
                    chunk = conn.recv(65536)
                    if not chunk:
                        break
                    data += chunk
                if not data:
                    continue
                request = json.loads(data.decode("utf-8").strip())
                response = service.handle(request)
                conn.sendall(json.dumps(response, separators=(",", ":")).encode("utf-8") + b"\n")
    finally:
        signal.signal(signal.SIGINT, old_int)
        signal.signal(signal.SIGTERM, old_term)
        try:
            sock.close()
        except OSError:
            pass
        if socket_path.exists():
            socket_path.unlink()
    return 0


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="rhythm_chip_controller")
    parser.add_argument("--socket", required=True, help="Unix domain socket path")
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    args = build_arg_parser().parse_args(argv)
    socket_path = Path(args.socket)
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    service = ChipControllerService(OfficialChipBackend())
    return _serve_forever(socket_path, service)

