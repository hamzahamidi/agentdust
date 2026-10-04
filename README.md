# AgentDust

AgentDust is a macOS tool in development. It will find the processes that AI coding agents (Claude Code, Codex, Cursor) leave running after their sessions end, and stop them only after you approve each one with a typed code.

**There is no release yet. A development build can list leftover processes with `agentdust doctor`, which only reads. It cannot clean anything today.**

- Need cleanup now? It is not ready. Release 0.1 will cover Claude Code on Apple silicon and install through Homebrew. Watch the repository's releases.
- Want to help test? Run the non-destructive [approval probe](#running-the-development-probe).

Agents start dev servers, MCP servers and helpers. When a session ends or crashes, some of them keep running under `launchd`, holding memory, ports and sometimes CPU. The upstream reports are open: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935) and [openai/codex#21008](https://github.com/openai/codex/issues/21008).

Security and privacy: [SECURITY.md](SECURITY.md), the [threat model](docs/threat-model.md) and the [section below](#security-and-privacy).

## Status

Release 0.1 adds the analysis and the approved cleanup for Claude Code. The Codex and Cursor adapters follow, and 0.2 is the hardened beta. The milestones are in [ROADMAP.md](ROADMAP.md).

| Part | State |
| --- | --- |
| Process identity, one environment variable read from another process, `KERN_PROCARGS2` parser (fuzzed) | Built |
| Claude Code hook that records session and shell events in a local journal | Built |
| The hook records the agent process above it and hands each session a tag through `CLAUDE_ENV_FILE`. `session::scopes` derives unknown, active and ended sessions from the journal ([provenance](docs/m2/provenance.md)), and `doctor` reads them | Built |
| Journal rotation and retention, as library functions that nothing runs yet | Built |
| MCP approval probe: a typed-code form that changes nothing | Built |
| Reproducible release pipeline | Proven in a dry run: two identical binaries, a deterministic tarball, a verified attestation ([report](docs/m0/report.md)) |
| Homebrew distribution | Proven with a local tap in the dry run. No public tap or release yet |
| `agentdust doctor`: takes a snapshot of your processes, gives each a class (managed, owned by a live or an ended session, suspect, unknown), prints the counts and the findings, and says when owned classes are unavailable. It never signals ([doctor](docs/m2/doctor.md)) | Built |
| Plan and apply (cleanup with approval) | Planned for 0.1 |

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
- The journal holds event kinds, session and tool identifiers, timestamps, the boot session, the PID, start time, user and executable name of the agent process, a keyed digest of the working directory and, for a session start, a keyed digest of the session tag. It never holds a path or the raw tag, which is written only to the file that Claude Code names. The digests are HMACs under a random secret that stays in the data directory. A [privacy test](docs/m1/privacy-test.md) runs the hook under a fake agent process for four events whose payloads carry sentinels in the command, a 5 MB response, the working directory, a transcript path, an unknown field and the environment, then reads every byte of every file in the data directory (secret, maintenance lock, journal and rotated copies) and every file the run left in the system temp directory. The generated tag and the path of the agent executable must not appear anywhere in them either. Only the session id may appear, and a journal key outside one list in the code fails the test. Another test sends a 9 MB tool response through the hook and checks that neither the output nor the command reaches the journal.
- The hook exits 0 and prints nothing, including on malformed input, an unwritable data directory and a data directory on a volume that is not local APFS, where it records nothing.
- The journal decoder, the journal reader and the hook payload parser are [fuzzed](docs/m1/fuzzing.md) beside the `KERN_PROCARGS2` parser. Each target runs under a counting allocator and fails when memory is not bounded by the input or by what the call kept, and the decoder target checks that a record appended after any bytes is read. The seeds are replayed by the test suite and by the Linux CI job, and property tests check that a short write costs the record it cut and no other.
- The live tests start real processes (some ignore SIGTERM, some detach into their own session, some outlive their parent) through a [harness](docs/m1/live-harness.md) that can only signal processes it started. It revalidates each one's identity before every signal and logs each signal sent.
- The classifier is run over a [corpus of 22 fixtures](docs/fixtures.md) that carry the truth about each process: left behind by an ended session, live, detached, managed or unknown. It includes six protected cases (PID 1, another user, the agent itself, a launchd job, a Homebrew service, an app helper). The fixtures that can be started are started for real through the harness, each one's parent, session and liveness are checked against the kernel, and every observed process must get the class its label names. `agentdust doctor` never signals, never puts a command or a path in its JSON, and says when owned classes are unavailable ([doctor](docs/m2/doctor.md)).

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

## Running doctor

`doctor` is read-only. It takes about 2 seconds, because it samples CPU time twice, and it creates nothing in your data directory.

```bash
cargo build --release
./target/release/agentdust doctor
./target/release/agentdust doctor --json
```

The text report lists processes that belong to an ended session or look abandoned, with a redacted command and the directory of each. The JSON holds typed fields only: no command and no path. Without the hook installed the journal is empty, so owned classes are reported as unavailable and only suspects can appear.

## Licence

[MIT](LICENSE)
