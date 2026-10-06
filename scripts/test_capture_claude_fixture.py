import json
import os
import runpy
import stat
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

from capture_claude_fixture import _append_private, _read_key, main, normalise


KEY = b"temporary fixture key with at least 32 bytes"


class CaptureClaudeFixtureTest(unittest.TestCase):
    def test_records_lifecycle_shape_and_discards_private_payload_fields(self):
        payload = {
            "hook_event_name": "SubagentStop",
            "session_id": "session-secret",
            "agent_id": "agent-secret",
            "agent_type": "private-agent-name",
            "agent_transcript_path": "/private/path/transcript.jsonl",
            "transcript_path": "/private/path/main.jsonl",
            "cwd": "/private/project",
            "last_assistant_message": "private final response",
            "stop_hook_active": True,
            "unknown_field": "unknown-secret",
        }

        result = normalise(json.dumps(payload).encode(), KEY)

        self.assertEqual(result["hook_event_name"], "SubagentStop")
        self.assertTrue(result["agent_type_present"])
        self.assertTrue(result["stop_hook_active"])
        encoded = json.dumps(result)
        for private_value in (
            "session-secret",
            "agent-secret",
            "private-agent-name",
            "/private/path/transcript.jsonl",
            "/private/path/main.jsonl",
            "/private/project",
            "private final response",
            "unknown-secret",
        ):
            self.assertNotIn(private_value, encoded)
        self.assertNotIn("agent_transcript_path", result)
        self.assertNotIn("last_assistant_message", result)

    def test_ids_are_stable_only_within_the_ephemeral_key(self):
        payload = b'{"hook_event_name":"SubagentStart","session_id":"s1","agent_id":"a1"}'
        first = normalise(payload, KEY)
        second = normalise(payload, KEY)
        other_key = normalise(payload, b"another temporary fixture key 123456")

        self.assertEqual(first, second)
        self.assertNotEqual(first["session_id"], other_key["session_id"])
        self.assertNotEqual(first["agent_id"], other_key["agent_id"])
        self.assertNotIn("s1", json.dumps(first))
        self.assertNotIn("a1", json.dumps(first))

    def test_ignores_unknown_malformed_oversized_and_duplicate_key_events(self):
        for payload in (
            b"not json",
            b'{"hook_event_name":"Unknown"}',
            b'{"hook_event_name":"SessionStart","session_id":"a","session_id":"b"}',
            b"x" * (64 * 1024 + 1),
        ):
            with self.subTest(payload=payload[:30]):
                self.assertIsNone(normalise(payload, KEY))

    def test_deep_json_and_recursion_errors_are_ignored(self):
        payload = (
            b'{"hook_event_name":"SessionStart","unknown":'
            + b"[" * 1100
            + b"0"
            + b"]" * 1100
            + b"}"
        )
        self.assertLess(len(payload), 64 * 1024)
        self.assertEqual(
            normalise(payload, KEY),
            {"hook_event_name": "SessionStart", "session_id": None},
        )
        with patch("capture_claude_fixture.json.loads", side_effect=RecursionError):
            self.assertIsNone(normalise(payload, KEY))

        with patch("capture_claude_fixture._read_bounded_stdin", side_effect=RecursionError):
            self.assertEqual(main(), 0)

    def test_private_capture_file_is_created_with_mode_0600(self):
        from capture_claude_fixture import _append_private

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "events.jsonl"
            _append_private(str(output), {"hook_event_name": "SessionStart"})
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)
            self.assertEqual(json.loads(output.read_text()), {"hook_event_name": "SessionStart"})

    def test_refuses_a_hard_link_as_capture_output(self):
        with tempfile.TemporaryDirectory() as directory:
            original = Path(directory) / "original.jsonl"
            alias = Path(directory) / "alias.jsonl"
            original.write_text("private data\n")
            original.chmod(0o600)
            os.link(original, alias)
            _append_private(str(alias), {"hook_event_name": "SessionStart"})
            self.assertEqual(original.read_text(), "private data\n")

    def test_rejects_fifo_key_and_output_without_blocking(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key_fifo = root / "capture-key"
            output_fifo = root / "events.jsonl"
            os.mkfifo(key_fifo, 0o600)
            os.mkfifo(output_fifo, 0o600)

            self.assertIsNone(_read_key(str(key_fifo)))
            with self.assertRaises(OSError):
                _append_private(str(output_fifo), {"hook_event_name": "SessionStart"})

    def test_prepare_m6_capture_creates_private_hooks_and_ephemeral_key(self):
        prepare_module = runpy.run_path(str(Path(__file__).with_name("prepare_m6_capture.py")))
        prepare = prepare_module["prepare"]
        with tempfile.TemporaryDirectory() as parent:
            root = Path(parent)
            binary = root / "agentdust"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o700)
            directory = prepare(binary, root)

            self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)
            for filename in ("capture-key", "settings.json", "mcp.json", "capture.env"):
                self.assertEqual(stat.S_IMODE((directory / filename).stat().st_mode), 0o600)
            key = (directory / "capture-key").read_text().strip()
            self.assertEqual(len(key), 64)
            self.assertNotIn(key, (directory / "capture.env").read_text())

            settings = json.loads((directory / "settings.json").read_text())
            self.assertEqual(
                set(settings["hooks"]),
                {"SessionStart", "SessionEnd", "SubagentStart", "SubagentStop", "PreToolUse", "PostToolUse"},
            )
            mcp = json.loads((directory / "mcp.json").read_text())
            self.assertEqual(mcp["mcpServers"]["agentdust"]["args"], ["mcp"])


if __name__ == "__main__":
    unittest.main()
