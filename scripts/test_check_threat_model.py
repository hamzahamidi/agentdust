import contextlib
import io
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_threat_model

SCRIPT = Path(__file__).resolve().with_name("check_threat_model.py")
SPEC_PATH = "docs/superpowers/specs/2026-10-03-agentdust-design.md"
MODEL_PATH = "docs/threat-model.md"
TEST_FILE = "crates/core/tests/safe_open.rs"

ADVERSARIES = [
    "Malicious model",
    "Malicious process metadata",
    "Buggy MCP client",
    "Same-user tampering",
    "PID reuse",
    "Concurrent apply",
    "Stale plans",
    "Compromised release artifact",
]

SPEC = """# Design

### 9.1 Safety requirements and their tests

| ID | Requirement | Tests |
| --- | --- | --- |
| S1 | first rule | one |
| S2 | second rule | two |

### 9.2 Layers

| S3 | a row outside section 9.1 | three |
"""

HEADER = "| Control | Spec | Requirement | Status | Tests |\n| --- | --- | --- | --- | --- |"
GOOD_ROWS = [
    ("Refuse symlinks", "S1", "implemented", f"`{TEST_FILE}`"),
    ("Typed code", "S2", "planned M3", "contract test with a wrong code"),
]


def table(rows):
    lines = [HEADER]
    for control, requirement, status, tests in rows:
        lines.append(f"| {control} | 7.3 | {requirement} | {status} | {tests} |")
    return "\n".join(lines)


def build_model(drop=(), rows=GOOD_ROWS, extra=""):
    parts = ["# Threat model", ""]
    for index, title in enumerate(ADVERSARIES, 1):
        if title in drop:
            continue
        parts += [f"## {index}. {title}", ""]
        if index == 1:
            parts += [table(rows), "", extra, ""]
    return "\n".join(parts)


class CheckerTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.base = Path(directory.name)
        self.root = self.base / "repo"
        (self.root / TEST_FILE).parent.mkdir(parents=True)
        (self.root / TEST_FILE).write_text("fn main() {}\n")

    def problems(self, model, spec=SPEC):
        return check_threat_model.check(model, spec, self.root)

    def only_problem(self, model, spec=SPEC):
        found = self.problems(model, spec)
        self.assertEqual(len(found), 1, found)
        return found[0]

    def test_a_complete_model_has_no_problems(self):
        self.assertEqual(self.problems(build_model()), [])

    def test_a_missing_adversary_heading_is_reported(self):
        problem = self.only_problem(build_model(drop=["Stale plans"]))
        self.assertIn("Stale plans", problem)

    def test_every_missing_heading_is_reported(self):
        found = self.problems(build_model(drop=["PID reuse", "Concurrent apply"]))
        self.assertEqual(len(found), 2, found)
        self.assertTrue(any("PID reuse" in p for p in found))
        self.assertTrue(any("Concurrent apply" in p for p in found))

    def test_a_heading_at_the_wrong_level_does_not_count(self):
        model = build_model().replace("## 7. Stale plans", "### 7. Stale plans")
        self.assertIn("Stale plans", self.only_problem(model))

    def test_a_heading_without_a_number_counts(self):
        model = build_model().replace("## 7. Stale plans", "## Stale plans")
        self.assertEqual(self.problems(model), [])

    def test_a_heading_inside_a_code_fence_does_not_count(self):
        model = build_model().replace("## 7. Stale plans", "```\n## 7. Stale plans\n```")
        self.assertIn("Stale plans", self.only_problem(model))

    def test_an_unknown_requirement_id_is_reported(self):
        problem = self.only_problem(build_model(extra="This also cites S99."))
        self.assertIn("S99", problem)

    def test_an_id_that_only_appears_outside_section_9_1_is_unknown(self):
        problem = self.only_problem(build_model(extra="This cites S3."))
        self.assertIn("S3", problem)

    def test_an_id_inside_a_code_fence_is_not_cited(self):
        model = build_model(extra="```\nS99\n```")
        self.assertEqual(self.problems(model), [])

    def test_an_uncovered_requirement_id_is_reported(self):
        rows = [GOOD_ROWS[0], ("Typed code", "none", "planned M3", "a planned test")]
        problem = self.only_problem(build_model(rows=rows))
        self.assertIn("S2", problem)

    def test_every_uncovered_requirement_id_is_reported(self):
        rows = [("Refuse symlinks", "none", "implemented", f"`{TEST_FILE}`")]
        found = self.problems(build_model(rows=rows))
        self.assertEqual(len(found), 2, found)

    def test_a_spec_without_requirement_ids_is_a_problem_and_not_a_pass(self):
        found = self.problems(build_model(), spec="# Design\n\n### 9.1 Safety\n\nnothing here\n")
        self.assertTrue(any("9.1" in p for p in found), found)

    def test_a_missing_test_file_in_an_implemented_row_is_reported(self):
        rows = [("Refuse symlinks", "S1", "implemented", "`crates/core/tests/gone.rs`"), GOOD_ROWS[1]]
        problem = self.only_problem(build_model(rows=rows))
        self.assertIn("crates/core/tests/gone.rs", problem)

    def test_every_path_of_an_implemented_row_is_checked(self):
        tests = f"`{TEST_FILE}` and `crates/core/tests/one.rs` and `crates/core/tests/two.rs`"
        rows = [("Refuse symlinks", "S1", "implemented", tests), GOOD_ROWS[1]]
        found = self.problems(build_model(rows=rows))
        self.assertEqual(len(found), 2, found)

    def test_a_planned_row_may_name_a_file_that_does_not_exist_yet(self):
        rows = [GOOD_ROWS[0], ("Typed code", "S2", "planned M3", "`crates/mcp/tests/apply.rs`")]
        self.assertEqual(self.problems(build_model(rows=rows)), [])

    def test_an_implemented_row_without_a_path_is_reported(self):
        rows = [("Refuse symlinks", "S1", "implemented", "tests exist somewhere"), GOOD_ROWS[1]]
        problem = self.only_problem(build_model(rows=rows))
        self.assertIn("implemented", problem)

    def test_an_unrecognised_status_is_reported(self):
        for status in ["done", "planned", "planned M", "implemented (M1)", "M1", ""]:
            with self.subTest(status=status):
                rows = [("Refuse symlinks", "S1", status, f"`{TEST_FILE}`"), GOOD_ROWS[1]]
                self.assertIn("status", self.only_problem(build_model(rows=rows)).lower())

    def test_a_row_with_no_status_cell_is_reported(self):
        model = build_model(extra="\n".join([HEADER, "| Short row | 6.3 | S2 |"]))
        self.assertIn("status", self.only_problem(model).lower())

    def test_a_table_without_a_status_column_is_not_checked(self):
        extra = "| Name | File |\n| --- | --- |\n| x | `docs/missing.md` |"
        self.assertEqual(self.problems(build_model(extra=extra)), [])

    def test_a_path_outside_the_repository_does_not_count(self):
        outside = self.base / "outside.rs"
        outside.write_text("fn main() {}\n")
        for token in ["../outside.rs", str(outside)]:
            with self.subTest(token=token):
                rows = [("Refuse symlinks", "S1", "implemented", f"`{token}`"), GOOD_ROWS[1]]
                self.assertIn(token, self.only_problem(build_model(rows=rows)))

    def test_a_status_cell_is_read_case_insensitively(self):
        rows = [("Refuse symlinks", "S1", "Implemented", f"`{TEST_FILE}`"), ("Typed code", "S2", "Planned M3", "x")]
        self.assertEqual(self.problems(build_model(rows=rows)), [])

    def test_every_problem_is_listed_at_once(self):
        rows = [("Refuse symlinks", "S1 S99", "implemented", "`crates/core/tests/gone.rs`")]
        found = self.problems(build_model(drop=["Stale plans"], rows=rows))
        for needle in ["Stale plans", "S99", "gone.rs", "S2"]:
            self.assertTrue(any(needle in p for p in found), (needle, found))
        self.assertEqual(len(found), 4, found)


class MainTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / TEST_FILE).parent.mkdir(parents=True)
        (self.root / TEST_FILE).write_text("fn main() {}\n")
        (self.root / SPEC_PATH).parent.mkdir(parents=True)
        (self.root / SPEC_PATH).write_text(SPEC)

    def write_model(self, text):
        (self.root / MODEL_PATH).parent.mkdir(parents=True, exist_ok=True)
        (self.root / MODEL_PATH).write_text(text)

    def run_script(self, *extra):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(self.root), *extra],
            capture_output=True,
            text=True,
        )

    def test_a_good_model_exits_zero_and_prints_nothing_on_stderr(self):
        self.write_model(build_model())
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")

    def test_a_bad_model_exits_one_and_lists_every_problem_with_the_file_name(self):
        rows = [("Refuse symlinks", "S1 S99", "implemented", "`crates/core/tests/gone.rs`")]
        self.write_model(build_model(drop=["Stale plans"], rows=rows))
        result = self.run_script()
        self.assertEqual(result.returncode, 1)
        lines = [line for line in result.stderr.splitlines() if line]
        self.assertEqual(len(lines), 4, result.stderr)
        self.assertTrue(all(MODEL_PATH in line for line in lines), result.stderr)

    def test_a_missing_model_file_is_a_problem_and_not_a_traceback(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 1)
        self.assertIn(MODEL_PATH, result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_a_missing_spec_file_is_a_problem_and_not_a_traceback(self):
        self.write_model(build_model())
        (self.root / SPEC_PATH).unlink()
        result = self.run_script()
        self.assertEqual(result.returncode, 1)
        self.assertIn("2026-10-03-agentdust-design.md", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_the_model_and_spec_paths_can_be_given(self):
        (self.root / "elsewhere").mkdir()
        (self.root / "elsewhere/model.md").write_text(build_model())
        (self.root / "elsewhere/spec.md").write_text(SPEC)
        result = self.run_script("--model", "elsewhere/model.md", "--spec", "elsewhere/spec.md")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_main_returns_the_exit_status_for_in_process_callers(self):
        self.write_model(build_model())
        captured = io.StringIO()
        with contextlib.redirect_stderr(captured):
            self.assertEqual(check_threat_model.main(["--root", str(self.root)]), 0)
            self.write_model(build_model(drop=["PID reuse"]))
            self.assertEqual(check_threat_model.main(["--root", str(self.root)]), 1)
        self.assertIn("PID reuse", captured.getvalue())


if __name__ == "__main__":
    unittest.main()
