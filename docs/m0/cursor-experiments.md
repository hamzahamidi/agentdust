# M0 Cursor experiments

Two questions from section 10 of the design spec, answered with `scripts/m0/cursor_probe.py`. Raw entries are in `~/Library/Application Support/agentdust-m0/cursor-probe.jsonl`.

Cursor version:

## 1. Does `sessionStart` env reach shell processes?

| Check | Result |
| --- | --- |
| `AGENTDUST_M0_TAG` printed by `env` in an agent shell command | |
| `tag_in_hook_env` in the `afterShellExecution` entry | |

Answer:

## 2. Which process anchors a Cursor conversation?

| Event | Ancestry (pid and command, nearest first) |
| --- | --- |
| `sessionStart` | |
| `afterShellExecution` | |

| Check | Result |
| --- | --- |
| Lowest common ancestor of both chains | |
| Is that process still alive after the conversation is closed? | |
| Is the `sleep 600 &` child still alive after the conversation is closed, and what is its parent? | |

Answer:
