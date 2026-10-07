import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

TAP = "agentdust-local/m0"
CLAUDE_STUB = r'''#!/bin/sh
state="$STUB_CLAUDE_STATE"
[ "$1" = mcp ] || exit 64
case "$2" in
  get)
    if [ -f "$state/server" ]; then
      command=$(sed -n 1p "$state/server")
      args=$(sed -n 2p "$state/server")
      printf 'agentdust:\n  Scope: User config (available in all your projects)\n  Status: \342\234\230 Failed to connect\n  Type: stdio\n  Command: %s\n  Args: %s\n  Environment:\n\nTo remove this server, run: claude mcp remove agentdust -s user\n' "$command" "$args"
      exit 0
    fi
    echo 'No MCP server named "agentdust". Run `claude mcp add` to add one.' >&2
    exit 1
    ;;
  add)
    [ ! -f "$state/server" ] || exit 1
    shift 6
    printf '%s\n%s\n' "$1" "$2" > "$state/server"
    echo "Added stdio MCP server agentdust to user config"
    ;;
  remove)
    rm -f "$state/server"
    echo "Removed MCP server agentdust from user config"
    ;;
  *)
    exit 64
    ;;
esac
'''


def run(*command: str, env: dict[str, str] | None = None) -> str:
    print("+", " ".join(command), flush=True)
    result = subprocess.run(command, capture_output=True, text=True, env=env)
    if result.stdout:
        print(result.stdout, end="", flush=True)
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr, flush=True)
    if result.returncode:
        raise subprocess.CalledProcessError(
            result.returncode, command, output=result.stdout, stderr=result.stderr
        )
    return result.stdout.strip()


def verify_setup_and_apply(binary: Path, version: str) -> None:
    with tempfile.TemporaryDirectory(prefix="agentdust-homebrew-") as temporary:
        root = Path(temporary)
        home = root / "home"
        claude_config = root / "claude"
        data = root / "data"
        state = root / "claude-state"
        stubs = root / "stubs"
        for directory in (home, claude_config, data, state, stubs):
            directory.mkdir(parents=True)
        data.chmod(0o700)

        claude = stubs / "claude"
        claude.write_text(CLAUDE_STUB)
        claude.chmod(0o755)
        isolated_path = f"{stubs}:/usr/bin:/bin"
        setup_env = {
            "HOME": str(home),
            "CLAUDE_CONFIG_DIR": str(claude_config),
            "AGENTDUST_DATA_DIR": str(data),
            "STUB_CLAUDE_STATE": str(state),
            "PATH": isolated_path,
        }

        setup_started = time.perf_counter()
        setup = run(str(binary), "setup", "--yes", env=setup_env)
        setup_seconds = time.perf_counter() - setup_started
        if not all(marker in setup for marker in ("--- ", "+++ ", "SessionStart", "Setup finished")):
            sys.exit("setup did not print its expected diff and completion message")
        if max(setup.index("--- "), setup.index("+++ ")) > setup.index("Setup finished"):
            sys.exit("setup did not print its diff before the completion message")
        if setup_seconds >= 5:
            sys.exit(f"setup took {setup_seconds:.2f} seconds, expected under 5 seconds")
        print(f"Homebrew setup completed in {setup_seconds:.2f} seconds")
        run(str(binary), "setup", "--check", env=setup_env)
        settings = json.loads((claude_config / "settings.json").read_text())
        if "SessionStart" not in settings.get("hooks", {}):
            sys.exit("setup did not write the Claude Code session hook")
        server = (state / "server").read_text().splitlines()
        if server != [str(binary), "mcp"]:
            sys.exit(f"setup registered an unexpected MCP server: {server!r}")

        run("cargo", "build", "--release", "--locked", "-p", "agentdust-testkit", "--bin", "fixture-sleeper")
        target_dir = Path(os.environ.get("CARGO_TARGET_DIR", "target")).resolve()
        fixture = target_dir / "release" / "fixture-sleeper"
        test_env = os.environ.copy()
        test_env.update(
            {
                "HOME": str(home),
                "CLAUDE_CONFIG_DIR": str(claude_config),
                "AGENTDUST_DATA_DIR": str(data),
                "AGENTDUST_TEST_BINARY": str(binary),
                "AGENTDUST_FIXTURE_SLEEPER": str(fixture),
                "PATH": f"{isolated_path}:{os.environ.get('PATH', '')}",
            }
        )
        test_env.setdefault("CARGO_HOME", str(Path(os.environ["HOME"]) / ".cargo"))
        test_env.setdefault("RUSTUP_HOME", str(Path(os.environ["HOME"]) / ".rustup"))
        run(
            "cargo",
            "test",
            "--release",
            "--locked",
            "-p",
            "agentdust",
            "--test",
            "brew_apply",
            "--",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
            env=test_env,
        )
        print(f"Homebrew {version} install, isolated setup and approved apply passed")


def main() -> None:
    parser = argparse.ArgumentParser(description="Install an AgentDust formula and run its clean environment acceptance.")
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
        binary = Path(run("brew", "--prefix")) / "bin/agentdust"
        reported = run(str(binary), "version")
        expected = f"agentdust {args.version}"
        if reported != expected:
            sys.exit(f"expected {expected!r}, got {reported!r}")
        verify_setup_and_apply(binary, args.version)
    finally:
        subprocess.run(["brew", "uninstall", "--formula", "agentdust"], capture_output=True)
        subprocess.run(["brew", "untap", TAP], capture_output=True)


if __name__ == "__main__":
    main()
