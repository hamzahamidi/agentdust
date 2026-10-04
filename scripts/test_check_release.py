import contextlib
import io
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_release

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check_release.py"
COMMIT = "a" * 40

CARGO_TOML = """[workspace]
members = ["crates/one"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
serde = { version = "1.0.229" }
"""


def run_main(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        try:
            code = check_release.main(list(argv))
        except SystemExit as exit_:
            code = exit_.code
    return code, out.getvalue(), err.getvalue()


def git(repo, *args):
    result = subprocess.run(
        [
            "git",
            "-c", "user.name=Test",
            "-c", "user.email=test@example.invalid",
            "-c", "commit.gpgsign=false",
            "-c", "tag.gpgsign=false",
            *args,
        ],
        cwd=repo,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def commit(repo, name):
    (Path(repo) / name).write_text(name)
    git(repo, "add", name)
    git(repo, "commit", "-m", name)
    return git(repo, "rev-parse", "HEAD")


def check_run(name, conclusion="success", status="completed", run_id=1, app="github-actions", sha=COMMIT):
    return {
        "id": run_id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "head_sha": sha,
        "app": {"slug": app},
    }


def page(*runs):
    return json.dumps({"total_count": len(runs), "check_runs": list(runs)})


GREEN = page(check_run("linux", run_id=1), check_run("macos", run_id=2))


class TagTest(unittest.TestCase):
    def test_a_release_tag_is_v_and_three_numbers(self):
        for tag, version in (("v0.1.0", "0.1.0"), ("v1.20.3", "1.20.3"), ("v10.0.0", "10.0.0")):
            with self.subTest(tag=tag):
                self.assertEqual(check_release.tag_version(tag), version)

    def test_anything_else_is_refused(self):
        for tag in (
            "",
            "0.1.0",
            "V0.1.0",
            "v0.1",
            "v0.1.0.1",
            "v0.1.0-rc1",
            "v0.1.0+build",
            "v01.2.3",
            "v1.02.3",
            "v1.2.03",
            " v0.1.0",
            "v0.1.0 ",
            "v0.1.0\n",
            "refs/tags/v0.1.0",
            "vv0.1.0",
            "v-1.0.0",
        ):
            with self.subTest(tag=tag):
                with self.assertRaises(check_release.ReleaseError):
                    check_release.tag_version(tag)

    def test_the_crate_version_is_the_workspace_package_version(self):
        self.assertEqual(check_release.crate_version(CARGO_TOML), "0.1.0")

    def test_a_dependency_version_before_the_package_table_is_not_the_crate_version(self):
        text = '[workspace.dependencies]\nserde = { version = "9.9.9" }\n\n[workspace.package]\nversion = "0.2.0"\n'
        self.assertEqual(check_release.crate_version(text), "0.2.0")

    def test_a_manifest_without_a_workspace_version_is_refused(self):
        for text in ("[workspace]\nmembers = []\n", '[package]\nname = "x"\nversion = "0.1.0"\n', "not toml ["):
            with self.subTest(text=text):
                with self.assertRaises(check_release.ReleaseError):
                    check_release.crate_version(text)

    def test_a_tag_equal_to_the_crate_version_passes(self):
        self.assertEqual(check_release.check_tag("v0.1.0", CARGO_TOML), "0.1.0")

    def test_a_tag_that_differs_from_the_crate_version_fails_and_names_both(self):
        for tag in ("v0.1.1", "v0.2.0", "v1.0.0", "v0.0.0"):
            with self.subTest(tag=tag):
                with self.assertRaises(check_release.ReleaseError) as caught:
                    check_release.check_tag(tag, CARGO_TOML)
                self.assertIn(tag, str(caught.exception))
                self.assertIn("0.1.0", str(caught.exception))

    def test_a_malformed_tag_fails_before_the_version_is_compared(self):
        with self.assertRaises(check_release.ReleaseError) as caught:
            check_release.check_tag("v0.1.0-rc1", CARGO_TOML)
        self.assertIn("v0.1.0-rc1", str(caught.exception))

    def test_the_repository_manifest_has_a_version_a_tag_can_name(self):
        version = check_release.crate_version((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        self.assertEqual(check_release.tag_version(f"v{version}"), version)


class ReachabilityTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.repo = self.dir.name
        git(self.repo, "init", "-b", "main")
        self.first = commit(self.repo, "one")
        self.second = commit(self.repo, "two")

    def test_a_lightweight_tag_on_main_resolves_to_its_commit(self):
        git(self.repo, "tag", "v0.1.0", self.second)
        self.assertEqual(check_release.tag_commit("v0.1.0", self.repo), self.second)

    def test_an_annotated_tag_resolves_to_the_commit_and_not_the_tag_object(self):
        git(self.repo, "tag", "-a", "v0.1.0", "-m", "release", self.second)
        tag_object = git(self.repo, "rev-parse", "v0.1.0")
        self.assertNotEqual(tag_object, self.second)
        self.assertEqual(check_release.tag_commit("v0.1.0", self.repo), self.second)

    def test_a_missing_tag_is_refused(self):
        with self.assertRaises(check_release.ReleaseError):
            check_release.tag_commit("v9.9.9", self.repo)

    def test_a_tag_on_main_is_reachable(self):
        git(self.repo, "tag", "v0.1.0", self.second)
        check_release.check_reachable(self.second, "main", self.repo)

    def test_an_older_commit_of_main_is_reachable(self):
        check_release.check_reachable(self.first, "main", self.repo)

    def test_a_commit_on_a_branch_that_main_does_not_contain_is_not_reachable(self):
        git(self.repo, "checkout", "-b", "feature")
        loose = commit(self.repo, "loose")
        git(self.repo, "checkout", "main")
        with self.assertRaises(check_release.ReleaseError) as caught:
            check_release.check_reachable(loose, "main", self.repo)
        self.assertIn("main", str(caught.exception))
        self.assertIn(loose, str(caught.exception))

    def test_a_branch_that_was_merged_into_main_is_reachable(self):
        git(self.repo, "checkout", "-b", "feature")
        merged = commit(self.repo, "merged")
        git(self.repo, "checkout", "main")
        git(self.repo, "merge", "--no-ff", "-m", "merge", "feature")
        check_release.check_reachable(merged, "main", self.repo)

    def test_a_commit_that_diverged_from_main_after_a_fork_point_is_not_reachable(self):
        git(self.repo, "checkout", "-b", "release-fix", self.first)
        diverged = commit(self.repo, "fix")
        git(self.repo, "checkout", "main")
        with self.assertRaises(check_release.ReleaseError):
            check_release.check_reachable(diverged, "main", self.repo)

    def test_a_main_ref_that_does_not_exist_is_refused(self):
        with self.assertRaises(check_release.ReleaseError):
            check_release.check_reachable(self.second, "origin/main", self.repo)

    def test_a_shallow_clone_is_refused_because_its_history_cannot_prove_anything(self):
        with tempfile.TemporaryDirectory() as clone_dir:
            clone = Path(clone_dir) / "clone"
            git(clone_dir, "clone", "--depth", "1", f"file://{self.repo}", str(clone))
            tip = git(clone, "rev-parse", "HEAD")
            with self.assertRaises(check_release.ReleaseError) as caught:
                check_release.check_reachable(tip, "origin/main", str(clone))
            self.assertIn("shallow", str(caught.exception))


class ChecksTest(unittest.TestCase):
    def problems(self, text, required=("linux", "macos")):
        return check_release.check_run_problems(text, COMMIT, required)

    def test_the_required_checks_are_the_two_ci_jobs(self):
        self.assertEqual(check_release.REQUIRED_CHECKS, ("linux", "macos"))

    def test_the_ci_workflow_has_a_job_for_each_required_check_and_does_not_rename_it(self):
        text = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        for name in check_release.REQUIRED_CHECKS:
            with self.subTest(job=name):
                match = re.search(rf"^  {name}:\n(.*?)(?=^  \S|\Z)", text, re.MULTILINE | re.DOTALL)
                self.assertIsNotNone(match)
                self.assertNotIn("\n    name:", "\n" + match.group(1))

    def test_both_checks_green_on_the_commit_pass(self):
        self.assertEqual(self.problems(GREEN), [])

    def test_a_commit_without_any_check_run_fails_for_every_required_check(self):
        problems = self.problems(page())
        self.assertEqual(len(problems), 2)
        self.assertIn("linux", problems[0])
        self.assertIn("macos", problems[1])

    def test_a_missing_required_check_fails(self):
        problems = self.problems(page(check_run("linux")))
        self.assertEqual(len(problems), 1)
        self.assertIn("macos", problems[0])

    def test_every_conclusion_but_success_fails(self):
        for conclusion in ("failure", "cancelled", "skipped", "neutral", "timed_out", "action_required", "stale", None):
            with self.subTest(conclusion=conclusion):
                text = page(check_run("linux", run_id=1), check_run("macos", conclusion=conclusion, run_id=2))
                problems = self.problems(text)
                self.assertEqual(len(problems), 1)
                self.assertIn("macos", problems[0])

    def test_a_check_that_is_still_running_fails_even_with_a_success_conclusion_field(self):
        for status in ("in_progress", "queued", "waiting"):
            with self.subTest(status=status):
                text = page(check_run("linux", run_id=1), check_run("macos", status=status, run_id=2))
                self.assertEqual(len(self.problems(text)), 1)

    def test_the_newest_run_of_a_check_decides(self):
        recovered = page(
            check_run("linux", run_id=1),
            check_run("macos", conclusion="failure", run_id=2),
            check_run("macos", run_id=3),
        )
        self.assertEqual(self.problems(recovered), [])
        regressed = page(
            check_run("linux", run_id=1),
            check_run("macos", run_id=2),
            check_run("macos", conclusion="failure", run_id=3),
        )
        self.assertEqual(len(self.problems(regressed)), 1)

    def test_the_newest_run_is_chosen_by_id_and_not_by_position(self):
        text = page(
            check_run("linux", run_id=1),
            check_run("macos", conclusion="failure", run_id=9),
            check_run("macos", run_id=4),
        )
        self.assertEqual(len(self.problems(text)), 1)

    def test_a_check_run_from_another_app_does_not_count(self):
        text = page(check_run("linux", run_id=1), check_run("macos", app="some-other-app", run_id=2))
        problems = self.problems(text)
        self.assertEqual(len(problems), 1)
        self.assertIn("macos", problems[0])

    def test_a_check_run_without_an_app_does_not_count(self):
        forged = check_run("macos", run_id=2)
        del forged["app"]
        self.assertEqual(len(self.problems(page(check_run("linux", run_id=1), forged))), 1)

    def test_a_check_run_for_another_commit_does_not_count(self):
        text = page(check_run("linux", run_id=1), check_run("macos", run_id=2, sha="b" * 40))
        self.assertEqual(len(self.problems(text)), 1)

    def test_other_checks_are_ignored(self):
        text = page(
            check_run("linux", run_id=1),
            check_run("macos", run_id=2),
            check_run("docs", conclusion="failure", run_id=3),
        )
        self.assertEqual(self.problems(text), [])

    def test_pages_written_one_after_another_are_all_read(self):
        text = page(check_run("linux", run_id=1)) + "\n" + page(check_run("macos", run_id=2))
        self.assertEqual(self.problems(text), [])

    def test_an_array_of_pages_is_read(self):
        pages = [json.loads(page(check_run("linux", run_id=1))), json.loads(page(check_run("macos", run_id=2)))]
        self.assertEqual(self.problems(json.dumps(pages)), [])

    def test_input_that_is_not_check_run_json_is_refused(self):
        for text in ("", "   ", "not json", "[1, 2]", '{"total_count": 0}', '{"check_runs": "no"}', '{"check_runs": [1]}'):
            with self.subTest(text=text):
                with self.assertRaises(check_release.ReleaseError):
                    self.problems(text)

    def test_a_required_name_can_be_changed_by_the_caller(self):
        self.assertEqual(self.problems(page(check_run("build")), required=("build",)), [])
        self.assertEqual(len(self.problems(page(check_run("build")), required=("build", "test"))), 1)


class CommandLineTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name)
        self.cargo = self.path / "Cargo.toml"
        self.cargo.write_text(CARGO_TOML, encoding="utf-8")

    def test_tag_prints_the_version_and_exits_0_when_the_tag_matches(self):
        code, out, err = run_main("tag", "--tag", "v0.1.0", "--cargo-toml", str(self.cargo))
        self.assertEqual((code, out, err), (0, "0.1.0\n", ""))

    def test_tag_exits_1_with_a_message_and_prints_nothing_on_a_version_mismatch(self):
        code, out, err = run_main("tag", "--tag", "v0.2.0", "--cargo-toml", str(self.cargo))
        self.assertEqual(code, 1)
        self.assertEqual(out, "")
        self.assertIn("v0.2.0", err)
        self.assertIn("0.1.0", err)

    def test_tag_exits_1_on_a_malformed_tag(self):
        code, out, err = run_main("tag", "--tag", "v0.1", "--cargo-toml", str(self.cargo))
        self.assertEqual((code, out), (1, ""))
        self.assertIn("v0.1", err)

    def test_tag_exits_1_when_the_manifest_cannot_be_read(self):
        code, out, err = run_main("tag", "--tag", "v0.1.0", "--cargo-toml", str(self.path / "missing.toml"))
        self.assertEqual((code, out), (1, ""))
        self.assertNotEqual(err, "")

    def test_reachable_prints_the_commit_for_a_tag_on_main(self):
        repo = self.path / "repo"
        repo.mkdir()
        git(repo, "init", "-b", "main")
        sha = commit(repo, "one")
        git(repo, "tag", "-a", "v0.1.0", "-m", "release")
        code, out, err = run_main("reachable", "--tag", "v0.1.0", "--main-ref", "main", "--repo", str(repo))
        self.assertEqual((code, out, err), (0, sha + "\n", ""))

    def test_reachable_exits_1_for_a_tag_that_main_does_not_contain(self):
        repo = self.path / "repo"
        repo.mkdir()
        git(repo, "init", "-b", "main")
        commit(repo, "one")
        git(repo, "checkout", "-b", "feature")
        commit(repo, "loose")
        git(repo, "tag", "v0.1.0")
        git(repo, "checkout", "main")
        code, out, err = run_main("reachable", "--tag", "v0.1.0", "--main-ref", "main", "--repo", str(repo))
        self.assertEqual((code, out), (1, ""))
        self.assertIn("main", err)

    def test_checks_exits_0_when_both_required_checks_passed(self):
        runs = self.path / "runs.json"
        runs.write_text(GREEN, encoding="utf-8")
        code, out, err = run_main("checks", "--commit", COMMIT, "--runs", str(runs))
        self.assertEqual((code, err), (0, ""))
        self.assertIn("linux", out)
        self.assertIn("macos", out)

    def test_checks_exits_1_and_lists_every_problem_when_there_are_no_check_runs(self):
        runs = self.path / "runs.json"
        runs.write_text(page(), encoding="utf-8")
        code, out, err = run_main("checks", "--commit", COMMIT, "--runs", str(runs))
        self.assertEqual((code, out), (1, ""))
        self.assertIn("linux", err)
        self.assertIn("macos", err)
        self.assertIn("ci.yml", err)

    def test_checks_exits_1_on_a_file_that_is_not_json(self):
        runs = self.path / "runs.json"
        runs.write_text("<html>", encoding="utf-8")
        code, out, err = run_main("checks", "--commit", COMMIT, "--runs", str(runs))
        self.assertEqual((code, out), (1, ""))

    def test_checks_exits_1_on_a_missing_file(self):
        code, out, err = run_main("checks", "--commit", COMMIT, "--runs", str(self.path / "none.json"))
        self.assertEqual((code, out), (1, ""))

    def test_checks_takes_the_required_names_from_the_command_line(self):
        runs = self.path / "runs.json"
        runs.write_text(page(check_run("build")), encoding="utf-8")
        code, _, _ = run_main("checks", "--commit", COMMIT, "--runs", str(runs), "--require", "build")
        self.assertEqual(code, 0)

    def test_a_missing_subcommand_is_a_usage_error(self):
        code, out, _ = run_main()
        self.assertEqual((code, out), (2, ""))

    def test_the_script_runs_as_a_program_and_the_exit_status_reaches_the_caller(self):
        version = check_release.crate_version((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        ok = subprocess.run(
            [sys.executable, str(SCRIPT), "tag", "--tag", f"v{version}"], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual((ok.returncode, ok.stdout), (0, version + "\n"))
        wrong = subprocess.run(
            [sys.executable, str(SCRIPT), "tag", "--tag", "v999.0.0"], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual(wrong.returncode, 1)
        self.assertEqual(wrong.stdout, "")


if __name__ == "__main__":
    unittest.main()
