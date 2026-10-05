import hashlib
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().with_name("formula.py")
URL = "https://github.com/hamzahamidi/agentdust/releases/download/v0.1.0/agentdust-0.1.0-darwin-arm64.tar.gz"


class FormulaTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name)
        self.tarball = self.path / "agentdust-0.1.0-darwin-arm64.tar.gz"
        self.tarball.write_bytes(b"release tarball bytes")

    def write(self, out="out/agentdust.rb", url=URL, version="0.1.0"):
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--tarball", str(self.tarball),
                "--version", version,
                "--url", url,
                "--out", str(self.path / out),
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.path / out, result.stdout

    def test_the_formula_pins_the_url_the_version_and_the_digest_of_the_tarball(self):
        formula, _ = self.write()
        text = formula.read_text()
        self.assertIn(f'url "{URL}"', text)
        self.assertIn('version "0.1.0"', text)
        self.assertIn(f'sha256 "{hashlib.sha256(self.tarball.read_bytes()).hexdigest()}"', text)

    def test_the_formula_installs_one_binary_for_apple_silicon_macos(self):
        text = self.write()[0].read_text()
        self.assertIn('bin.install "agentdust"', text)
        self.assertEqual(len(re.findall(r"bin\.install", text)), 1)
        self.assertIn("depends_on arch: :arm64", text)
        self.assertIn("depends_on :macos", text)
        self.assertIn("class Agentdust < Formula", text)

    def test_the_formula_test_runs_the_installed_binary_and_expects_its_version_line(self):
        text = self.write()[0].read_text()
        self.assertIn('assert_match "agentdust #{version}", shell_output("#{bin}/agentdust version")', text)

    def test_the_formula_is_valid_ruby(self):
        result = subprocess.run(["ruby", "-c", str(self.write()[0])], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_the_same_inputs_give_the_same_formula(self):
        first, _ = self.write("one.rb")
        second, _ = self.write("two.rb")
        self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_a_changed_tarball_changes_the_digest_in_the_formula(self):
        first = self.write("one.rb")[0].read_text()
        self.tarball.write_bytes(b"other bytes")
        second = self.write("two.rb")[0].read_text()
        self.assertNotEqual(first, second)

    def test_the_output_directory_is_created_and_its_path_is_printed(self):
        formula, out = self.write("deep/er/agentdust.rb")
        self.assertTrue(formula.is_file())
        self.assertEqual(out.strip(), str(formula))


if __name__ == "__main__":
    unittest.main()
