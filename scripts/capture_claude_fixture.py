#!/usr/bin/env python3
"""Store a privacy-filtered Claude Code hook event for M6 fixture capture."""

from __future__ import annotations

import fcntl
import hashlib
import hmac
import json
import os
import stat
import sys
from pathlib import Path
from typing import Any


MAX_INPUT_BYTES = 64 * 1024
MAX_OUTPUT_BYTES = 8 * 1024 * 1024
ALLOWED_EVENTS = {
    "SessionStart",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
    "PreToolUse",
    "PostToolUse",
}
ALLOWED_SOURCES = {"startup", "resume", "clear", "compact", "fork"}
ALLOWED_REASONS = {"clear", "resume", "logout", "prompt_input_exit"}
ALLOWED_TOOLS = {"Bash", "Agent", "Task"}


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def _label(value: Any, namespace: str, key: bytes) -> str | None:
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > 256:
        return None
    message = namespace.encode("ascii") + b"\0" + value.encode("utf-8")
    digest = hmac.new(key, message, hashlib.sha256).hexdigest()[:16]
    return f"{namespace}-{digest}"


def normalise(payload: bytes, key: bytes) -> dict[str, Any] | None:
    """Return an allowlisted record. Values outside the allowlist are discarded."""
    if len(payload) > MAX_INPUT_BYTES:
        return None
    try:
        event = json.loads(payload, object_pairs_hook=_object_without_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError, ValueError):
        return None
    if not isinstance(event, dict):
        return None

    name = event.get("hook_event_name")
    if not isinstance(name, str) or name not in ALLOWED_EVENTS:
        return None

    record: dict[str, Any] = {
        "hook_event_name": name,
        "session_id": _label(event.get("session_id"), "session", key),
    }
    agent_ref = _label(event.get("agent_id"), "agent", key)
    tool_ref = _label(event.get("tool_use_id"), "tool", key)
    if agent_ref is not None:
        record["agent_id"] = agent_ref
    if tool_ref is not None:
        record["tool_use_id"] = tool_ref

    source = event.get("source")
    if isinstance(source, str):
        record["source"] = source if source in ALLOWED_SOURCES else "other"
    reason = event.get("reason")
    if isinstance(reason, str):
        record["reason"] = reason if reason in ALLOWED_REASONS else "other"
    tool_name = event.get("tool_name")
    if isinstance(tool_name, str):
        record["tool_name"] = tool_name if tool_name in ALLOWED_TOOLS else "other"
    if "agent_type" in event:
        record["agent_type_present"] = isinstance(event["agent_type"], str)
    if isinstance(event.get("stop_hook_active"), bool):
        record["stop_hook_active"] = event["stop_hook_active"]
    return record


def _read_key(path: str) -> bytes | None:
    flags = os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError:
        return None
    try:
        info = os.fstat(fd)
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_uid != os.geteuid()
            or info.st_mode & 0o077
            or info.st_nlink != 1
        ):
            return None
        key = os.read(fd, 4096).strip()
        return key if len(key) >= 32 else None
    finally:
        os.close(fd)


def _append_private(path: str, record: dict[str, Any]) -> None:
    destination = Path(path)
    parent = destination.parent
    directory = parent.stat(follow_symlinks=False)
    if (
        not stat.S_ISDIR(directory.st_mode)
        or directory.st_uid != os.geteuid()
        or directory.st_mode & 0o077
    ):
        return

    flags = (
        os.O_WRONLY
        | os.O_APPEND
        | os.O_CREAT
        | os.O_NONBLOCK
        | getattr(os, "O_NOFOLLOW", 0)
    )
    fd = os.open(destination, flags, 0o600)
    try:
        info = os.fstat(fd)
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_uid != os.geteuid()
            or info.st_mode & 0o077
            or info.st_nlink != 1
            or info.st_size > MAX_OUTPUT_BYTES
        ):
            return
        encoded = json.dumps(record, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n"
        fcntl.flock(fd, fcntl.LOCK_EX)
        if os.fstat(fd).st_size + len(encoded) <= MAX_OUTPUT_BYTES:
            view = memoryview(encoded)
            while view:
                written = os.write(fd, view)
                if written == 0:
                    return
                view = view[written:]
    finally:
        os.close(fd)


def _read_bounded_stdin() -> bytes | None:
    content = bytearray()
    oversized = False
    while True:
        chunk = sys.stdin.buffer.read(8192)
        if not chunk:
            break
        if len(content) + len(chunk) > MAX_INPUT_BYTES:
            oversized = True
        elif not oversized:
            content.extend(chunk)
    return None if oversized else bytes(content)


def main() -> int:
    try:
        payload = _read_bounded_stdin()
        output_path = os.environ.get("AGENTDUST_FIXTURE_CAPTURE_PATH")
        key_path = os.environ.get("AGENTDUST_FIXTURE_KEY_FILE")
        if (
            payload is None
            or not output_path
            or not key_path
            or not Path(output_path).is_absolute()
            or not Path(key_path).is_absolute()
        ):
            return 0
        key = _read_key(key_path)
        if key is None:
            return 0
        record = normalise(payload, key)
        if record is not None:
            _append_private(output_path, record)
    except (OSError, RecursionError, ValueError, TypeError):
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
