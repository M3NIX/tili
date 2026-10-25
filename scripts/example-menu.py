#!/usr/bin/env python3
"""Minimal external launcher mode example for tili."""

import json
import sys


ENTRIES = {
    "shell": {"id": "shell", "label": "Open shell"},
    "top": {"id": "top", "label": "System monitor"},
}


def main(arguments: list[str]) -> int:
    if arguments == ["list"]:
        for entry in ENTRIES.values():
            print(json.dumps(entry, separators=(",", ":")))
        return 0

    if len(arguments) == 2 and arguments[0] == "select":
        commands = {
            "shell": {"action": "open_pane", "command": "/bin/sh", "args": []},
            "top": {"action": "open_pane", "command": "top", "args": []},
        }
        action = commands.get(arguments[1])
        if action is not None:
            print(json.dumps(action, separators=(",", ":")))
            return 0

    print("usage: example-menu.py list | select <id>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
