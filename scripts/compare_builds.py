import argparse
import hashlib
import json
import sys
from pathlib import Path


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description="Compare two independent release builds.")
    parser.add_argument("first", type=Path)
    parser.add_argument("second", type=Path)
    args = parser.parse_args()

    binaries = [args.first / "target/release/agentdust", args.second / "target/release/agentdust"]
    toolchains = [json.loads((d / "toolchain.json").read_text()) for d in (args.first, args.second)]
    hashes = [digest(b) for b in binaries]
    for path, value in zip(binaries, hashes):
        print(f"{value}  {path}")
    failures = []
    if toolchains[0] != toolchains[1]:
        failures.append("the two builds recorded different toolchains")
    if hashes[0] != hashes[1]:
        failures.append("the two binaries differ")
    for failure in failures:
        print(failure, file=sys.stderr)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
