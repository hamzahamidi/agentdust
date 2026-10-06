#!/usr/bin/env python3
"""Prepare a private, temporary Claude Code configuration for M6 capture."""

from __future__ import annotations

import json
import os
import secrets
import shlex
import sys
import tempfile
import shutil
from pathlib import Path
from typing import Any


HOOKS = {
    "SessionStart": None,
    "SessionEnd": None,
    "SubagentStart": None,
    "SubagentStop": None,
    "PreToolUse": "Bash",
    "PostToolUse": "Bash",
}


def _write_private(path: Path, content: bytes) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        view = memoryview(content)
        while view:
            written = os.write(fd, view)
            if written == 0:
                raise OSError("short write")
            view = view[written:]
        os.fsync(fd)
    finally:
        os.close(fd)


def prepare(agentdust: Path, base_dir: Path | None = None) -> Path:
    agentdust = Path(os.path.abspath(agentdust))
    if not agentdust.is_file() or not os.access(agentdust, os.X_OK):
        raise ValueError("agentdust must be an absolute executable file")
    capture_script = Path(__file__).resolve().with_name("capture_claude_fixture.py")
    if not capture_script.is_file():
        raise ValueError("capture helper is missing")

    directory = Path(tempfile.mkdtemp(prefix="agentdust-m6-", dir=base_dir))
    os.chmod(directory, 0o700)
    try:
        data_dir = directory / "data"
        data_dir.mkdir(mode=0o700)
        key_path = directory / "capture-key"
        key = secrets.token_hex(32).encode("ascii") + b"\n"
        _write_private(key_path, key)

        capture_path = directory / "events.jsonl"
        capture_command = " ".join(
            shlex.quote(value)
            for value in (sys.executable, str(capture_script))
        )
        agentdust_command = f"{shlex.quote(str(agentdust))} hook claude"
        handlers = [
            {"type": "command", "command": agentdust_command},
            {"type": "command", "command": capture_command},
        ]
        hooks: dict[str, Any] = {}
        for event, matcher in HOOKS.items():
            group: dict[str, Any] = {"hooks": handlers}
            if matcher is not None:
                group["matcher"] = matcher
            hooks[event] = [group]
        settings = {"hooks": hooks}
        mcp = {
            "mcpServers": {
                "agentdust": {
                    "command": str(agentdust),
                    "args": ["mcp"],
                    "env": {"AGENTDUST_DATA_DIR": str(data_dir)},
                }
            }
        }
        environment = {
            "AGENTDUST_DATA_DIR": str(data_dir),
            "AGENTDUST_FIXTURE_CAPTURE_PATH": str(capture_path),
            "AGENTDUST_FIXTURE_KEY_FILE": str(key_path),
        }
        _write_private(directory / "settings.json", (json.dumps(settings, indent=2) + "\n").encode())
        _write_private(directory / "mcp.json", (json.dumps(mcp, indent=2) + "\n").encode())
        env_text = "".join(f"export {name}={shlex.quote(value)}\n" for name, value in environment.items())
        _write_private(directory / "capture.env", env_text.encode())
        return directory
    except BaseException:
        shutil.rmtree(directory)
        raise


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: prepare_m6_capture.py /absolute/path/to/agentdust", file=sys.stderr)
        return 2
    try:
        directory = prepare(Path(sys.argv[1]))
    except (OSError, ValueError) as error:
        print(f"prepare M6 capture: {error}", file=sys.stderr)
        return 1
    print(directory)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
