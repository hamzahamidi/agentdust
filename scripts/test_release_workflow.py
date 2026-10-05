import json
import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = ROOT / ".github" / "workflows"
RELEASE = WORKFLOWS / "release.yml"
PIN = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_./-]+)?@[0-9a-f]{40}")
KNOWN_PINS = re.compile(r"uses:\s*(\S+@[0-9a-f]{40})")
TAG_GLOB = "v[0-9]+.[0-9]+.[0-9]+"
VERIFIED_COMMIT = "${{ needs.verify.outputs.commit }}"
TAP_ON = "env.TAP_ENABLED == 'true'"
TAP_OFF = "env.TAP_ENABLED != 'true'"
JOBS = ("verify", "audit", "build", "package", "release", "tap-pr")


def load(path):
    script = "puts JSON.generate(YAML.safe_load(File.read(ARGV[0]), aliases: false))"
    result = subprocess.run(["ruby", "-ryaml", "-rjson", "-e", script, str(path)], capture_output=True, text=True)
    if result.returncode != 0:
        raise AssertionError(f"{path.name} is not valid YAML: {result.stderr}")
    return json.loads(result.stdout)


def toolchain_record():
    return json.loads((ROOT / "release" / "toolchain.json").read_text(encoding="utf-8"))


def steps(job):
    return job.get("steps", [])


def runs(job):
    return "\n".join(step["run"] for step in steps(job) if "run" in step)


def step_index(job, needle):
    for index, step in enumerate(steps(job)):
        if needle in step.get("run", "") or needle in step.get("uses", ""):
            return index
    raise AssertionError(f"no step contains {needle!r}")


def step_with(job, needle):
    return steps(job)[step_index(job, needle)]


def needs(job):
    value = job.get("needs", [])
    return {value} if isinstance(value, str) else set(value)


class ReleaseWorkflowTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = RELEASE.read_text(encoding="utf-8")
        cls.doc = load(RELEASE)
        cls.jobs = cls.doc["jobs"]

    def test_the_file_is_valid_yaml_with_the_expected_jobs(self):
        self.assertEqual(self.doc["name"], "release")
        self.assertEqual(sorted(self.jobs), sorted(JOBS))

    def test_it_runs_only_for_a_pushed_version_tag(self):
        triggers = self.doc.get("on", self.doc.get("true"))
        self.assertEqual(triggers, {"push": {"tags": [TAG_GLOB]}})

    def test_it_has_no_schedule_no_manual_start_and_no_pull_request_trigger(self):
        for word in ("schedule", "cron", "workflow_dispatch", "pull_request", "workflow_run", "branches"):
            with self.subTest(word=word):
                self.assertNotIn(word, self.text)

    def test_the_default_token_can_only_read_contents(self):
        self.assertEqual(self.doc["permissions"], {"contents": "read"})

    def test_only_the_jobs_that_need_it_get_write_access(self):
        writes = {}
        for name, job in self.jobs.items():
            granted = {key for key, value in job.get("permissions", {}).items() if value == "write"}
            if granted:
                writes[name] = granted
        self.assertEqual(writes, {"package": {"id-token", "attestations"}, "release": {"contents"}})

    def test_only_the_gate_reads_checks(self):
        for name, job in self.jobs.items():
            with self.subTest(job=name):
                reads = {key for key, value in job.get("permissions", {}).items() if value == "read"}
                self.assertEqual("checks" in reads, name == "verify")

    def test_every_action_is_pinned_to_a_commit_the_other_workflows_already_use(self):
        known = set()
        for other in ("ci.yml", "release-dry-run.yml"):
            known |= set(KNOWN_PINS.findall((WORKFLOWS / other).read_text(encoding="utf-8")))
        used = [step["uses"] for job in self.jobs.values() for step in steps(job) if "uses" in step]
        self.assertTrue(used)
        for action in used:
            with self.subTest(action=action):
                self.assertRegex(action, PIN)
                self.assertIn(action, known)

    def test_no_run_step_takes_a_value_from_an_expression(self):
        for name, job in self.jobs.items():
            with self.subTest(job=name):
                self.assertNotIn("${{", runs(job))

    def test_every_checkout_keeps_the_token_out_of_the_repository(self):
        checkouts = [
            step for job in self.jobs.values() for step in steps(job) if step.get("uses", "").startswith("actions/checkout@")
        ]
        self.assertGreaterEqual(len(checkouts), 5)
        for step in checkouts:
            self.assertIs(step["with"]["persist-credentials"], False)

    def test_runners_are_named_images_and_never_latest(self):
        for name, job in self.jobs.items():
            with self.subTest(job=name):
                self.assertIn(job["runs-on"], ("ubuntu-24.04", "macos-15"))
        self.assertEqual(self.jobs["build"]["runs-on"], "macos-15")
        self.assertEqual(self.jobs["package"]["runs-on"], "macos-15")

    def test_every_job_has_a_timeout(self):
        for name, job in self.jobs.items():
            with self.subTest(job=name):
                self.assertIsInstance(job["timeout-minutes"], int)

    def test_two_releases_of_one_tag_never_run_side_by_side(self):
        self.assertEqual(self.doc["concurrency"]["cancel-in-progress"], False)
        self.assertIn("github.ref", self.doc["concurrency"]["group"])

    def test_every_script_the_workflow_runs_exists(self):
        scripts = set(re.findall(r"scripts/[a-z_]+\.py", self.text))
        self.assertGreaterEqual(len(scripts), 6)
        for script in scripts:
            with self.subTest(script=script):
                self.assertTrue((ROOT / script).is_file())


class GateTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.jobs = load(RELEASE)["jobs"]
        cls.verify = cls.jobs["verify"]

    def test_the_gate_reads_the_whole_history(self):
        self.assertEqual(steps(self.verify)[0]["with"]["fetch-depth"], 0)

    def test_the_gate_checks_the_tag_then_main_then_ci(self):
        gate = runs(self.verify)
        for command in (
            'check_release.py tag --tag "$GITHUB_REF_NAME"',
            'check_release.py reachable --tag "$GITHUB_REF_NAME" --main-ref origin/main',
            'check_release.py checks --commit "$COMMIT"',
        ):
            self.assertIn(command, gate)
        self.assertLess(gate.index("check_release.py tag"), gate.index("check_release.py reachable"))
        self.assertLess(gate.index("check_release.py reachable"), gate.index("check_release.py checks"))

    def test_the_gate_lists_check_runs_of_the_tag_commit_through_the_api(self):
        gate = runs(self.verify)
        self.assertIn('gh api --paginate "repos/$GITHUB_REPOSITORY/commits/$COMMIT/check-runs', gate)
        self.assertEqual(step_with(self.verify, "gh api")["env"]["GH_TOKEN"], "${{ github.token }}")

    def test_a_failing_script_cannot_be_hidden_inside_an_echo(self):
        gate = runs(self.verify)
        self.assertNotRegex(gate, r"echo\s+\"[^\"]*\$\(")
        self.assertIn('version="$(python3 scripts/check_release.py tag', gate)
        self.assertIn('commit="$(python3 scripts/check_release.py reachable', gate)

    def test_the_checked_out_tree_is_the_commit_that_main_contains(self):
        self.assertIn('test "$(git rev-parse HEAD)" = "$commit"', runs(self.verify))

    def test_the_gate_publishes_the_version_and_the_commit(self):
        self.assertEqual(
            self.verify["outputs"],
            {"version": "${{ steps.tag.outputs.version }}", "commit": "${{ steps.main.outputs.commit }}"},
        )
        self.assertEqual(step_with(self.verify, "check_release.py tag")["id"], "tag")
        self.assertEqual(step_with(self.verify, "check_release.py reachable")["id"], "main")

    def test_nothing_runs_before_the_gate_has_passed(self):
        for name in ("audit", "build", "package", "release", "tap-pr"):
            with self.subTest(job=name):
                self.assertIn("verify", needs(self.jobs[name]))

    def test_every_later_job_builds_the_commit_the_gate_verified(self):
        for name in ("audit", "build", "package", "tap-pr"):
            with self.subTest(job=name):
                checkout = step_with(self.jobs[name], "actions/checkout")
                self.assertEqual(checkout["with"]["ref"], VERIFIED_COMMIT)

    def test_the_release_waits_for_the_audit_the_build_and_the_package(self):
        self.assertLessEqual({"verify", "audit", "package"}, needs(self.jobs["release"]))
        self.assertLessEqual({"verify", "build"}, needs(self.jobs["package"]))
        self.assertLessEqual({"verify", "package", "release"}, needs(self.jobs["tap-pr"]))


class BuildTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = RELEASE.read_text(encoding="utf-8")
        cls.build = load(RELEASE)["jobs"]["build"]

    def test_the_binary_is_built_twice_in_two_separate_jobs(self):
        self.assertEqual(self.build["strategy"]["matrix"], {"copy": ["a", "b"]})
        upload = step_with(self.build, "actions/upload-artifact")
        self.assertEqual(upload["with"]["name"], "build-${{ matrix.copy }}")

    def test_the_toolchain_is_checked_against_the_record_with_the_lock_hash_before_compiling(self):
        check = step_with(self.build, "toolchain.py check")
        self.assertEqual(check["run"].strip(), "python3 scripts/toolchain.py check --include-lock")
        self.assertLess(step_index(self.build, "toolchain.py check"), step_index(self.build, "cargo build"))

    def test_a_missing_record_is_never_accepted(self):
        self.assertNotIn("--allow-missing", self.text)

    def test_the_rust_toolchain_is_installed_before_the_check(self):
        self.assertLess(step_index(self.build, "rustup toolchain install"), step_index(self.build, "toolchain.py check"))

    def test_the_toolchain_of_each_build_is_recorded_for_the_comparison(self):
        self.assertIn("python3 scripts/toolchain.py record > toolchain.json", runs(self.build))

    def test_the_build_is_locked_and_release_mode(self):
        self.assertIn("cargo build --release --locked -p agentdust", runs(self.build))

    def test_the_build_selects_the_xcode_and_sdk_of_the_record(self):
        record = toolchain_record()
        self.assertEqual(self.build["env"]["DEVELOPER_DIR"], record["developer_dir"])
        self.assertEqual(self.build["env"]["SDKROOT"], record["sdk_path"])

    def test_the_rust_version_is_the_pinned_one(self):
        pinned = re.search(r'channel = "([^"]+)"', (ROOT / "rust-toolchain.toml").read_text(encoding="utf-8")).group(1)
        self.assertIn(f'RUST_TOOLCHAIN: "{pinned}"', self.text)


class PackageTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.jobs = load(RELEASE)["jobs"]
        cls.package = cls.jobs["package"]
        cls.script = runs(cls.package)

    def test_the_binaries_are_compared_before_anything_is_packaged(self):
        self.assertIn("python3 scripts/compare_builds.py builds/build-a builds/build-b", self.script)
        self.assertLess(step_index(self.package, "compare_builds.py"), step_index(self.package, "package.py"))

    def test_the_binary_reports_the_version_of_the_tag(self):
        self.assertIn('"$(builds/build-a/target/release/agentdust version)" = "agentdust $VERSION"', self.script)
        self.assertIn("chmod +x builds/build-a/target/release/agentdust", self.script)
        self.assertEqual(self.package["env"]["VERSION"], "${{ needs.verify.outputs.version }}")

    def test_both_binaries_are_packaged_and_the_tarballs_must_be_identical(self):
        self.assertEqual(self.script.count("python3 scripts/package.py"), 2)
        self.assertIn("builds/build-b/target/release/agentdust", self.script)
        self.assertRegex(self.script, r'cmp "dist/[^"]+" "second/[^"]+"')
        self.assertEqual(self.script.count('SOURCE_DATE_EPOCH="$epoch"'), 2)
        self.assertIn('epoch="$(git log -1 --format=%ct)"', self.script)

    def test_the_checksum_lists_the_tarball_and_is_verified(self):
        self.assertIn("shasum -a 256 *.tar.gz *-sbom.cdx.json > SHA256SUMS", self.script)
        self.assertIn("shasum -a 256 -c SHA256SUMS", self.script)

    def test_the_binary_and_the_tarball_are_attested(self):
        attest = step_with(self.package, "actions/attest-build-provenance")
        subjects = attest["with"]["subject-path"].split()
        self.assertEqual(subjects, ["builds/build-a/target/release/agentdust", "dist/*.tar.gz", "dist/*-sbom.cdx.json"])

    def test_the_attestation_comes_after_the_comparison_and_the_checksum(self):
        attest = step_index(self.package, "actions/attest-build-provenance")
        self.assertLess(step_index(self.package, "compare_builds.py"), attest)
        self.assertLess(step_index(self.package, "anchore/sbom-action"), attest)
        self.assertLess(step_index(self.package, "SHA256SUMS"), attest)

    def test_the_release_sbom_is_generated_from_the_source_and_included_in_release_files(self):
        sbom = step_with(self.package, "anchore/sbom-action")
        self.assertEqual(sbom["with"]["path"], ".")
        self.assertEqual(sbom["with"]["format"], "cyclonedx-json")
        self.assertEqual(sbom["with"]["output-file"], "dist/agentdust-${{ env.VERSION }}-sbom.cdx.json")
        self.assertEqual(sbom["with"]["upload-artifact"], False)
        self.assertEqual(sbom["with"]["upload-release-assets"], False)
        release_files = step_with(self.package, "actions/upload-artifact")
        self.assertEqual(release_files["with"]["name"], "release-files")
        self.assertEqual(release_files["with"]["path"], "dist")

    def test_the_formula_points_at_the_release_download_and_is_checked_as_ruby(self):
        self.assertIn("python3 scripts/formula.py", self.script)
        self.assertIn(
            'releases/download/$GITHUB_REF_NAME/agentdust-$VERSION-darwin-arm64.tar.gz', self.script
        )
        self.assertIn("--out formula/agentdust.rb", self.script)
        self.assertIn("ruby -c formula/agentdust.rb", self.script)

    def test_the_release_tarball_is_installed_through_a_local_tap_before_the_release(self):
        self.assertIn('python3 scripts/brew_smoke.py smoke/agentdust.rb "$VERSION"', self.script)
        self.assertIn('--url "file://$PWD/dist/', self.script)

    def test_the_release_files_and_the_formula_leave_as_artifacts(self):
        uploads = {
            step["with"]["name"]: step["with"]["path"]
            for step in steps(self.package)
            if step.get("uses", "").startswith("actions/upload-artifact@")
        }
        self.assertEqual(uploads["release-files"].split(), ["dist"])
        self.assertEqual(uploads["homebrew-formula"].split(), ["formula/agentdust.rb"])


class ReleaseJobTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.jobs = load(RELEASE)["jobs"]

    def test_the_audit_runs_cargo_audit_with_the_pinned_tool_installer(self):
        audit = self.jobs["audit"]
        self.assertEqual(audit["runs-on"], "ubuntu-24.04")
        self.assertIn("cargo audit", runs(audit))
        installer = step_with(audit, "taiki-e/install-action")
        self.assertEqual(installer["with"]["tool"], "cargo-audit")

    def test_the_release_is_a_draft_on_a_tag_that_exists(self):
        release = self.jobs["release"]
        command = runs(release)
        self.assertIn("gh release create", command)
        self.assertIn("--draft", command)
        self.assertIn("--verify-tag", command)
        self.assertIn('"$GITHUB_REF_NAME"', command)

    def test_nothing_publishes_the_release_or_changes_an_existing_one(self):
        command = runs(self.jobs["release"])
        for word in ("gh release edit", "gh release upload", "gh release delete", "--draft=false", "--latest"):
            with self.subTest(word=word):
                self.assertNotIn(word, command)

    def test_the_job_that_can_write_the_release_never_checks_out_the_source(self):
        release = self.jobs["release"]
        self.assertEqual(release["permissions"], {"contents": "write"})
        self.assertFalse([step for step in steps(release) if step.get("uses", "").startswith("actions/checkout@")])
        download = step_with(release, "actions/download-artifact")
        self.assertEqual(download["with"], {"name": "release-files", "path": "dist"})

    def test_the_release_carries_the_tarball_and_the_checksum(self):
        self.assertIn("dist/*", runs(self.jobs["release"]))


class TapJobTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = RELEASE.read_text(encoding="utf-8")
        cls.jobs = load(RELEASE)["jobs"]
        cls.tap = cls.jobs["tap-pr"]

    def test_the_job_is_off_until_the_secret_exists(self):
        self.assertEqual(self.tap["environment"], "release")
        self.assertEqual(self.tap["env"]["TAP_ENABLED"], "${{ secrets.HOMEBREW_TAP_TOKEN != '' }}")
        self.assertNotIn("if", self.tap)

    def test_every_step_is_guarded_by_the_secret_check_except_the_notice(self):
        guards = []
        for step in steps(self.tap):
            guards.append(step.get("if"))
        self.assertEqual(guards.count(TAP_OFF), 1)
        self.assertEqual([guard for guard in guards if guard != TAP_OFF], [TAP_ON] * (len(guards) - 1))

    def test_the_notice_says_why_nothing_happened_and_does_nothing_else(self):
        notice = [step for step in steps(self.tap) if step["if"] == TAP_OFF][0]
        self.assertIn("HOMEBREW_TAP_TOKEN", notice["run"])
        self.assertNotIn("secrets.", json.dumps(notice))

    def test_the_secret_appears_only_in_the_tap_job_and_only_in_guarded_steps(self):
        self.assertEqual(sum(1 for job in self.jobs.values() if "secrets.HOMEBREW_TAP_TOKEN" in json.dumps(job)), 1)
        self.assertIn("secrets.HOMEBREW_TAP_TOKEN", json.dumps(self.tap))
        for step in steps(self.tap):
            if "secrets.HOMEBREW_TAP_TOKEN" in json.dumps(step):
                self.assertEqual(step["if"], TAP_ON)

    def test_the_secret_is_not_a_job_wide_environment_value(self):
        self.assertNotIn("HOMEBREW_TAP_TOKEN", json.dumps({key: value for key, value in self.tap["env"].items() if key != "TAP_ENABLED"}))

    def test_the_job_itself_can_only_read_contents(self):
        self.assertEqual(self.tap["permissions"], {"contents": "read"})

    def test_the_tap_repository_is_named_once_for_the_whole_workflow(self):
        self.assertRegex(self.text, r"(?m)^  TAP_REPOSITORY: hamzahamidi/homebrew-agentdust$")
        checkout = [step for step in steps(self.tap) if step.get("with", {}).get("repository")][0]
        self.assertEqual(checkout["with"]["repository"], "${{ env.TAP_REPOSITORY }}")
        self.assertEqual(checkout["with"]["token"], "${{ secrets.HOMEBREW_TAP_TOKEN }}")

    def test_the_formula_files_come_from_the_tested_script_and_the_pull_request_is_only_opened(self):
        command = runs(self.tap)
        self.assertIn('python3 scripts/tap_update.py --tap tap --formula formula/agentdust.rb --version "$VERSION"', command)
        self.assertIn("gh pr create", command)
        for word in ("gh pr merge", "--auto", "git push --force", "--force"):
            with self.subTest(word=word):
                self.assertNotIn(word, command)

    def test_the_formula_comes_from_the_package_job_of_this_run(self):
        download = step_with(self.tap, "actions/download-artifact")
        self.assertEqual(download["with"], {"name": "homebrew-formula", "path": "formula"})


class DryRunTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.jobs = load(WORKFLOWS / "release-dry-run.yml")["jobs"]
        cls.build = cls.jobs["build"]
        cls.package = cls.jobs["package"]

    def test_the_dry_run_packages_the_version_from_the_workspace_manifest(self):
        version = step_with(self.package, "tomllib.load")
        self.assertIn('tomllib.load(open("Cargo.toml", "rb"))', version["run"])
        self.assertIn('>> "$GITHUB_ENV"', version["run"])

    def test_the_dry_run_generates_checksums_and_an_attested_sbom(self):
        sbom = step_with(self.package, "anchore/sbom-action")
        self.assertEqual(sbom["with"]["format"], "cyclonedx-json")
        self.assertEqual(sbom["with"]["output-file"], "dist/agentdust-${{ env.VERSION }}-sbom.cdx.json")
        self.assertIn("*-sbom.cdx.json", runs(self.package))
        attest = step_with(self.package, "actions/attest-build-provenance")
        self.assertIn("dist/*-sbom.cdx.json", attest["with"]["subject-path"])

    def test_the_toolchain_is_recorded_before_it_is_checked(self):
        self.assertLess(step_index(self.build, "toolchain.py record"), step_index(self.build, "toolchain.py check"))

    def test_the_record_is_uploaded_even_when_the_check_found_drift(self):
        upload = step_with(self.build, "actions/upload-artifact")
        self.assertEqual(upload["if"], "always()")
        self.assertIn("toolchain.json", upload["with"]["path"].split())

    def test_the_dry_run_still_builds_only_after_the_check_passed(self):
        self.assertLess(step_index(self.build, "toolchain.py check"), step_index(self.build, "cargo build"))


if __name__ == "__main__":
    unittest.main()
