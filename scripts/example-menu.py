#!/usr/bin/env python3
"""Minimal external launcher mode example for tili."""

import json
import sys

MENU = {
    "shell": {
        "label": "Open shell",
        "action": {"action": "open_pane", "command": "/bin/bash", "args": []},
    },
    "top": {
        "label": "System monitor",
        "action": {"action": "open_pane", "command": "top", "args": []},
    },
}


def main(arguments: list[str]) -> int:
    if arguments == ["list"]:
        for entry_id, entry in MENU.items():
            entry = {"id": entry_id, "label": entry["label"]}
            print(json.dumps(entry, separators=(",", ":")))
        return 0

    if len(arguments) == 2 and arguments[0] == "select":
        entry = MENU.get(arguments[1])
        if entry is not None:
            print(json.dumps(entry["action"], separators=(",", ":")))
            return 0

    print("usage: example-menu.py list | select <id>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
