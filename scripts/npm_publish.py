import argparse
import base64
import hashlib
import json
import subprocess
import tarfile
from pathlib import Path

from check_release import tag_version
from npm_package import manifest


def validate(directory: Path, version: str) -> Path:
    tag_version(f"v{version}")
    release = json.loads((directory / "release.json").read_text())
    if release.get("tag_name") != f"v{version}" or release.get("draft") is not False or release.get("prerelease") is not False:
        raise ValueError("npm requires a public stable GitHub release with the requested tag")

    names = [f"agentdust-{version}-npm.tgz", f"agentdust-{version}-darwin-arm64.tar.gz"]
    checksums = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split()
        if name in checksums:
            raise ValueError(f"duplicate checksum for {name}")
        checksums[name] = digest
    for name in names:
        if hashlib.sha256((directory / name).read_bytes()).hexdigest() != checksums.get(name):
            raise ValueError(f"missing or mismatched checksum for {name}")

    package = directory / names[0]
    with tarfile.open(package) as tar:
        members = tar.getmembers()
        if sorted(item.name for item in members) != [
            "package/LICENSE", "package/README.md", "package/bin/agentdust", "package/package.json"
        ] or not all(item.isfile() for item in members):
            raise ValueError("npm archive must contain only the binary, manifest, README and license")
        document = json.load(tar.extractfile("package/package.json"))
        if document != manifest(version):
            raise ValueError("npm manifest does not match the release version and package contract")
        if tar.getmember("package/bin/agentdust").mode != 0o755:
            raise ValueError("npm binary must be executable")
        binary = tar.extractfile("package/bin/agentdust").read()
    with tarfile.open(directory / names[1]) as tar:
        native = tar.extractfile(f"agentdust-{version}-darwin-arm64/agentdust").read()
    if binary != native:
        raise ValueError("npm binary differs from the native release binary")
    return package


def publish(package: Path, version: str) -> None:
    result = subprocess.run(
        ["npm", "view", f"agentdust@{version}", "dist.integrity", "--json", "--registry=https://registry.npmjs.org/"],
        capture_output=True, text=True,
    )
    document = json.loads(result.stdout)
    if result.returncode == 0:
        integrity = "sha512-" + base64.b64encode(hashlib.sha512(package.read_bytes()).digest()).decode()
        if document != integrity:
            raise ValueError("npm already has this version with different package bytes")
        print(f"agentdust@{version} already has the verified package bytes; nothing to publish")
        return
    if not isinstance(document, dict) or document.get("error", {}).get("code") != "E404":
        raise ValueError(f"cannot check npm publication state: {result.stderr}")
    subprocess.run(
        ["npm", "publish", str(package.resolve()), "--access=public", "--ignore-scripts", "--registry=https://registry.npmjs.org/"],
        check=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Validate release artifacts and publish immutable npm package bytes.")
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    package = validate(args.directory, args.version)
    if not args.verify_only:
        publish(package, args.version)


if __name__ == "__main__":
    main()
