import json
import re
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_release

ROOT = Path(__file__).resolve().parent.parent
README = ROOT / "README.md"
SECURITY = ROOT / "SECURITY.md"
RELEASE_DOC = ROOT / "docs" / "release.md"
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
MAIN_RS = ROOT / "crates" / "agentdust" / "src" / "main.rs"
SETUP_RS = ROOT / "crates" / "agentdust" / "src" / "setup.rs"
THREAT_MODEL = ROOT / "docs" / "threat-model.md"
REPOSITORY = "hamzahamidi/agentdust"
SIGNER = f"{REPOSITORY}/.github/workflows/release.yml"
RELEASE_COMMANDS = {"setup", "doctor", "apply", "status", "version", "mcp", "hook"}
BANNED = (
    "ensure",
    "leverage",
    "comprehensive",
    "robust",
    "seamless",
    "optimize",
    "overall",
    "ultimately",
    "additionally",
    "furthermore",
    "moreover",
)


def read(path):
    return path.read_text(encoding="utf-8")


def section(text, heading):
    lines = text.splitlines()
    for start, line in enumerate(lines):
        if re.fullmatch(heading, line):
            level = len(line) - len(line.lstrip("#"))
            end = len(lines)
            for index in range(start + 1, len(lines)):
                stripped = lines[index]
                if stripped.startswith("#") and len(stripped) - len(stripped.lstrip("#")) <= level:
                    end = index
                    break
            return "\n".join(lines[start:end])
    raise AssertionError(f"no heading matching {heading!r}")


def binary_has(command):
    return f'"{command}"' in read(MAIN_RS)


class ReadmeUsageTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = read(README)
        cls.usage = section(cls.text, r"## Using AgentDust \(from release 0\.1\)")

    def test_the_section_says_the_release_is_not_published(self):
        self.assertIn("not published", self.usage)

    def test_the_section_covers_the_commands_of_release_0_1(self):
        for needle in (
            "brew install hamzahamidi/agentdust/agentdust",
            "agentdust setup",
            "agentdust doctor",
            "agentdust doctor --json",
            "agentdust apply",
            "agentdust status",
            "agentdust setup --remove",
            "agentdust_doctor",
            "agentdust_plan",
            "agentdust_apply",
            "apply = false",
            "SIGTERM",
        ):
            with self.subTest(needle=needle):
                self.assertIn(needle, self.usage)

    def test_the_section_names_the_classes_and_which_of_them_can_be_signalled(self):
        for needle in ("owned-ended", "suspect", "managed", "unknown", "owned-live"):
            with self.subTest(needle=needle):
                self.assertIn(needle, self.usage)

    def test_the_section_states_the_approval_rules(self):
        for needle in ("4-character code", "10 items", "one code per suspect", "apply_not_supported"):
            with self.subTest(needle=needle):
                self.assertIn(needle, self.usage)

    def test_the_section_leaves_out_what_release_0_1_does_not_have(self):
        for needle in ("support-bundle", "codex mcp add", "--claude-config-dir", "--purge-data", "SIGKILL"):
            with self.subTest(needle=needle):
                self.assertNotIn(needle, self.usage)

    def test_the_section_links_the_release_document_for_verification(self):
        self.assertIn("docs/release.md#verify-a-release", self.usage)

    def test_every_command_the_readme_shows_is_a_command_of_release_0_1(self):
        shown = set(re.findall(r"\bagentdust ([a-z][a-z-]*)", self.text))
        self.assertTrue(shown)
        self.assertLessEqual(shown, RELEASE_COMMANDS, shown - RELEASE_COMMANDS)

    def test_the_flags_the_readme_gives_setup_are_the_ones_the_binary_parses(self):
        flags = set(re.findall(r"agentdust setup((?: --[a-z-]+)+)", self.text))
        parsed = {flag for group in flags for flag in group.split()}
        self.assertLessEqual(parsed, {"--check", "--remove", "--yes"})
        for flag in parsed:
            with self.subTest(flag=flag):
                self.assertIn(f'"{flag}"', read(SETUP_RS))

    def test_the_only_flag_the_readme_gives_doctor_is_json(self):
        flags = set(re.findall(r"agentdust doctor((?: --[a-z-]+)+)", self.text))
        self.assertEqual(flags, {" --json"})

    def test_a_command_the_binary_lacks_is_listed_for_the_integrator(self):
        integrator = section(read(RELEASE_DOC), r"## For the integrator")
        for command in ("doctor", "apply"):
            if not binary_has(command):
                with self.subTest(command=command):
                    self.assertIn(f"`agentdust {command}`", integrator)

    def test_the_status_section_links_the_release_document(self):
        self.assertIn("docs/release.md", section(self.text, r"## Status"))


class ReleaseDocTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = read(RELEASE_DOC)
        cls.jobs = json.loads(
            subprocess.run(
                ["ruby", "-ryaml", "-rjson", "-e", "puts JSON.generate(YAML.safe_load(File.read(ARGV[0]))['jobs'].keys)", str(WORKFLOW)],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
        )

    def test_the_document_has_the_four_parts(self):
        for heading in ("## Cut a release", "## Verify a release", "## Roll back", "## For the integrator"):
            with self.subTest(heading=heading):
                self.assertRegex(self.text, rf"(?m)^{re.escape(heading)}$")

    def test_every_job_of_the_workflow_is_described(self):
        self.assertEqual(len(self.jobs), 6)
        for job in self.jobs:
            with self.subTest(job=job):
                self.assertIn(f"`{job}`", self.text)

    def test_verification_names_the_repository_and_the_signing_workflow_that_exists(self):
        verify = section(self.text, r"## Verify a release")
        self.assertIn(f"--repo {REPOSITORY}", verify)
        self.assertIn(f"--signer-workflow {SIGNER}", verify)
        self.assertTrue((ROOT / ".github" / "workflows" / "release.yml").is_file())
        self.assertGreaterEqual(verify.count("gh attestation verify"), 2)
        self.assertIn("shasum -a 256 -c SHA256SUMS", verify)

    def test_the_document_says_what_homebrew_does_not_verify(self):
        self.assertIn("Homebrew does not verify", self.text)

    def test_cutting_a_release_starts_ci_on_the_tag_commit_with_the_command_the_gate_prints(self):
        cut = section(self.text, r"## Cut a release")
        self.assertIn("gh workflow run ci.yml --ref main", cut)
        self.assertIn("gh workflow run ci.yml --ref main", check_release.CI_HINT)
        for needle in ("release-dry-run", "release/toolchain.json", "git tag -a", "TAP_TOKEN", "homebrew-agentdust", "[workspace.package]"):
            with self.subTest(needle=needle):
                self.assertIn(needle, cut)

    def test_rolling_back_gives_both_ways_out(self):
        back = section(self.text, r"## Roll back")
        for needle in ("apply = false", "agentdust@", "agentdust setup --remove", "config.toml"):
            with self.subTest(needle=needle):
                self.assertIn(needle, back)

    def test_the_integrator_part_lists_what_the_workflow_cannot_settle(self):
        integrator = section(self.text, r"## For the integrator")
        for needle in ("0.0.0", "release/toolchain.json", "SBOM", "TAP_TOKEN", "fork", "no release yet", "typed-code form"):
            with self.subTest(needle=needle):
                self.assertIn(needle, integrator)

    def test_every_link_points_at_a_file(self):
        for target in re.findall(r"\]\(([^)\s#]+)(?:#[^)]*)?\)", self.text):
            if "://" not in target:
                with self.subTest(target=target):
                    self.assertTrue((RELEASE_DOC.parent / target).exists(), target)


class SecurityPolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = read(SECURITY)

    def test_the_supported_versions_name_the_latest_release_line_and_main(self):
        supported = section(self.text, r"## Supported versions")
        self.assertIn("latest 0.x release", supported)
        self.assertIn("`main`", supported)
        self.assertNotIn("Nothing is released yet", self.text)

    def test_a_withdrawn_release_has_both_ways_out(self):
        withdrawn = section(self.text, r"## When a release is withdrawn")
        for needle in ("apply = false", "agentdust@", "docs/release.md#roll-back"):
            with self.subTest(needle=needle):
                self.assertIn(needle, withdrawn)

    def test_the_threat_model_link_stays(self):
        self.assertIn("](docs/threat-model.md)", self.text)


class ThreatModelReleaseRowsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rows = [line for line in read(THREAT_MODEL).splitlines() if "| S20 |" in line]

    def test_the_gate_and_the_rollback_rows_are_implemented_and_name_their_tests(self):
        implemented = "\n".join(row for row in self.rows if "| implemented |" in row)
        for path in (
            ".github/workflows/release.yml",
            "scripts/check_release.py",
            "scripts/test_check_release.py",
            "scripts/test_release_workflow.py",
            "scripts/tap_update.py",
            "scripts/test_tap_update.py",
            "docs/release.md",
        ):
            with self.subTest(path=path):
                self.assertIn(f"`{path}`", implemented)

    def test_only_the_sbom_and_scoped_token_row_is_still_planned(self):
        planned = [row for row in self.rows if "planned M3" in row]
        self.assertEqual(len(planned), 1)
        self.assertIn("SBOM", planned[0])
        self.assertIn("scoped", planned[0])

    def test_the_lock_hash_is_described_as_compared_by_the_release(self):
        text = read(THREAT_MODEL)
        self.assertIn("--include-lock", text)
        self.assertNotIn("except the `Cargo.lock` hash", text)


class StyleTest(unittest.TestCase):
    def test_the_documents_have_no_em_or_en_dash_and_none_of_the_banned_words(self):
        for path in (README, SECURITY, RELEASE_DOC):
            text = read(path)
            with self.subTest(path=path.name):
                self.assertNotIn("—", text)
                self.assertNotIn("–", text)
                for word in BANNED:
                    self.assertIsNone(re.search(rf"(?i)\b{word}\b", text), word)


if __name__ == "__main__":
    unittest.main()
