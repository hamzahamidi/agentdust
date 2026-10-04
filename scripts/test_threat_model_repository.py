import re
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_threat_model

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check_threat_model.py"
MODEL = "docs/threat-model.md"
CHECKER_STEP = "python3 scripts/check_threat_model.py"
TESTS_STEP = "python3 -m unittest discover -s scripts -p 'test_*.py'"
TOOLCHAIN_STEP = "rustup toolchain install"
ROADMAP_LINE = re.compile(r"^- Written threat model covering (.+)\.$", re.MULTILINE)


def roadmap_adversaries():
    match = ROADMAP_LINE.search((ROOT / "ROADMAP.md").read_text(encoding="utf-8"))
    names = re.split(r",\s*|\s+and\s+", match.group(1)) if match else []
    return [re.sub(r"^(?:a|an|the)\s+", "", name.strip()).lower() for name in names]


def ci_job(name):
    text = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    match = re.search(rf"^  {name}:\n(.*?)(?=^  \S|\Z)", text, re.MULTILINE | re.DOTALL)
    return match.group(1) if match else ""


class RepositoryThreatModelTest(unittest.TestCase):
    def test_the_written_threat_model_satisfies_the_checker(self):
        result = subprocess.run([sys.executable, str(SCRIPT)], capture_output=True, text=True, cwd=ROOT)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")

    def test_the_security_policy_links_to_the_threat_model(self):
        self.assertIn(f"]({MODEL})", (ROOT / "SECURITY.md").read_text(encoding="utf-8"))

    def test_the_readme_links_to_the_threat_model(self):
        self.assertIn(f"]({MODEL})", (ROOT / "README.md").read_text(encoding="utf-8"))

    def test_every_link_in_the_threat_model_points_at_a_file(self):
        text = (ROOT / MODEL).read_text(encoding="utf-8")
        base = (ROOT / MODEL).parent
        targets = [t for t in re.findall(r"\]\(([^)\s#]+)(?:#[^)]*)?\)", text) if "://" not in t]
        self.assertTrue(targets)
        for target in targets:
            with self.subTest(target=target):
                self.assertTrue((base / target).exists(), target)

    def test_the_roadmap_names_eight_adversaries(self):
        self.assertEqual(len(roadmap_adversaries()), 8, roadmap_adversaries())

    def test_every_adversary_the_roadmap_names_has_a_section_the_checker_requires(self):
        required = {title.lower() for title in check_threat_model.ADVERSARIES}
        for name in roadmap_adversaries():
            with self.subTest(adversary=name):
                self.assertIn(name, required)


class ContinuousIntegrationTest(unittest.TestCase):
    def test_the_linux_job_exists(self):
        self.assertIn("runs-on: ubuntu-24.04", ci_job("linux"))

    def test_the_linux_job_runs_the_threat_model_checker(self):
        self.assertIn(CHECKER_STEP, ci_job("linux"))

    def test_the_linux_job_runs_the_script_tests(self):
        self.assertIn(TESTS_STEP, ci_job("linux"))

    def test_both_python_steps_run_before_the_rust_toolchain_is_installed(self):
        job = ci_job("linux")
        for step in (CHECKER_STEP, TESTS_STEP):
            with self.subTest(step=step):
                self.assertIn(step, job)
                self.assertLess(job.index(step), job.index(TOOLCHAIN_STEP))

    def test_python_bytecode_is_ignored(self):
        lines = (ROOT / ".gitignore").read_text(encoding="utf-8").splitlines()
        self.assertIn("__pycache__/", [line.strip() for line in lines])


if __name__ == "__main__":
    unittest.main()
