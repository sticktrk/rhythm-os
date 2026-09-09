#!/usr/bin/env python3
"""Start the real server with fresh storage and no cloud configuration."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/rhythm-server'))
    parser.add_argument('--timeout', type=float, default=30)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='rhythm-server-smoke-') as temporary:
        root = Path(temporary)
        env = {'PATH': os.environ.get('PATH', ''), 'HOME': temporary, 'TMPDIR': temporary}
        with (root / 'server.log').open('w+') as log:
            server = subprocess.Popen([str(binary), '--port', str(port), '--data-dir', str(root / 'state'),
                                       '--log-level', 'warn'], cwd=root, env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + args.timeout
                while time.monotonic() < deadline:
                    if server.poll() is not None:
                        raise RuntimeError(f'Server exited before accepting requests ({server.returncode}).')
                    try:
                        with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/nodes/state', timeout=1) as response:
                            state = json.load(response)
                            if not isinstance(state, (dict, list)):
                                raise RuntimeError('Server did not return a state object or node list.')
                            if not (root / 'state').is_dir():
                                raise RuntimeError('Server did not initialize its data directory.')
                            print('PASS: fresh server starts and serves node state without cloud credentials or hardware.')
                            return
                    except (urllib.error.URLError, TimeoutError):
                        time.sleep(0.1)
                raise RuntimeError('Timed out waiting for the fresh server API.')
            except Exception:
                log.flush()
                log.seek(0)
                print(log.read())
                raise
            finally:
                if server.poll() is None:
                    server.terminate()
                    try:
                        server.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait(timeout=5)


if __name__ == '__main__':
    main()
