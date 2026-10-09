import argparse
import gzip
import hashlib
import io
import json
import os
import subprocess
import tarfile
from pathlib import Path

from check_release import check_tag
from package import add_bytes, add_file


def manifest(version: str) -> dict:
    return {
        "name": "agentdust",
        "version": version,
        "description": "Find and clean leftover processes from AI coding agents on Apple silicon.",
        "license": "MIT",
        "repository": {"type": "git", "url": "git+https://github.com/hamzahamidi/agentdust.git"},
        "homepage": "https://github.com/hamzahamidi/agentdust#readme",
        "bugs": {"url": "https://github.com/hamzahamidi/agentdust/issues"},
        "keywords": ["claude-code", "codex", "mcp", "process-cleanup", "macos"],
        "os": ["darwin"],
        "cpu": ["arm64"],
        "bin": {"agentdust": "bin/agentdust"},
        "files": ["bin/agentdust", "LICENSE", "README.md"],
        "publishConfig": {"access": "public", "registry": "https://registry.npmjs.org/"},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Package the release binary for npm without install scripts.")
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out-dir", required=True, type=Path)
    args = parser.parse_args()

    check_tag(f"v{args.version}", Path("Cargo.toml").read_text())
    reported = subprocess.check_output([str(args.binary.resolve()), "version"], text=True).strip()
    if reported != f"agentdust {args.version}":
        parser.error(f"binary reports {reported!r}, expected agentdust {args.version}")

    mtime = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        add_file(tar, Path("LICENSE"), "package/LICENSE", 0o644, mtime)
        add_file(tar, Path("README.md"), "package/README.md", 0o644, mtime)
        add_file(tar, args.binary, "package/bin/agentdust", 0o755, mtime)
        data = (json.dumps(manifest(args.version), indent=2, sort_keys=True) + "\n").encode()
        add_bytes(tar, data, "package/package.json", 0o644, mtime)

    args.out_dir.mkdir(parents=True, exist_ok=True)
    out = args.out_dir / f"agentdust-{args.version}-npm.tgz"
    with out.open("wb") as handle:
        with gzip.GzipFile(filename="", mode="wb", fileobj=handle, mtime=0, compresslevel=9) as gz:
            gz.write(raw.getvalue())
    print(f"{hashlib.sha256(out.read_bytes()).hexdigest()}  {out.name}")


if __name__ == "__main__":
    main()
