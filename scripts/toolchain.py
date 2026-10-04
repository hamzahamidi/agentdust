import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path


def output(*command: str) -> str:
    result = subprocess.run(command, capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else "unavailable"


def record() -> dict:
    return {
        "runner_image": f"{os.environ.get('ImageOS', 'local')} {os.environ.get('ImageVersion', '')}".strip(),
        "rustc": output("rustc", "-Vv"),
        "cargo": output("cargo", "-V"),
        "developer_dir": output("xcode-select", "-p"),
        "xcode": output("xcodebuild", "-version"),
        "sdk_version": output("xcrun", "--show-sdk-version"),
        "sdk_path": output("xcrun", "--show-sdk-path"),
        "cargo_lock_sha256": hashlib.sha256(Path("Cargo.lock").read_bytes()).hexdigest(),
        "rustflags": os.environ.get("RUSTFLAGS", ""),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Record or check the release toolchain.")
    parser.add_argument("mode", choices=["record", "check"])
    parser.add_argument("--expected", type=Path, default=Path("release/toolchain.json"))
    parser.add_argument("--allow-missing", action="store_true")
    parser.add_argument("--include-lock", action="store_true")
    args = parser.parse_args()

    current = record()
    if args.mode == "record":
        print(json.dumps(current, indent=2, sort_keys=True))
        return
    if args.allow_missing and not args.expected.exists():
        print(f"no expected toolchain at {args.expected}; nothing to compare")
        return
    expected = json.loads(args.expected.read_text())
    keys = set(expected) - {"cargo_lock_sha256"}
    if args.include_lock:
        keys.add("cargo_lock_sha256")
    keys = sorted(keys)
    drift = [key for key in keys if expected.get(key) != current.get(key)]
    for key in drift:
        print(f"toolchain drift in {key}:\n  expected: {expected.get(key)}\n  current:  {current.get(key)}", file=sys.stderr)
    sys.exit(1 if drift else 0)


if __name__ == "__main__":
    main()
