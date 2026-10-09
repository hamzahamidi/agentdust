import base64
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import npm_package
import npm_publish
from test_release_workflow import WORKFLOWS, load, runs, step_index, steps, PIN

ROOT = Path(__file__).resolve().parent.parent
VERSION = "1.5.0"


class NpmPackageTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.binary = self.root / "agentdust"
        self.binary.write_text(f"#!/bin/sh\nprintf 'agentdust {VERSION}\\n'\n")
        self.binary.chmod(0o755)
        (self.root / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{VERSION}"\n')
        (self.root / "LICENSE").write_text("MIT\n")
        (self.root / "README.md").write_text("AgentDust\n")

    def package(self, out="dist", version=VERSION):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/npm_package.py"), "--binary", str(self.binary), "--version", version, "--out-dir", out],
            cwd=self.root, env={**os.environ, "SOURCE_DATE_EPOCH": "1700000000"}, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return self.root / out / f"agentdust-{version}-npm.tgz"

    def artifacts(self):
        package = self.package()
        directory = package.parent
        native = directory / f"agentdust-{VERSION}-darwin-arm64.tar.gz"
        with tarfile.open(native, "w:gz") as tar:
            tar.add(self.binary, arcname=f"agentdust-{VERSION}-darwin-arm64/agentdust")
        (directory / "release.json").write_text(json.dumps({"tag_name": f"v{VERSION}", "draft": False, "prerelease": False}))
        self.checksums(directory)
        return directory, package, native

    def checksums(self, directory):
        (directory / "SHA256SUMS").write_text("".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
            for path in sorted(directory.iterdir()) if path.suffix in (".tgz", ".gz")
        ))

    def test_package_is_reproducible_and_contains_the_unchanged_executable(self):
        one, two = self.package("one"), self.package("two")
        self.assertEqual(one.read_bytes(), two.read_bytes())
        with tarfile.open(one) as tar:
            self.assertEqual(tar.extractfile("package/bin/agentdust").read(), self.binary.read_bytes())
            self.assertEqual(tar.getmember("package/bin/agentdust").mode, 0o755)
            self.assertEqual(json.load(tar.extractfile("package/package.json")), npm_package.manifest(VERSION))
            self.assertTrue(all(item.uid == 0 and item.mtime == 1700000000 for item in tar.getmembers()))
        self.assertEqual(one.read_bytes()[4:8], b"\x00" * 4)

    def test_binary_and_workspace_version_must_match_the_package(self):
        for wrong_workspace in (False, True):
            if wrong_workspace:
                (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "1.4.0"\n')
            else:
                self.binary.write_text("#!/bin/sh\necho 'agentdust 1.4.0'\n")
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/npm_package.py"), "--binary", str(self.binary), "--version", VERSION, "--out-dir", "bad"],
                cwd=self.root, capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((self.root / "bad").exists())

    def test_verified_release_is_accepted(self):
        directory, package, _ = self.artifacts()
        self.assertEqual(npm_publish.validate(directory, VERSION), package)

    def test_drafts_prereleases_and_another_tag_are_rejected(self):
        directory, _, _ = self.artifacts()
        for fields in ({"draft": True}, {"prerelease": True}, {"tag_name": "v1.4.0"}):
            (directory / "release.json").write_text(json.dumps({"tag_name": f"v{VERSION}", "draft": False, "prerelease": False, **fields}))
            with self.assertRaisesRegex(ValueError, "public stable"):
                npm_publish.validate(directory, VERSION)

    def test_modified_package_and_missing_checksum_are_rejected(self):
        directory, package, _ = self.artifacts()
        package.write_bytes(package.read_bytes() + b"changed")
        with self.assertRaisesRegex(ValueError, "checksum"):
            npm_publish.validate(directory, VERSION)
        (directory / "SHA256SUMS").write_text("")
        with self.assertRaisesRegex(ValueError, "checksum"):
            npm_publish.validate(directory, VERSION)

    def test_a_binary_different_from_the_native_release_is_rejected(self):
        directory, _, native = self.artifacts()
        with tarfile.open(native, "w:gz") as tar:
            member = tarfile.TarInfo(f"agentdust-{VERSION}-darwin-arm64/agentdust")
            member.size = 7
            tar.addfile(member, io.BytesIO(b"another"))
        self.checksums(directory)
        with self.assertRaisesRegex(ValueError, "differs"):
            npm_publish.validate(directory, VERSION)

    def test_install_scripts_and_wrong_platform_metadata_are_rejected(self):
        directory, package, _ = self.artifacts()
        for change in ({"scripts": {"postinstall": "curl example.invalid"}}, {"os": ["linux"]}, {"version": "1.4.0"}):
            with tarfile.open(package) as tar:
                files = [(item, tar.extractfile(item).read()) for item in tar.getmembers()]
            with tarfile.open(package, "w:gz") as tar:
                for member, data in files:
                    if member.name == "package/package.json":
                        data = json.dumps({**npm_package.manifest(VERSION), **change}).encode()
                        member.size = len(data)
                    tar.addfile(member, io.BytesIO(data))
            self.checksums(directory)
            with self.assertRaisesRegex(ValueError, "manifest"):
                npm_publish.validate(directory, VERSION)

    def test_publication_retries_accept_only_identical_registry_bytes(self):
        package = self.package()
        integrity = "sha512-" + base64.b64encode(hashlib.sha512(package.read_bytes()).digest()).decode()
        with patch("npm_publish.subprocess.run", return_value=subprocess.CompletedProcess([], 0, json.dumps(integrity), "")) as run:
            npm_publish.publish(package, VERSION)
            self.assertEqual(run.call_count, 1)
        with patch("npm_publish.subprocess.run", return_value=subprocess.CompletedProcess([], 0, '"different"', "")) as run:
            with self.assertRaisesRegex(ValueError, "different package bytes"):
                npm_publish.publish(package, VERSION)
            self.assertEqual(run.call_count, 1)

    def test_registry_failure_cannot_be_treated_as_a_new_package(self):
        package = self.package()
        with patch("npm_publish.subprocess.run", return_value=subprocess.CompletedProcess([], 1, '{"error":{"code":"E401"}}', "unauthenticated")) as run:
            with self.assertRaisesRegex(ValueError, "publication state"):
                npm_publish.publish(package, VERSION)
            self.assertEqual(run.call_count, 1)

    def test_an_absent_version_is_published_without_lifecycle_scripts(self):
        package = self.package()
        absent = subprocess.CompletedProcess([], 1, '{"error":{"code":"E404"}}', "")
        with patch("npm_publish.subprocess.run", side_effect=[absent, subprocess.CompletedProcess([], 0)]) as run:
            npm_publish.publish(package, VERSION)
            self.assertEqual(run.call_count, 2)
            self.assertIn("--ignore-scripts", run.call_args.args[0])


class NpmWorkflowTest(unittest.TestCase):
    def test_publishing_requires_verified_public_release_artifacts_and_oidc(self):
        workflow = load(WORKFLOWS / "npm-publish.yml")
        triggers = workflow.get("on", workflow.get("true"))
        self.assertEqual(triggers["release"]["types"], ["published"])
        job = workflow["jobs"]["publish"]
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        self.assertEqual(job["permissions"], {"attestations": "read", "contents": "read", "id-token": "write"})
        self.assertEqual(job["environment"], "npm")
        verification = steps(job)[step_index(job, "--verify-only")]["run"]
        self.assertLess(verification.index("--verify-only"), verification.index("gh attestation verify"))
        self.assertLess(step_index(job, "gh attestation verify"), len(steps(job)) - 1)
        self.assertIn('--source-ref "refs/tags/$RELEASE_TAG"', runs(job))
        self.assertNotIn("${{", runs(job))
        self.assertNotIn("NPM_TOKEN", json.dumps(workflow))
        self.assertIn('test "$GITHUB_REF" = refs/heads/main', runs(job))
        for step in steps(job):
            if "uses" in step:
                self.assertRegex(step["uses"], PIN)

    def test_ci_and_both_release_paths_exercise_the_packed_install(self):
        for name in ("ci.yml", "release.yml", "release-dry-run.yml"):
            workflow = load(WORKFLOWS / name)
            commands = "\n".join(runs(job) for job in workflow["jobs"].values())
            self.assertIn("scripts/npm_package.py", commands)
            self.assertIn("scripts/npm_smoke.py", commands)
        for name in ("release.yml", "release-dry-run.yml"):
            job = load(WORKFLOWS / name)["jobs"]["package"]
            self.assertIn("dist/*.tgz", json.dumps(job))
            self.assertIn("*.tgz *-sbom.cdx.json > SHA256SUMS", runs(job))

    def test_homebrew_recovery_downloads_every_asset_in_the_checksum_list(self):
        commands = runs(load(WORKFLOWS / "homebrew-tap-recovery.yml")["jobs"]["tap-pr"])
        download = next(line for line in commands.splitlines() if "gh release download" in line)
        self.assertNotIn("--pattern", download)
        self.assertIn("shasum -a 256 -c SHA256SUMS", commands)


if __name__ == "__main__":
    unittest.main()
