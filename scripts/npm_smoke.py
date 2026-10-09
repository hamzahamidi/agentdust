import argparse
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path

from brew_smoke import CLAUDE_STUB, run

CODEX_STUB = '''import json, os, sys
from pathlib import Path
state = Path(os.environ["STUB_CLAUDE_STATE"]) / "codex-server"
args = sys.argv[1:]
if args[:2] == ["mcp", "get"]:
    if not state.exists():
        print("Error: No MCP server named 'agentdust' found.", file=sys.stderr)
        sys.exit(1)
    command, *rest = json.loads(state.read_text())
    print(json.dumps({"enabled": True, "transport": {"type": "stdio", "command": command, "args": rest}}))
elif args[:2] == ["mcp", "add"]:
    state.write_text(json.dumps(args[args.index("--") + 1:]))
elif args[:2] == ["mcp", "remove"]:
    state.unlink()
else:
    sys.exit(64)
'''


def previous_package(package: Path, destination: Path) -> None:
    with tarfile.open(package) as source, tarfile.open(destination, "w:gz") as target:
        for member in source.getmembers():
            data = source.extractfile(member).read()
            if member.name == "package/package.json":
                document = json.loads(data)
                document["version"] = "0.0.0"
                data = json.dumps(document).encode()
                member.size = len(data)
            target.addfile(member, io.BytesIO(data))


def worker_restart(binary: Path, data: Path, env: dict) -> None:
    automatic = data / "automatic"
    automatic.mkdir(mode=0o700, exist_ok=True)
    policy = automatic / "policy.json"
    policy.write_text(json.dumps({"version": 1, "enabled": True, "projects": [], "keep": []}))
    policy.chmod(0o600)
    for _ in range(2):
        process = subprocess.Popen([str(binary), "auto", "worker"], env=env)
        try:
            deadline = time.monotonic() + 10
            while True:
                status = json.loads(subprocess.check_output([str(binary), "auto", "status"], env=env))
                if status["worker_running"]:
                    break
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("installed npm worker did not start")
                time.sleep(0.1)
        finally:
            process.terminate()
            process.wait(timeout=10)
    if (data / "audit.log").exists():
        raise RuntimeError("isolated npm worker unexpectedly wrote a signal audit")


def main() -> None:
    parser = argparse.ArgumentParser(description="Check npm global installation, upgrade, hooks and worker restart in isolation.")
    parser.add_argument("package", type=Path)
    parser.add_argument("version")
    args = parser.parse_args()
    package = args.package.resolve()
    with tempfile.TemporaryDirectory(prefix="agentdust-npm-") as temporary:
        root = Path(temporary).resolve()
        prefix = root / "npm prefix"
        stubs = root / "stubs"
        data = root / "data"
        for directory in (stubs, data, root / "home", root / "claude", root / "codex", root / "state"):
            directory.mkdir(mode=0o700)
        (stubs / "claude").write_text(CLAUDE_STUB)
        (stubs / "codex").write_text(f"#!{sys.executable}\n" + CODEX_STUB)
        for stub in stubs.iterdir():
            stub.chmod(0o755)
        env = {
            **os.environ,
            "HOME": str(root / "home"),
            "CLAUDE_CONFIG_DIR": str(root / "claude"),
            "CODEX_HOME": str(root / "codex"),
            "AGENTDUST_DATA_DIR": str(data),
            "STUB_CLAUDE_STATE": str(root / "state"),
            "PATH": f"{stubs}:{os.environ['PATH']}",
            "NPM_CONFIG_USERCONFIG": os.devnull,
            "NPM_CONFIG_CACHE": str(root / "cache"),
        }
        npm = shutil.which("npm")
        previous = root / "previous.tgz"
        previous_package(package, previous)
        install = [npm, "install", "--global", "--prefix", str(prefix), "--ignore-scripts", "--no-audit", "--no-fund"]
        run(*install, str(previous), env=env)
        binary = prefix / "bin/agentdust"
        if not binary.is_symlink():
            raise RuntimeError("npm did not link the native executable")
        for agent in ([], ["codex"]):
            run(str(binary), "setup", *agent, "--yes", env=env)
            run(str(binary), "setup", *agent, "--check", env=env)
        settings = (root / "claude/settings.json").read_bytes()
        hooks = (root / "codex/hooks.json").read_bytes()
        expected = str(binary)
        if (root / "state/server").read_text().splitlines() != [expected, "mcp"]:
            raise RuntimeError("Claude MCP registration did not use the global npm executable")
        if json.loads((root / "state/codex-server").read_text()) != [expected, "mcp"]:
            raise RuntimeError("Codex MCP registration did not use the global npm executable")
        if "lib/node_modules" in settings.decode() or "lib/node_modules" in hooks.decode():
            raise RuntimeError("hooks did not use the global npm executable")

        run(*install, str(package), env=env)
        if run(str(binary), "version", env=env) != f"agentdust {args.version}":
            raise RuntimeError("installed binary has another version")
        with tarfile.open(package) as tar:
            digest = hashlib.sha256(tar.extractfile("package/bin/agentdust").read()).digest()
        if hashlib.sha256(binary.read_bytes()).digest() != digest:
            raise RuntimeError("npm changed the packaged native binary")
        installed = json.loads((prefix / "lib/node_modules/agentdust/package.json").read_text())
        if installed["version"] != args.version:
            raise RuntimeError("npm did not upgrade the package version")
        for agent in ([], ["codex"]):
            run(str(binary), "setup", *agent, "--check", env=env)
        if settings != (root / "claude/settings.json").read_bytes() or hooks != (root / "codex/hooks.json").read_bytes():
            raise RuntimeError("npm upgrade changed the hook registrations")
        worker_restart(binary, data, env)
        for agent in ([], ["codex"]):
            run(str(binary), "setup", *agent, "--remove", "--yes", env=env)
        run(npm, "uninstall", "--global", "--prefix", str(prefix), "agentdust", env=env)
        if binary.exists() or binary.is_symlink():
            raise RuntimeError("npm uninstall left the executable link")
        print(f"npm {args.version} native install, upgrade, Claude and Codex setup, worker restart and removal passed")


if __name__ == "__main__":
    main()
