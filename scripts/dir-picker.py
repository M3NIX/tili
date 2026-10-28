#!/usr/bin/env python3
"""Fast directory launcher for tili using fd."""

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

HOME = Path.home()


def emit_directory(path):
    relative = path.relative_to(HOME)

    label = "~" if path == HOME else f"~/{relative}"

    print(
        json.dumps(
            {
                "id": str(path),
                "label": label,
            }
        ),
    )


def list_directories():
    fd = shutil.which("fd") or shutil.which("fdfind")

    if not fd:
        print("Error: fd is not installed", file=sys.stderr)
        return 1

    emit_directory(HOME)

    command = [
        fd,
        "--type",
        "d",
        "--follow",
        "--exclude",
        ".git",
        "--print0",
        ".",
        str(HOME),
    ]

    with subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    ) as process:
        buffer = b""

        while chunk := process.stdout.read(65536):
            buffer += chunk
            *paths, buffer = buffer.split(b"\0")

            for raw_path in paths:
                if raw_path:
                    emit_directory(Path(os.fsdecode(raw_path)))

        if buffer:
            emit_directory(Path(os.fsdecode(buffer)))

        return process.wait()


def select_directory(path):
    target = Path(path)

    if not target.is_dir():
        return 1

    print(
        json.dumps(
            {
                "action": "open_pane",
                "command": "bash",
                "args": [
                    "-c",
                    'cd -- "$1" && exec bash -i',
                    "bash",
                    str(target),
                ],
            }
        )
    )
    return 0


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "list":
        return list_directories()

    if len(sys.argv) == 3 and sys.argv[1] == "select":
        return select_directory(sys.argv[2])

    return 2


if __name__ == "__main__":
    sys.exit(main())
