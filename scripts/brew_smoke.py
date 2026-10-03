import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

TAP = "agentdust-local/m0"


def run(*command: str) -> str:
    print("+", " ".join(command), flush=True)
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout.strip()


def main() -> None:
    parser = argparse.ArgumentParser(description="Install a agentdust formula from a local tap and run it.")
    parser.add_argument("formula", type=Path)
    parser.add_argument("version")
    args = parser.parse_args()

    os.environ["HOMEBREW_NO_AUTO_UPDATE"] = "1"
    os.environ["HOMEBREW_NO_INSTALL_CLEANUP"] = "1"
    tap_dir = Path(run("brew", "--repository")) / "Library/Taps/agentdust-local/homebrew-m0/Formula"
    try:
        run("brew", "tap-new", "--no-git", TAP)
        tap_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy(args.formula, tap_dir / "agentdust.rb")
        run("brew", "trust", "--formula", f"{TAP}/agentdust")
        run("brew", "install", f"{TAP}/agentdust")
        reported = run(str(Path(run("brew", "--prefix")) / "bin/agentdust"), "version")
        expected = f"agentdust {args.version}"
        print(reported)
        if reported != expected:
            sys.exit(f"expected {expected!r}, got {reported!r}")
    finally:
        subprocess.run(["brew", "uninstall", "--formula", "agentdust"], capture_output=True)
        subprocess.run(["brew", "untap", TAP], capture_output=True)


if __name__ == "__main__":
    main()
