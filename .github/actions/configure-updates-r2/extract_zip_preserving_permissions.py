#!/usr/bin/env python3
"""Extract a trusted ZIP archive while restoring archived Unix modes."""

from __future__ import annotations

import os
from pathlib import Path, PurePosixPath
import stat
import sys
import zipfile


def main() -> int:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} ARCHIVE DESTINATION", file=sys.stderr)
        return 2

    archive_path = Path(sys.argv[1])
    destination = Path(sys.argv[2])
    destination.mkdir(parents=True, exist_ok=True)
    destination_root = destination.resolve()

    with zipfile.ZipFile(archive_path) as archive:
        for member in archive.infolist():
            archive_name = PurePosixPath(member.filename)
            if archive_name.is_absolute() or ".." in archive_name.parts:
                raise ValueError(f"Unsafe ZIP member path: {member.filename}")

            archived_mode = (member.external_attr >> 16) & 0xFFFF
            if stat.S_ISLNK(archived_mode):
                raise ValueError(f"Symbolic links are not supported: {member.filename}")

            extracted_path = Path(archive.extract(member, destination_root)).resolve()
            if extracted_path != destination_root and destination_root not in extracted_path.parents:
                raise ValueError(f"ZIP member escaped destination: {member.filename}")

            permissions = stat.S_IMODE(archived_mode)
            if permissions:
                os.chmod(extracted_path, permissions)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
