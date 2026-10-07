# M6 real client fixtures

The synthetic cases in `crates/agentdust-core/tests/multi_agent_attribution.rs` check ownership decisions. This page records sanitized Claude Code hook fixtures. The capture helper writes privacy-filtered records to `events.jsonl`.

The capture helper accepts only `SessionStart`, `SessionEnd`, `SubagentStart`, `SubagentStop`, `PreToolUse` and `PostToolUse`. It HMACs session, subagent and tool IDs with a temporary key. It keeps only lifecycle source and reason enums, a short tool name allowlist, whether `agent_type` was present, and `stop_hook_active`. It drops commands, responses, messages, paths, raw IDs, agent type names and every unknown field. Input is capped at 64 KiB and the private JSONL output at 8 MiB.

## Capture

Create an isolated configuration with `python3 scripts/prepare_m6_capture.py /absolute/path/to/agentdust`. Source its `capture.env`, then start Claude Code with the generated settings and MCP files:

```sh
M6_DIR="$(python3 scripts/prepare_m6_capture.py "$(pwd)/target/release/agentdust")"
source "$M6_DIR/capture.env"
claude --settings "$M6_DIR/settings.json" --setting-sources project,local --mcp-config "$M6_DIR/mcp.json" --strict-mcp-config
```

The temporary config does not edit `~/.claude/settings.json`. `scripts/capture_claude_fixture.py` exits successfully and writes nothing when input, configuration or the key is invalid. Delete the temporary directory after reviewing and copying the sanitized fixture.

The `agentdust hook claude` handler also runs and writes to the temporary AgentDust journal under `data/`. That journal uses the normal schema and stores raw Claude session and tool-use IDs. The capture helper's filtering does not apply to this journal. Keep the capture directory local and delete it after the run; do not share `data/`.

1. Record `claude --version` and macOS version in the fixture manifest.
2. Capture three concurrent sessions, two active subagents, foreground and background subagents, resume, `/clear`, abrupt exit, and an Agent Team with two teammates.
3. Keep the original hook payloads in memory only. The JSONL file contains the normalized output from the capture helper.
4. Check the normalized file for private values, review it, then copy the approved fixture into `fixtures/m6/`.
5. Delete the temporary key, temporary settings and capture directory after the run.

Claude Code provides `agent_id` and `agent_type` on subagent hook calls. Record team events separately by session and process identity. Record the tested Claude Code versions because Agent Teams remains experimental. See the [hooks reference](https://code.claude.com/docs/en/hooks) and [Agent Teams documentation](https://code.claude.com/docs/en/agent-teams).

For the Agent Team scenario, set `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` and run Claude Code interactively. Claude Code does not start teammates in `-p` mode.

## Version policy

No Claude Code version range is declared for M6. A version can be listed as supported only after every required scenario below is captured and passes replay for that exact version. A fixture proves only the scenario it records.

The local CLI reported Claude Code 2.1.292 on macOS 26.6.2 arm64 on 2026-10-07. Sanitized captures cover three concurrent sessions, two background subagents, one foreground subagent, resume and `/clear`. Abrupt exit and the Agent Team scenario remain unrecorded, so 2.1.292 is not listed as supported. The 2.1.289 capture below is not a version-wide compatibility claim.

## Recorded clients

| Claude Code | macOS | Scenarios | Fixture |
| --- | --- | --- | --- |
| 2.1.289 | 26.6.2 | Two concurrently active subagents, each with one Bash helper call | [`claude-2.1.289-two-subagents.jsonl`](../../fixtures/m6/claude-2.1.289-two-subagents.jsonl) |
| 2.1.289 | 26.6.2 | Typed MCP approval | [M0 client matrix](../m0/client-matrix.md) |
| 2.1.292 | 26.6.2 | Two concurrently active background subagents, each with one Bash call | [`claude-2.1.292-two-background-subagents.jsonl`](../../fixtures/m6/claude-2.1.292-two-background-subagents.jsonl) |
| 2.1.292 | 26.6.2 | Three concurrent sessions with interleaved Bash events | [`claude-2.1.292-three-sessions.jsonl`](../../fixtures/m6/claude-2.1.292-three-sessions.jsonl) |
| 2.1.292 | 26.6.2 | One foreground subagent with one Bash call | [`claude-2.1.292-foreground-subagent.jsonl`](../../fixtures/m6/claude-2.1.292-foreground-subagent.jsonl) |
| 2.1.292 | 26.6.2 | Resumed session | [`claude-2.1.292-resume.jsonl`](../../fixtures/m6/claude-2.1.292-resume.jsonl) |
| 2.1.292 | 26.6.2 | `/clear` and the resulting new session | [`claude-2.1.292-clear.jsonl`](../../fixtures/m6/claude-2.1.292-clear.jsonl) |

The 2.1.289 capture produced two `SubagentStart` events before either helper call, paired `PreToolUse` and `PostToolUse` events for each agent, then two matching `SubagentStop` events. The fixture test replays the normalized records through the Claude adapter.

## Required scenario matrix

| Scenario | Status |
| --- | --- |
| Three concurrent sessions with interleaved shell and lifecycle events | Recorded on 2.1.292 |
| Two active subagents in one session, with one Bash call each | Recorded on 2.1.289 and 2.1.292 |
| Foreground subagent | Recorded on 2.1.292 |
| Background subagent | Recorded on 2.1.292 |
| Resume | Recorded on 2.1.292 |
| `/clear` | Recorded on 2.1.292 |
| Abrupt exit | Unrecorded |
| Agent Team with two teammates | Unrecorded |

The 2.1.292 hook fixtures replay through the Claude adapter test. Synthetic tests cover owner liveness, unrelated sessions, shared ownership and apply races. Real hook fixtures do not replace those process identity and signalling checks.

The fixture matrix remains incomplete. M6 also needs one machine to complete at least two weeks of dogfood with attribution errors, false proposals, blocked applies, signals, failures and hook latency recorded. See the [dogfood record](dogfood.md).
