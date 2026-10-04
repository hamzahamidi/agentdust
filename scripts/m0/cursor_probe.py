import json
import os
import secrets
import subprocess
import sys
import time
from pathlib import Path

LOG = Path.home() / "Library/Application Support/agentdust-m0/cursor-probe.jsonl"
TAG = "AGENTDUST_M0_TAG"


def ancestry(pid: int) -> list[dict]:
    chain = []
    while pid > 1 and len(chain) < 12:
        line = subprocess.run(["ps", "-o", "ppid=,comm=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
        if not line:
            break
        ppid, comm = line.split(None, 1)
        chain.append({"pid": pid, "comm": comm})
        pid = int(ppid)
    return chain


def main() -> None:
    event = sys.argv[1] if len(sys.argv) > 1 else "unknown"
    payload = json.loads(sys.stdin.read() or "{}")
    LOG.parent.mkdir(parents=True, exist_ok=True)
    entry = {
        "ts": time.time(),
        "event": event,
        "session_id": payload.get("session_id"),
        "duration": payload.get("duration"),
        "tag_in_hook_env": os.environ.get(TAG),
        "ancestry": ancestry(os.getpid()),
    }
    with LOG.open("a") as handle:
        handle.write(json.dumps(entry) + "\n")
    if event == "sessionStart":
        print(json.dumps({"env": {TAG: f"probe-{secrets.token_hex(4)}"}}))


if __name__ == "__main__":
    main()
