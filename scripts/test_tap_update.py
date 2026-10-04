import contextlib
import io
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import tap_update

SCRIPTS = Path(__file__).resolve().parent
FORMULA = "Formula/agentdust.rb"


def ruby_accepts(path):
    return subprocess.run(["ruby", "-c", str(path)], capture_output=True, text=True)


class TapUpdateTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name)
        self.tap = self.path / "tap"
        self.tap.mkdir()

    def make_formula(self, version, name=None):
        tarball = self.path / f"agentdust-{version}.tar.gz"
        tarball.write_bytes(f"tarball of {version}".encode())
        out = self.path / (name or f"agentdust-{version}.rb")
        subprocess.run(
            [
                sys.executable,
                str(SCRIPTS / "formula.py"),
                "--tarball", str(tarball),
                "--version", version,
                "--url", f"https://example.invalid/agentdust-{version}-darwin-arm64.tar.gz",
                "--out", str(out),
            ],
            check=True,
            capture_output=True,
        )
        return out

    def update(self, version):
        return tap_update.update_tap(self.tap, self.make_formula(version), version)

    def files(self):
        return sorted(str(path.relative_to(self.tap)) for path in self.tap.rglob("*") if path.is_file())

    def read(self, relative):
        return (self.tap / relative).read_text(encoding="utf-8")

    def test_the_first_release_creates_the_formula(self):
        written = self.update("0.1.0")
        self.assertEqual(written, [FORMULA])
        self.assertEqual(self.files(), [FORMULA])
        self.assertEqual(self.read(FORMULA), self.make_formula("0.1.0").read_text(encoding="utf-8"))

    def test_a_patch_release_replaces_the_formula_and_adds_no_versioned_one(self):
        self.update("0.1.0")
        written = self.update("0.1.1")
        self.assertEqual(written, [FORMULA])
        self.assertEqual(self.files(), [FORMULA])
        self.assertIn('version "0.1.1"', self.read(FORMULA))

    def test_a_new_minor_keeps_the_previous_minor_as_a_versioned_formula(self):
        self.update("0.1.2")
        old = self.read(FORMULA)
        written = self.update("0.2.0")
        self.assertEqual(written, ["Formula/agentdust@0.1.rb", FORMULA])
        self.assertEqual(self.files(), ["Formula/agentdust.rb", "Formula/agentdust@0.1.rb"])
        versioned = self.read("Formula/agentdust@0.1.rb")
        self.assertIn("class AgentdustAT01 < Formula", versioned)
        self.assertNotIn("class Agentdust < Formula", versioned)
        self.assertEqual(versioned.replace("class AgentdustAT01 < Formula", "class Agentdust < Formula"), old)
        self.assertIn('version "0.1.2"', versioned)
        self.assertIn('version "0.2.0"', self.read(FORMULA))
        self.assertIn("class Agentdust < Formula", self.read(FORMULA))

    def test_every_formula_that_is_written_is_valid_ruby(self):
        self.update("0.1.0")
        self.update("0.2.0")
        self.update("0.3.0")
        self.assertEqual(len(self.files()), 3)
        for relative in self.files():
            with self.subTest(file=relative):
                result = ruby_accepts(self.tap / relative)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_older_minors_stay_as_they_were(self):
        self.update("0.1.0")
        self.update("0.2.0")
        first = self.read("Formula/agentdust@0.1.rb")
        self.update("0.2.4")
        self.update("0.3.0")
        self.assertEqual(
            self.files(), ["Formula/agentdust.rb", "Formula/agentdust@0.1.rb", "Formula/agentdust@0.2.rb"]
        )
        self.assertEqual(self.read("Formula/agentdust@0.1.rb"), first)
        self.assertIn('version "0.2.4"', self.read("Formula/agentdust@0.2.rb"))

    def test_the_class_name_follows_the_homebrew_rule_for_versioned_formulas(self):
        self.assertEqual(tap_update.class_name("0.1"), "AgentdustAT01")
        self.assertEqual(tap_update.class_name("1.10"), "AgentdustAT110")
        self.assertEqual(tap_update.class_name("12.3"), "AgentdustAT123")

    def test_a_new_major_keeps_the_last_minor_of_the_old_major(self):
        self.update("0.9.1")
        self.update("1.0.0")
        self.assertIn("class AgentdustAT09 < Formula", self.read("Formula/agentdust@0.9.rb"))

    def test_a_minor_with_two_digits_is_ordered_as_a_number(self):
        self.update("1.9.0")
        self.update("1.10.0")
        self.assertEqual(self.files(), ["Formula/agentdust.rb", "Formula/agentdust@1.9.rb"])
        with self.assertRaises(tap_update.TapError):
            self.update("1.9.5")

    def test_a_version_older_than_the_formula_in_the_tap_is_refused_and_nothing_changes(self):
        self.update("0.2.0")
        before = self.files(), self.read(FORMULA)
        with self.assertRaises(tap_update.TapError) as caught:
            self.update("0.1.3")
        self.assertIn("0.1.3", str(caught.exception))
        self.assertIn("0.2.0", str(caught.exception))
        self.assertEqual((self.files(), self.read(FORMULA)), before)

    def test_the_same_version_again_with_the_same_content_changes_nothing(self):
        self.update("0.1.0")
        before = self.read(FORMULA)
        self.update("0.1.0")
        self.assertEqual((self.files(), self.read(FORMULA)), ([FORMULA], before))

    def test_the_same_version_with_another_digest_is_refused(self):
        self.update("0.1.0")
        before = self.read(FORMULA)
        other = self.make_formula("0.1.0", "other.rb")
        other.write_text(re.sub(r'sha256 "[0-9a-f]{64}"', 'sha256 "' + "0" * 64 + '"', other.read_text()))
        with self.assertRaises(tap_update.TapError) as caught:
            tap_update.update_tap(self.tap, other, "0.1.0")
        self.assertIn("0.1.0", str(caught.exception))
        self.assertEqual(self.read(FORMULA), before)

    def test_a_formula_whose_version_differs_from_the_one_given_is_refused(self):
        formula = self.make_formula("0.1.0")
        with self.assertRaises(tap_update.TapError):
            tap_update.update_tap(self.tap, formula, "0.2.0")
        self.assertEqual(self.files(), [])
        self.assertFalse((self.tap / "Formula").exists())

    def test_a_malformed_version_is_refused(self):
        formula = self.make_formula("0.1.0")
        for version in ("0.1", "v0.1.0", "0.1.0-rc1", ""):
            with self.subTest(version=version):
                with self.assertRaises(tap_update.TapError):
                    tap_update.update_tap(self.tap, formula, version)
        self.assertEqual(self.files(), [])

    def test_a_formula_in_the_tap_without_a_version_is_not_overwritten(self):
        (self.tap / "Formula").mkdir()
        (self.tap / FORMULA).write_text("class Agentdust < Formula\nend\n", encoding="utf-8")
        with self.assertRaises(tap_update.TapError):
            self.update("0.2.0")
        self.assertEqual(self.read(FORMULA), "class Agentdust < Formula\nend\n")
        self.assertEqual(self.files(), [FORMULA])

    def test_a_new_formula_that_is_not_the_agentdust_class_is_refused(self):
        formula = self.path / "wrong.rb"
        formula.write_text('class Other < Formula\n  version "0.1.0"\nend\n', encoding="utf-8")
        with self.assertRaises(tap_update.TapError):
            tap_update.update_tap(self.tap, formula, "0.1.0")
        self.assertEqual(self.files(), [])

    def test_an_existing_versioned_formula_with_other_content_is_not_overwritten(self):
        self.update("0.1.0")
        (self.tap / "Formula/agentdust@0.1.rb").write_text("hand edited\n", encoding="utf-8")
        with self.assertRaises(tap_update.TapError) as caught:
            self.update("0.2.0")
        self.assertIn("agentdust@0.1.rb", str(caught.exception))
        self.assertEqual(self.read("Formula/agentdust@0.1.rb"), "hand edited\n")
        self.assertIn('version "0.1.0"', self.read(FORMULA))

    def test_a_missing_formula_file_is_refused(self):
        with self.assertRaises(tap_update.TapError):
            tap_update.update_tap(self.tap, self.path / "none.rb", "0.1.0")

    def test_the_command_line_prints_the_files_it_wrote(self):
        formula = self.make_formula("0.1.0")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = tap_update.main(["--tap", str(self.tap), "--formula", str(formula), "--version", "0.1.0"])
        self.assertEqual((code, out.getvalue(), err.getvalue()), (0, FORMULA + "\n", ""))

    def test_the_command_line_exits_1_with_a_message_and_changes_nothing_on_refusal(self):
        formula = self.make_formula("0.1.0")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = tap_update.main(["--tap", str(self.tap), "--formula", str(formula), "--version", "0.9.9"])
        self.assertEqual((code, out.getvalue()), (1, ""))
        self.assertIn("0.9.9", err.getvalue())
        self.assertEqual(self.files(), [])

    def test_the_script_runs_as_a_program(self):
        formula = self.make_formula("0.1.0")
        result = subprocess.run(
            [sys.executable, str(SCRIPTS / "tap_update.py"), "--tap", str(self.tap), "--formula", str(formula), "--version", "0.1.0"],
            capture_output=True,
            text=True,
        )
        self.assertEqual((result.returncode, result.stdout), (0, FORMULA + "\n"))
        self.assertEqual(self.files(), [FORMULA])


if __name__ == "__main__":
    unittest.main()
