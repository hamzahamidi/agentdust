# AgentDust

AgentDust is a macOS tool in development that finds processes left running after Claude Code sessions and stops them only after you approve each one with a typed code. Codex and Cursor support are planned for later releases.

**No release has been published yet. A development build can inspect processes and run approved cleanup.**

- Need a prebuilt install? Release 0.1 will cover Claude Code on Apple silicon and install through Homebrew. Watch the repository's releases.
- Want to help test? Run the non-destructive [approval probe](#running-the-development-probe).

Agents start dev servers, MCP servers and helpers. When a session ends or crashes, some of them keep running under `launchd`, holding memory, ports and sometimes CPU. The upstream reports are open: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935) and [openai/codex#21008](https://github.com/openai/codex/issues/21008).

Security and privacy: [SECURITY.md](SECURITY.md), the [threat model](docs/threat-model.md) and the [section below](#security-and-privacy).

## Status

Release 0.1 adds the analysis and the approved cleanup for Claude Code. The Codex and Cursor adapters follow, and 0.2 is the hardened beta. The milestones are in [ROADMAP.md](ROADMAP.md).

| Part | State |
| --- | --- |
| Process identity, one environment variable read from another process, `KERN_PROCARGS2` parser (fuzzed) | Built |
| Claude Code hook that records session and shell events in a local journal | Built |
| Journal rotation and retention, as library functions that nothing runs yet | Built |
| MCP approval probe: a typed-code form that changes nothing | Built |
| Reproducible release pipeline | Proven in a dry run: two identical binaries, a deterministic tarball, a verified attestation ([report](docs/m0/report.md)) |
| Homebrew distribution | Tap repository exists. The first release adds the public formula |
| Release workflow: tag gate, audit, two builds, tarball, SBOM, attestation, draft release, formula | Built. The tagged release workflow has not run on GitHub yet ([release process](docs/release.md)) |
| `agentdust setup` for Claude Code (hooks in `settings.json`, the MCP server through the `claude` CLI, a diff and consent, `--check`, `--remove`) and `agentdust status` | Built ([setup](docs/m3/setup.md)) |
| `agentdust doctor`, the MCP doctor and plan tools, and cleanup through the MCP form or terminal approval | Built, not released ([apply design and limits](docs/m3/apply.md)) |

## How cleanup works

1. `agentdust doctor` lists processes that look left behind and says why, with a class for each, such as owned by an ended session, suspect or unknown.
2. A plan keeps only processes owned by an ended session and suspects, at most 10 per call. You approve by typing a 4-character code that AgentDust generates: one code for the whole batch of owned processes, and one code per suspect with its evidence. The form is in your agent, or in the terminal with `agentdust apply`.
3. Before signalling each process, AgentDust classifies it again and compares boot session, PID, start time, user and executable path. It sends `SIGTERM` to that one PID and nothing else.

The design is in [docs/superpowers/specs](docs/superpowers/specs).

## Why I built AgentDust

My Mac was running hot, so I looked for leftovers from my AI coding sessions. Six processes had outlived their sessions by 7 to 9 days: two dev servers, a mock server, a browser driver with its MCP server and a stale Node script. Three held ports, and all six sat at 0% CPU, so they were not what heated the Mac, but nothing had told me they were there. Related reports are still open upstream: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935) and [openai/codex#21008](https://github.com/openai/codex/issues/21008).

Killing every process whose parent PID is 1 is unsafe. A background Node job showed a parent PID of 1 after 0.3 s while its agent was still running. Killing by name or by parent PID can kill live work.

[tidewake](https://github.com/berkkorkmaz/tidewake) is read-only: it audits the leftovers, shows the commands you could run and executes nothing. AgentDust records ownership while the agent runs, through the agent's hooks, and classifies each process. It stops one only after I approve it with a 4-character code that AgentDust generates, and it accepts no approval supplied through a model's tool call. Before signalling, it checks the process identity again, then sends `SIGTERM` to that one PID and nothing else. The hook adds a median of 2.5 ms per event on a `macos-15` runner.

## Security and privacy

True of the code today:

- The release binary imports no socket calls, and the dependency tree has no networking crates.
- The journal holds event kinds, session and tool identifiers, timestamps, the boot session and a keyed digest of the working directory, never the path. The digest is an HMAC under a random secret that stays in the data directory. A [privacy test](docs/m1/privacy-test.md) runs the hook for four events whose payloads carry sentinels in the command, a 5 MB response, the working directory, a transcript path, an unknown field and the environment, then reads every byte of every file in the data directory (secret, maintenance lock, journal and rotated copies) and every file the run left in the system temp directory. Only the session id may appear, and a journal key outside one list in the code fails the test. Another test sends a 9 MB tool response through the hook and checks that neither the output nor the command reaches the journal.
- `setup` never edits a symlinked, hard-linked or non-regular `settings.json`, writes the file by rename with its mode kept and no backup copy, removes only entries its manifest says it created and whose value still matches, and trusts the `claude` CLI only after `claude mcp get` shows the exact registration.
- The hook exits 0 and prints nothing, including on malformed input, an unwritable data directory and a data directory on a volume that is not local APFS, where it records nothing.
- The journal decoder, the journal reader and the hook payload parser are [fuzzed](docs/m1/fuzzing.md) beside the `KERN_PROCARGS2` parser. Each target runs under a counting allocator and fails when memory is not bounded by the input or by what the call kept, and the decoder target checks that a record appended after any bytes is read. The seeds are replayed by the test suite and by the Linux CI job, and property tests check that a short write costs the record it cut and no other.
- The live tests start real processes (some ignore SIGTERM, some detach into their own session, some outlive their parent) through a [harness](docs/m1/live-harness.md) that can only signal processes it started. It revalidates each one's identity before every signal and logs each signal sent.
- The classifier is checked against a [corpus of 22 fixtures](docs/fixtures.md) that carry the truth about each process: left behind by an ended session, live, detached, managed or unknown. It includes six protected cases (PID 1, another user, the agent itself, a launchd job, a Homebrew service, an app helper). The fixtures that can be started are started for real through the harness, and each one's parent, session and liveness are checked against the kernel.

Cleanup controls:

- No process is signalled without a code typed by you. AgentDust generates the code and shows it to you.
- Each process is classified and its identity read again immediately before the signal.
- The only action is `SIGTERM` to one PID.

Report a vulnerability privately as described in [SECURITY.md](SECURITY.md).

## Using AgentDust (from release 0.1)

Release 0.1 is not published. This section describes how 0.1 works on Apple silicon with Claude Code. The Status table above shows which parts the code has today.

### Install and connect

```bash
brew install hamzahamidi/agentdust/agentdust
agentdust setup
```

`brew install` pours the prebuilt binary. Check what you downloaded with `gh attestation verify` ([how](docs/release.md#verify-a-release)). `agentdust setup` shows a diff and asks before it changes anything ([details](#setting-up-claude-code)).

### Find leftover processes

`agentdust doctor` lists the processes that look left behind by an agent session, each with a class and the evidence for it. `agentdust doctor --json` prints the same report as JSON. Neither changes anything.

| Class | Meaning | Can AgentDust signal it |
| --- | --- | --- |
| owned-ended | Started by an agent session that has ended | Yes, with one code for the batch |
| suspect | Its parent is launchd or its launcher chain is dead, same user, old, idle and not managed | Yes, with one code per process |
| owned-live | Started by a session that is still running | Never |
| managed | A launchd job, a Homebrew service, a helper of a running app, or on the deny list | Never |
| unknown | Everything else | Never |

You can also ask the agent. Claude Code calls the `agentdust_doctor` tool and reports what it finds. The findings hold the class, the program name, the PID, the age and the kinds of evidence, and no command text or path.

### Clean up with your approval

Through the agent: ask Claude Code to clean up leftover processes. It calls `agentdust_plan`, which keeps only owned-ended and suspect processes, then `agentdust_apply`, which shows a form in Claude Code. The form lists the processes and a 4-character code that AgentDust generated. Type the code to approve. There is one code for the whole batch of owned-ended processes and one code per suspect, shown with its evidence. A call covers at most 10 items. A wrong or empty code, a decline, a cancel, and two minutes without an answer end the approval, and nothing is signalled. If the client cannot show the form, `agentdust_apply` returns `apply_not_supported` and points to the terminal command.

In a terminal: run `agentdust apply`. It runs the same steps in one process under the same rules. It refuses unless stdin, stdout and `/dev/tty` are terminals, the process is in the terminal's foreground, and no ancestor is a known agent, so run it from a terminal window that is not inside an agent. No flag or environment variable supplies a code.

Before each approved process is signalled, AgentDust classifies it again and compares boot session, PID, start time, user and executable path. It then sends `SIGTERM` to that one PID. It sends no other signal and never signals a process group. It waits up to 5 seconds and reports the process as terminated, a survivor, gone before the signal, or failed revalidation.

### Check, pause and remove

`agentdust status` shows the version, the data directory, the journal counters, whether setup is installed and whether apply is enabled. To turn cleanup off and keep everything else, put `apply = false` in `config.toml` in the data directory (`~/Library/Application Support/agentdust`). To disconnect Claude Code run `agentdust setup --remove`, then `brew uninstall agentdust`. The data directory stays until you delete it. If a release is withdrawn, see [SECURITY.md](SECURITY.md) and [docs/release.md](docs/release.md#roll-back).

## Setting up Claude Code

```bash
agentdust setup
agentdust status
```

`setup` prints a diff of `settings.json` and the `claude mcp add` command it will run, then asks before it writes anything (`--yes` approves without asking). It adds four hook entries (`SessionStart`, `SessionEnd`, and `PreToolUse` and `PostToolUse` for `Bash`) and registers the MCP server `agentdust`. It records what it added in a manifest, so `agentdust setup --remove` deletes only that, and `agentdust setup --check` exits 1 when the installation is missing or was modified. It does not edit a symlinked `settings.json`. A failed step is rolled back and the report says what was undone. The details are in [docs/m3/setup.md](docs/m3/setup.md).

## Running the development probe

Run the probe to check whether your agent client can collect a typed approval. It shows a typed-code form and reports how the client answered. It changes nothing on your machine. Building it needs Rust 1.99.0 (pinned in `rust-toolchain.toml`); the 0.1 release will be a prebuilt binary that needs no Rust.

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
- Claude Code 2.1.289: the full typed-code form was tested, including accept, wrong code, decline, cancel, timeout and a forged retry. No automatic approval was observed.
- Cursor: not tested.

## Licence

[MIT](LICENSE)
