# M0 Cursor experiments

Two questions from section 10 of the design spec, answered with `scripts/m0/cursor_probe.py`. Raw entries are in `~/Library/Application Support/agentdust-m0/cursor-probe.jsonl`.

Cursor version: not installed on the test Mac on 2026-10-04. Both experiments move to M5, which installs Cursor and runs `scripts/m0/cursor_probe.py` as written. The hook script was checked against a scratch home directory: it logs the event and its process ancestry and returns the `AGENTDUST_M0_TAG` environment on `sessionStart`.

## 1. Does `sessionStart` env reach shell processes?

| Check | Result |
| --- | --- |
| `AGENTDUST_M0_TAG` printed by `env` in an agent shell command | not run |
| `tag_in_hook_env` in the `afterShellExecution` entry | not run |

Answer: open until M5.

## 2. Which process anchors a Cursor conversation?

| Event | Ancestry (pid and command, nearest first) |
| --- | --- |
| `sessionStart` | not run |
| `afterShellExecution` | not run |

| Check | Result |
| --- | --- |
| Lowest common ancestor of both chains | not run |
| Is that process still alive after the conversation is closed? | not run |
| Is the `sleep 600 &` child still alive after the conversation is closed, and what is its parent? | not run |

Answer: open until M5.
