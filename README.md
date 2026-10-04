# AgentDust

AgentDust is a macOS tool in development. It will find the processes that AI coding agents (Claude Code, Codex, Cursor) leave running after their sessions end, and stop them only after you approve each one with a typed code.

**There is no release yet. It cannot find or clean processes today.**

- Need cleanup now? It is not ready. Release 0.1 will cover Claude Code on Apple silicon and install through Homebrew. Watch the repository's releases.
- Want to help test? Run the non-destructive [approval probe](#running-the-development-probe).

Agents start dev servers, MCP servers and helpers. When a session ends or crashes, some of them keep running under `launchd`, holding memory, ports and sometimes CPU. The upstream reports are open: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935) and [openai/codex#21008](https://github.com/openai/codex/issues/21008).

Security and privacy: [SECURITY.md](SECURITY.md) and the [section below](#security-and-privacy).

## Status

Release 0.1 adds the analysis and the approved cleanup for Claude Code. The Codex and Cursor adapters follow, and 0.2 is the hardened beta. The milestones are in [ROADMAP.md](ROADMAP.md).

| Part | State |
| --- | --- |
| Process identity, one environment variable read from another process, `KERN_PROCARGS2` parser (fuzzed) | Built |
| Claude Code hook that records session and shell events in a local journal | Built |
| Journal rotation and retention, as library functions that nothing runs yet | Built |
| MCP approval probe: a typed-code form that changes nothing | Built |
| Reproducible release pipeline | Proven in a dry run: two identical binaries, a deterministic tarball, a verified attestation ([report](docs/m0/report.md)) |
| Homebrew distribution | Proven with a local tap in the dry run. No public tap or release yet |
| `doctor` (analysis), plan and apply (cleanup with approval) | Planned for 0.1 |

## How cleanup will work

1. `agentdust doctor` lists processes that look left behind and says why, with a class for each, such as owned by an ended session, suspect or unknown.
2. A plan keeps only processes owned by an ended session and suspects, at most 10 per call. You approve by typing a 4-character code that AgentDust generates: one code for the whole batch of owned processes, and one code per suspect with its evidence. The form is in your agent, or in the terminal with `agentdust apply`.
3. Before signalling each process, AgentDust classifies it again and compares boot session, PID, start time, user and executable path. It sends `SIGTERM` to that one PID and nothing else.

The design is in [docs/superpowers/specs](docs/superpowers/specs).

## Why AgentDust

A parent process of `launchd` does not mean a process was abandoned. Background jobs of a running agent are reparented to `launchd` within a fraction of a second: a background Node job showed a parent PID of 1 after 0.3 s while its agent was still running. AgentDust therefore records ownership while the agent runs, through the agent's hooks, and treats the parent PID as one signal among several.

Related tools exist. [tidewake](https://github.com/berkkorkmaz/tidewake) is a released, read-only audit for Claude Code and Codex that prints the commands to run and never deletes anything. AgentDust's aim is the step after the audit: cleanup that you approve per process class, with the process checked again at the moment of signalling. That is a design goal until 0.1 ships.

## Security and privacy

True of the code today:

- The release binary imports no socket calls, and the dependency tree has no networking crates.
- The journal holds event kinds, session and tool identifiers, timestamps, the boot session and a keyed digest of the working directory, never the path. The digest is an HMAC under a random secret that stays in the data directory. A test sends a 9 MB tool response through the hook and checks that neither the output nor the command reaches the journal. Another plants sentinel paths, commands, outputs and session tags and checks that none appears in any file of the data directory.
- The hook exits 0 and prints nothing, including on malformed input, an unwritable data directory and a data directory on a volume that is not local APFS, where it records nothing.
- The live tests start real processes (some ignore SIGTERM, some detach into their own session, some outlive their parent) through a [harness](docs/m1/live-harness.md) that can only signal processes it started. It revalidates each one's identity before every signal and logs each signal sent.

Design for 0.1, not implemented yet:

- No process is signalled without a code typed by you. AgentDust generates the code and shows it to you.
- Each process is classified and its identity read again immediately before the signal.
- The only action is `SIGTERM` to one PID.

Report a vulnerability privately as described in [SECURITY.md](SECURITY.md).

## Running the development probe

The probe is the one part you can run today. It shows a typed-code form in your agent and reports how the client answered. It changes nothing on your machine. Building it needs Rust 1.99.0 (pinned in `rust-toolchain.toml`); the 0.1 release will be a prebuilt binary that needs no Rust.

```bash
cargo build --release
```

Register it with the agent you use:

```bash
claude mcp add --scope user agentdust-probe "$PWD/target/release/agentdust" mcp
```

```bash
codex mcp add agentdust-probe "$PWD/target/release/agentdust" mcp
```

Ask the agent to call `agentdust_probe_approval`. The tool returns JSON such as `{"outcome": "approved", "protocol": "2025-06-18", "path": "legacy"}`. Remove the probe afterwards with `claude mcp remove --scope user agentdust-probe` or `codex mcp remove agentdust-probe`.

What has been checked, per client ([client matrix](docs/m0/client-matrix.md)):

- Codex CLI 0.156.1: the full typed-code flow, including a wrong code, Esc and a timeout.
- Claude Code 2.1.282: the server connects and lists its tool. The typed-code form has not been run yet.
- Cursor: not tested.

## Licence

[MIT](LICENSE)
