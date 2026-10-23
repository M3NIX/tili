#!/usr/bin/env python3
"""List concrete SSH aliases or return a structured tili launcher action."""

from __future__ import annotations

import glob
import json
import os
from pathlib import Path
import shlex
import sys


def _tokens(line: str) -> list[str]:
    lexer = shlex.shlex(line, posix=True)
    lexer.whitespace_split = True
    lexer.commenters = "#"
    try:
        tokens = list(lexer)
    except ValueError:
        return []
    if tokens and "=" in tokens[0]:
        keyword, value = tokens[0].split("=", 1)
        tokens = [keyword, *([value] if value else []), *tokens[1:]]
    elif len(tokens) > 1 and tokens[1].startswith("="):
        value = tokens[1][1:]
        tokens = [tokens[0], *([value] if value else []), *tokens[2:]]
    return tokens


def _is_concrete(alias: str) -> bool:
    return bool(alias) and not alias.startswith("!") and not any(
        character in alias for character in "*?["
    )


def _read_config(path: Path, include_base: Path, visited: set[Path]) -> list[list[str]]:
    try:
        resolved = path.expanduser().resolve()
    except OSError:
        return []
    if resolved in visited:
        return []
    try:
        lines = resolved.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    visited.add(resolved)

    directives: list[list[str]] = []
    for line in lines:
        tokens = _tokens(line)
        if not tokens:
            continue
        keyword = tokens[0].lower()
        values = tokens[1:]
        if keyword in ("host", "hostname", "match"):
            directives.append([keyword, *values])
        elif keyword == "include":
            for pattern in values:
                expanded = Path(os.path.expanduser(pattern))
                if not expanded.is_absolute():
                    expanded = include_base / expanded
                for included in sorted(glob.glob(str(expanded))):
                    directives.extend(_read_config(Path(included), include_base, visited))
    visited.remove(resolved)
    return directives


def discover_hosts(config: Path | None = None) -> list[tuple[str, str | None]]:
    config = config or Path.home() / ".ssh" / "config"
    include_base = config.expanduser().parent
    hosts: dict[str, tuple[str, str | None]] = {}
    active: list[str] = []
    for keyword, *values in _read_config(config, include_base, set()):
        if keyword == "host":
            active = [alias.casefold() for alias in values if _is_concrete(alias)]
            for alias in values:
                if _is_concrete(alias):
                    hosts.setdefault(alias.casefold(), (alias, None))
        elif keyword == "match":
            active = []
        elif keyword == "hostname" and len(values) == 1:
            for key in active:
                alias, hostname = hosts[key]
                if hostname is None:
                    hosts[key] = (alias, values[0])
    return sorted(hosts.values(), key=lambda host: (host[0].casefold(), host[0]))


def list_hosts() -> int:
    for alias, hostname in discover_hosts():
        print(
            json.dumps(
                {"id": alias, "label": f"{alias} ({hostname})" if hostname else alias},
                ensure_ascii=False,
                separators=(",", ":"),
            )
        )
    return 0


def select_host(alias: str) -> int:
    print(
        json.dumps(
            {"action": "open_pane", "command": "ssh", "args": [alias]},
            ensure_ascii=False,
            separators=(",", ":"),
        )
    )
    return 0


def main(argv: list[str]) -> int:
    if argv == ["list"]:
        return list_hosts()
    if len(argv) == 2 and argv[0] == "select":
        return select_host(argv[1])
    print("usage: ssh-hosts.py list | ssh-hosts.py select <entry-id>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
