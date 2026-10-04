import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().with_name("toolchain.py")
LOCK = b"# a lock file\n"
ENVIRONMENT = {"ImageOS": "macos15", "ImageVersion": "20260907.0337.1", "RUSTFLAGS": ""}


class ToolchainTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name)
        (self.path / "Cargo.lock").write_bytes(LOCK)
        self.expected = self.path / "expected.json"

    def run_script(self, *args, **overrides):
        env = {**os.environ, **ENVIRONMENT, **overrides}
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args], cwd=self.path, env=env, capture_output=True, text=True
        )

    def recorded(self, **overrides):
        result = self.run_script("record", **overrides)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def write_expected(self, **changes):
        record = {**self.recorded(), **changes}
        self.expected.write_text(json.dumps(record), encoding="utf-8")

    def check(self, *args, **overrides):
        return self.run_script("check", "--expected", str(self.expected), *args, **overrides)

    def test_the_record_holds_the_lock_hash_and_the_runner_image(self):
        record = self.recorded()
        self.assertEqual(record["cargo_lock_sha256"], hashlib.sha256(LOCK).hexdigest())
        self.assertEqual(record["runner_image"], "macos15 20260907.0337.1")

    def test_a_matching_record_passes(self):
        self.write_expected()
        result = self.check()
        self.assertEqual((result.returncode, result.stderr), (0, ""))

    def test_a_changed_runner_image_is_drift_and_the_message_names_both_values(self):
        self.write_expected()
        result = self.check(ImageVersion="20261001.0100.1")
        self.assertEqual(result.returncode, 1)
        self.assertIn("toolchain drift in runner_image", result.stderr)
        self.assertIn("macos15 20260907.0337.1", result.stderr)
        self.assertIn("macos15 20261001.0100.1", result.stderr)

    def test_each_recorded_toolchain_value_but_the_lock_hash_is_compared(self):
        for key in ("runner_image", "rustc", "cargo", "developer_dir", "xcode", "sdk_version", "sdk_path", "rustflags"):
            with self.subTest(key=key):
                self.write_expected(**{key: "something else"})
                result = self.check()
                self.assertEqual(result.returncode, 1)
                self.assertIn(f"toolchain drift in {key}", result.stderr)

    def test_every_drifted_key_is_reported_in_one_run(self):
        self.write_expected(xcode="Xcode 1.0", sdk_version="1.0")
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("toolchain drift in xcode", result.stderr)
        self.assertIn("toolchain drift in sdk_version", result.stderr)

    def test_the_lock_hash_is_not_compared_unless_asked(self):
        self.write_expected(cargo_lock_sha256="0" * 64)
        result = self.check()
        self.assertEqual((result.returncode, result.stderr), (0, ""))

    def test_a_different_lock_hash_is_drift_when_asked(self):
        self.write_expected(cargo_lock_sha256="0" * 64)
        result = self.check("--include-lock")
        self.assertEqual(result.returncode, 1)
        self.assertIn("toolchain drift in cargo_lock_sha256", result.stderr)
        self.assertIn("0" * 64, result.stderr)
        self.assertIn(hashlib.sha256(LOCK).hexdigest(), result.stderr)

    def test_an_edited_lock_file_is_drift_when_asked(self):
        self.write_expected()
        (self.path / "Cargo.lock").write_bytes(LOCK + b"# one more line\n")
        self.assertEqual(self.check().returncode, 0)
        result = self.check("--include-lock")
        self.assertEqual(result.returncode, 1)
        self.assertIn("cargo_lock_sha256", result.stderr)

    def test_a_matching_lock_hash_passes_when_asked(self):
        self.write_expected()
        result = self.check("--include-lock")
        self.assertEqual((result.returncode, result.stderr), (0, ""))

    def test_a_record_without_a_lock_hash_cannot_pass_when_the_hash_is_asked_for(self):
        record = self.recorded()
        del record["cargo_lock_sha256"]
        self.expected.write_text(json.dumps(record), encoding="utf-8")
        self.assertEqual(self.check().returncode, 0)
        result = self.check("--include-lock")
        self.assertEqual(result.returncode, 1)
        self.assertIn("toolchain drift in cargo_lock_sha256", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_a_missing_expected_file_fails(self):
        result = self.check()
        self.assertNotEqual(result.returncode, 0)

    def test_a_missing_expected_file_is_accepted_only_when_allowed(self):
        result = self.check("--allow-missing")
        self.assertEqual(result.returncode, 0)
        self.assertIn("nothing to compare", result.stdout)

    def test_allowing_a_missing_file_does_not_hide_drift_in_a_file_that_exists(self):
        self.write_expected(xcode="Xcode 1.0")
        result = self.check("--allow-missing")
        self.assertEqual(result.returncode, 1)

    def test_a_missing_lock_file_is_an_error_for_record(self):
        (self.path / "Cargo.lock").unlink()
        self.assertNotEqual(self.run_script("record").returncode, 0)


if __name__ == "__main__":
    unittest.main()
