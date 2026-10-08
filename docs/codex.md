# Codex support

AgentDust 1.3.0 adds Codex process tracking and cleanup after exact host exit on Apple silicon. The adapter targets native local hooks and shell markers in Codex 0.156.1. Cloud orchestration and remote shells are outside this support claim. Claude Code disk reports remain specific to Claude Code.

## Install and connect

```bash
agentdust setup codex --yes
agentdust setup codex --check
agentdust auto status
```

Setup shows a diff, installs six hooks in `$CODEX_HOME/hooks.json` (default `~/.codex/hooks.json`) and registers the local MCP server through the Codex CLI. Existing user hooks and MCP entries are preserved. Modified entries, conflicting MCP registrations, disabled hooks, inline hooks in `config.toml`, symlinks and unsafe files are refused rather than overwritten. Setup removal deletes only entries it created and still recognizes:

```bash
agentdust setup codex --remove --yes
```

Codex requires review and trust of non-managed hooks in `/hooks` before execution. Setup checks configuration, not trust. Review the six AgentDust definitions, then start a new chat so the configuration loads. AgentDust does not bypass or write Codex trust records.

After you authorize installation and directory scope, an AI agent with local shell access can run setup and batch enablement. The same existing policy applies to both supported clients:

```bash
agentdust auto enable --yes --projects /absolute/path/to/projects
```

Uncertain cleanup still requires human approval. Codex clients without usable MCP elicitation can use `agentdust apply` in a terminal. The optional skill package provides guidance, not a live connection.

## Ownership and liveness

Codex injects `CODEX_SESSION_ID` into local shell tools. It is the root session identity shared with descendant threads and matches hook `session_id`. `CODEX_THREAD_ID` can identify a child thread and is not substituted for the root session marker. AgentDust reads only named session markers, hashes the Codex marker with a separate HMAC domain and matches it to its local journal. Commands, tool output and whole environments are not stored. Session IDs are journal metadata, as with the Claude adapter.

The hook records session start and end, subagent start and stop, and paired Bash calls. It records the nearest supported native Codex ancestor's boot, PID, start time and user. Unknown ancestry cannot grant automatic cleanup. Repeated or resumed sessions retain their recorded hosts; a live host keeps matching helpers protected. A host change without a recorded root start is degraded evidence.

A native session marker provides attribution, not permission or liveness proof. The model's judgment does not replace the classifier. A process carrying both Claude and Codex markers is unverifiable because nested agents can inherit both. These cases are report-only. Markers can be deliberately copied by a same-UID actor; they are not an authentication boundary against that actor.

Every recorded owner process must be freshly gone before automatic cleanup. SessionEnd, Stop and SubagentStop do not replace kernel-owner death. SubagentStop reports a turn ending and the subagent can continue or resume.

| Runtime | Tracking | Automatic cleanup |
| --- | --- | --- |
| Dedicated local Codex runtime | Native hooks and session marker | Eligible after every recorded host exits, in an enabled directory |
| Desktop chats sharing a local server | Native hooks and session marker, when the desktop runtime exposes them | Waits for every recorded shared host to exit. Closing a chat alone is insufficient |
| Missing hooks, stripped marker or unrecognized host | Incomplete evidence | No automatic cleanup |
| Cloud or remote execution | Outside this adapter's scope | No support claim |

The executor retains fresh classification, exact target identity checks, keeps, pause, private directory policy and a durable attempt receipt before one SIGTERM. Process groups and SIGKILL are not used.

## Recorded checks

On the maintainer's Mac, Codex 0.156.1 emitted SessionStart, PreToolUse, PostToolUse and SessionEnd through the native hooks. Their session IDs matched the native shell marker and the hook identified the actual Codex host. Native Codex MCP installation, configuration check and removal passed in a private configuration directory.

The real-process worker fixtures passed normal and abrupt Codex host exit, live-owner protection, kept and unrelated helper protection, survivor handling and restart receipts. These are controlled fixtures. The background helper launched through the native Codex acceptance command did not survive that runtime, so this check does not claim reproduction of a natural Codex leak. Native desktop hook execution is unverified; the shared-host safety rule is covered by the liveness tests. The [acceptance record](codex/acceptance-0.156.1.json) separates those checks.

## Upstream contracts

- [Codex hook events and trust](https://learn.chatgpt.com/docs/hooks).
- [Codex 0.156.1 shell environment injection](https://github.com/openai/codex/blob/rust-v0.156.1/codex-rs/core/src/exec_env.rs).
- [Codex 0.156.1 hook dispatch](https://github.com/openai/codex/blob/rust-v0.156.1/codex-rs/core/src/hook_runtime.rs).

Immediate cleanup after one desktop chat ends while its shared server remains alive is a separate roadmap item. It needs a session revocation contract that protects resumed chats and live subagents.
