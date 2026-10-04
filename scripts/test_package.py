import gzip
import hashlib
import io
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().with_name("package.py")
NAME = "agentdust-0.1.0-darwin-arm64"


class PackageTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name)
        (self.path / "LICENSE").write_text("licence text\n")
        (self.path / "README.md").write_text("readme text\n")
        self.binary = self.path / "agentdust"
        self.binary.write_bytes(b"\xcf\xfa\xed\xfe binary \x00\x01\x02")
        self.binary.chmod(0o755)

    def package(self, out="dist", epoch="1700000000", version="0.1.0", binary=None, cwd=None):
        env = {**os.environ, "SOURCE_DATE_EPOCH": epoch}
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--binary", str(binary or self.binary),
                "--version", version,
                "--out-dir", str(self.path / out),
            ],
            cwd=cwd or self.path,
            env=env,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.path / out / f"agentdust-{version}-darwin-arm64.tar.gz", result.stdout

    def members(self, tarball):
        with tarfile.open(tarball) as tar:
            return tar.getmembers()

    def test_the_same_inputs_give_the_same_bytes_in_two_runs_and_two_directories(self):
        first, _ = self.package("one")
        second, _ = self.package("two")
        self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_the_printed_line_is_the_sha256_of_the_file_and_its_name(self):
        tarball, out = self.package()
        self.assertEqual(out, f"{hashlib.sha256(tarball.read_bytes()).hexdigest()}  {tarball.name}\n")

    def test_the_tarball_does_not_depend_on_file_times_or_the_execute_bit_of_the_input(self):
        reference, _ = self.package("one")
        os.utime(self.binary, (1, 1))
        os.utime(self.path / "LICENSE", (2, 2))
        self.binary.chmod(0o600)
        again, _ = self.package("two")
        self.assertEqual(reference.read_bytes(), again.read_bytes())

    def test_a_changed_binary_or_epoch_or_version_changes_the_tarball(self):
        reference, _ = self.package("one")
        self.binary.write_bytes(b"another binary")
        changed, _ = self.package("two")
        self.assertNotEqual(reference.read_bytes(), changed.read_bytes())
        epoch, _ = self.package("three", epoch="1700000001")
        self.assertNotEqual(changed.read_bytes(), epoch.read_bytes())
        version, _ = self.package("four", version="0.1.1")
        self.assertNotEqual(version.name, reference.name)

    def test_entries_are_sorted_with_fixed_owner_times_and_modes(self):
        tarball, _ = self.package(epoch="1700000000")
        members = self.members(tarball)
        names = [member.name for member in members]
        self.assertEqual(names, sorted(names))
        self.assertEqual(names, [f"{NAME}/LICENSE", f"{NAME}/README.md", f"{NAME}/agentdust"])
        for member in members:
            with self.subTest(member=member.name):
                self.assertEqual((member.uid, member.gid, member.uname, member.gname), (0, 0, "", ""))
                self.assertEqual(member.mtime, 1700000000)
                self.assertEqual(member.mode, 0o755 if member.name.endswith("/agentdust") else 0o644)
                self.assertTrue(member.isreg())

    def test_the_binary_inside_is_the_binary_that_went_in(self):
        tarball, _ = self.package()
        with tarfile.open(tarball) as tar:
            self.assertEqual(tar.extractfile(f"{NAME}/agentdust").read(), self.binary.read_bytes())

    def test_the_gzip_header_carries_no_name_and_no_time(self):
        tarball, _ = self.package()
        header = tarball.read_bytes()[:10]
        self.assertEqual(header[:2], b"\x1f\x8b")
        self.assertEqual(header[3] & 0x08, 0)
        self.assertEqual(header[4:8], b"\x00\x00\x00\x00")

    def test_the_tar_stream_is_the_ustar_format(self):
        tarball, _ = self.package()
        raw = gzip.decompress(tarball.read_bytes())
        self.assertEqual(raw[257:263], b"ustar\x00")
        self.assertEqual(len(tarfile.open(fileobj=io.BytesIO(raw)).getmembers()), 3)

    def test_a_missing_binary_is_an_error_and_leaves_no_tarball(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--binary", str(self.path / "none"), "--version", "0.1.0", "--out-dir", str(self.path / "dist")],
            cwd=self.path,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.path / "dist" / f"{NAME}.tar.gz").exists())


if __name__ == "__main__":
    unittest.main()
