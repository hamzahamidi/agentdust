# AgentDust

[![CI](https://github.com/hamzahamidi/agentdust/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/hamzahamidi/agentdust/actions/workflows/ci.yml) [![Latest release](https://img.shields.io/github/v/release/hamzahamidi/agentdust)](https://github.com/hamzahamidi/agentdust/releases/latest) [![License](https://img.shields.io/github/license/hamzahamidi/agentdust)](LICENSE) [![Homebrew custom tap](https://img.shields.io/badge/Homebrew-custom%20tap-FBB040?logo=homebrew&logoColor=black)](https://github.com/hamzahamidi/homebrew-agentdust)

AgentDust is a macOS tool that finds processes left running after Claude Code and Codex sessions. Manual cleanup requires your typed approval. Version 1.1.0 adds opt-in automatic cleanup for proven leftovers in directories you enable. It also reports Claude Code disk usage without deleting files. Codex tracking uses native hooks and the shell session marker. Automatic cleanup waits for every recorded Codex host process to exit, including a shared desktop server. Cursor support is deferred.

**AgentDust 1.5.1 supports Claude Code and Codex process tracking on Apple silicon. Automatic cleanup results include their record time. Codex desktop cleanup waits for its shared host to exit. Captured fixtures cover the listed concurrent session, subagent and lifecycle scenarios on Claude Code 2.1.292, including an abrupt CLI exit. Agent Teams remain outside the support claim. The installed 1.0.0 Homebrew binary passed controlled human decline and typed approval checks on the maintainer's Mac ([release evidence](docs/v1/readiness.md)).** See [install and connect](#install-and-connect). To try the non-destructive approval flow, run the [development probe](#running-the-development-probe).

Agents start dev servers, MCP servers and helpers. When a session ends or crashes, some of them keep running under `launchd`, holding memory, ports and sometimes CPU. The upstream reports are open: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935) and [openai/codex#21008](https://github.com/openai/codex/issues/21008).

Security and privacy: [SECURITY.md](SECURITY.md), the [threat model](docs/threat-model.md) and the [section below](#security-and-privacy).

## Status

AgentDust 1.5.1 provides process analysis, opt-in automatic cleanup and approved manual cleanup for Claude Code and Codex, plus a read-only Claude Code disk report. Automatic cleanup results include the time they were recorded. Codex support retains the exact host-exit requirement; it does not stop helpers immediately when a desktop chat ends. Cursor remains deferred. The milestones are in [ROADMAP.md](ROADMAP.md).

Releases follow [Semantic Versioning](docs/release.md#semantic-versioning): compatible fixes increment the patch version, compatible features increment the minor version, and incompatible public API changes increment the major version.

| Part | State |
| --- | --- |
| TCP port diagnosis and recovery | `agentdust port PORT [--resolve] [--json]` and `agentdust_port`. Recovery uses the existing automatic policy ([scope and limits](docs/port-conflicts.md)) |
| Opt-in automatic cleanup | Available since `1.1.0`. Directory policy, owner-exit worker and read-only agent results with per-result record times ([commands and limits](docs/automatic-cleanup.md)) |
| Process identity, named session markers read from another process, `KERN_PROCARGS2` parser (fuzzed) | Built |
| Claude Code and Codex hooks that record session and shell events in a local journal | Built |
| `agentdust setup codex` | Six user hooks, native MCP registration, diff, consent, check and removal ([Codex support and limits](docs/codex.md)) |
| Journal rotation and retention, as library functions that nothing runs yet | Built |
| MCP approval probe: a typed-code form that changes nothing | Built |
| Reproducible release pipeline | The GitHub Actions dry run built two identical binaries, packaged a deterministic tarball, and verified the binary, tarball and SBOM attestations ([M0 report](docs/m0/report.md), [0.1.0 run](https://github.com/hamzahamidi/agentdust/actions/runs/37249711505)) |
| Homebrew distribution | Available from the [AgentDust tap](https://github.com/hamzahamidi/homebrew-agentdust) for macOS on Apple silicon |
| Release workflow: tag gate, audit, two builds, tarball, SBOM, attestation, draft release, formula | Built ([release process](docs/release.md)) |
| `agentdust setup` for Claude Code (hooks in `settings.json`, the MCP server through the `claude` CLI, a diff and consent, `--check`, `--remove`) and `agentdust status` | Built ([setup](docs/m3/setup.md)) |
| `agentdust doctor`, the MCP doctor and plan tools, and cleanup through the MCP form or terminal approval | Built for release 0.1.0 ([apply design and limits](docs/m3/apply.md)) |
| Read-only Claude Code disk report and skill | `agentdust disk [--json]`, `agentdust_disk` and `/agentdust:disk` ([scope and limits](docs/m7/disk.md)) |
| Optional Claude Code skills | Available as the `agentdust` plugin marketplace below |

## Resolve a busy TCP port

Ask Claude or Codex: “Check what holds port 3000 and clean up a proven leftover.” With the AgentDust cleanup skill available, when a server start it initiated fails with `EADDRINUSE`, the agent can diagnose the port, attempt automatic cleanup for proven leftovers under your existing enabled-project policy, and retry that same start once after a fresh scan shows no visible listener. The `agentdust_port` tool diagnoses listeners by default. With `resolve: true`, it attempts cleanup under your existing enabled-project policy, then checks listeners again. Uncertain actionable processes require typed approval through the existing plan/apply flow.

```bash
agentdust port 3000
agentdust port 3000 --resolve --json
```

A port number does not grant permission to stop its listener. Live, kept, unknown and managed processes remain protected. Results cover current-user-visible IPv4/IPv6 TCP listeners. `no_visible_listener` is an observation, not a guarantee that a bind will succeed. See [port conflict recovery](docs/port-conflicts.md).

## Connect Codex

```bash
agentdust setup codex --yes
agentdust setup codex --check
```

After setup, review and trust the six AgentDust hooks in Codex `/hooks`, then start a new chat. Codex requires this trust review before user hooks execute. An AI agent can perform setup after you authorize it; setup does not bypass Codex hook trust.

The same directory policy covers Claude Code and Codex. Dedicated Codex runtimes qualify for automatic cleanup after they exit. A desktop chat can share a live server with other chats, so its helpers stay protected until every recorded host has exited. Uncertain cases still require approval. See [Codex support and limits](docs/codex.md).

## Inspect Claude Code disk usage

```bash
agentdust disk
agentdust disk --json
```

In Claude Code, ask “Use AgentDust to show where Claude Code uses disk space”, or use `/agentdust:disk` from the optional plugin. The `agentdust_disk` tool reads metadata under the Claude configuration root and the current project's `.claude/worktrees`. It reports logical and allocated bytes by purpose, marks partial scans and deletes nothing. Allocated bytes are not reclaimable space. See [scope, categories and limits](docs/m7/disk.md).

## How cleanup works

1. `agentdust doctor` lists processes that look left behind and says why, with a class for each, such as owned by an ended session, suspect or unknown.
2. A plan keeps only processes owned by an ended session and suspects, at most 10 per call. You approve by typing a 4-character code that AgentDust generates: one code for the whole batch of owned processes, and one code per suspect with its evidence. The form is in your agent, or in the terminal with `agentdust apply`.
3. Before signalling each process, AgentDust classifies it again and compares boot session, PID, start time, user and executable path. It sends `SIGTERM` to that one PID and nothing else.

The design is in [docs/superpowers/specs](docs/superpowers/specs).

## Why I built AgentDust

My Mac was running hot, so I looked for leftovers from my AI coding sessions. Six leftover processes were 7 to 9 days old: two dev servers, a mock server, a browser driver with its MCP server and a stale Node script. Three held ports, and all six sat at 0% CPU, so they were not what heated the Mac, but nothing had told me they were there. Related reports are still open upstream: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935) and [openai/codex#21008](https://github.com/openai/codex/issues/21008).

Killing every process whose parent PID is 1 is unsafe. A background Node job showed a parent PID of 1 after 0.3 s while its agent was still running. Killing by name or by parent PID can kill live work.

[tidewake](https://github.com/berkkorkmaz/tidewake) is read-only: it audits the leftovers, shows the commands you could run and executes nothing. AgentDust records ownership while the agent runs, through the agent's hooks, and classifies each process. In the manual flow, I approve cleanup with a 4-character code that AgentDust generates, and it accepts no approval supplied through a model's tool call. Opt-in automatic cleanup uses a directory policy I enable locally and leaves uncertain cases for explicit approval. Before signalling, it checks the process identity again, then sends `SIGTERM` to that one PID and nothing else. The hook adds a median of 2.5 ms per event on a `macos-15` runner.

## Security and privacy

True of the code today:

- The release binary imports no socket calls, and the dependency tree has no networking crates.
- The journal keeps `session_id` and `tool_use_id` as supplied. It stores keyed digests of the working directory and new subagent IDs, never their raw values. Older version 1 records may contain raw subagent IDs. The digests use an HMAC under a random secret that stays in the data directory. A [privacy test](docs/m1/privacy-test.md) runs six hook events whose payloads carry sentinels in the command, a 5 MB response, the working directory, subagent ID and type, transcript paths, an unknown field and the environment, then reads every byte of every file in the data directory (secret, maintenance lock, journal and rotated copies) and every file the run left in the system temp directory. Only the session id may appear, and a journal key outside one list in the code fails the test. Another test sends a 9 MB tool response through the hook and checks that neither the output nor the command reaches the journal.
- `setup` never edits a symlinked, hard-linked or non-regular `settings.json`, writes the file by rename with its mode kept and no backup copy, removes only entries its manifest says it created and whose value still matches, and trusts the `claude` CLI only after `claude mcp get` shows the exact registration.
- The hook exits 0 and prints nothing, including on malformed input, an unwritable data directory and a data directory on a volume that is not local APFS, where it records nothing.
- The journal decoder, the journal reader and the hook payload parser are [fuzzed](docs/m1/fuzzing.md) beside the `KERN_PROCARGS2` parser. Each target runs under a counting allocator and fails when memory is not bounded by the input or by what the call kept, and the decoder target checks that a record appended after any bytes is read. The seeds are replayed by the test suite and by the Linux CI job, and property tests check that a short write costs the record it cut and no other.
- The live tests start real processes (some ignore SIGTERM, some detach into their own session, some outlive their parent) through a [harness](docs/m1/live-harness.md) that can only signal processes it started. It revalidates each one's identity before every signal and logs each signal sent.
- The classifier is checked against a [corpus of 22 fixtures](docs/fixtures.md) that carry the truth about each process: left behind by an ended session, live, detached, managed or unknown. It includes six protected cases (PID 1, another user, the agent itself, a launchd job, a Homebrew service, an app helper). The fixtures that can be started are started for real through the harness, and each one's parent, session and liveness are checked against the kernel.

Cleanup controls:

- Manual cleanup requires a code typed by you. Automatic cleanup requires an enabled local directory policy and freshly proven ended-session ownership. Suspects always require per-item typed approval.
- Each process is classified and its identity read again immediately before the signal.
- The only action is `SIGTERM` to one PID.

Report a vulnerability privately as described in [SECURITY.md](SECURITY.md).

## Using AgentDust 1.5.1

Release 1.5.1 supports Apple silicon with Claude Code. Homebrew and npm distribute the native binary. Automatic cleanup status includes per-result record times. Codex process tracking and automatic cleanup wait for every recorded host to exit. Codex desktop cleanup waits while a shared server remains alive. Automatic cleanup passed isolated normal-exit, abrupt-exit and worker-restart checks. The 1.0.0 Homebrew installation, setup, live session discovery and controlled human cleanup acceptance passed on the maintainer's Mac. The listed M6 lifecycle and attribution scenarios are captured and replayed for Claude Code 2.1.292. Agent Teams remain outside the support claim. See the [M6 fixture matrix](docs/m6/fixtures.md) for the tested scenarios and their limits.

### Install and connect

Install the prebuilt binary and connect it to Claude Code:

```bash
brew install hamzahamidi/agentdust/agentdust
agentdust setup
```

`brew install` installs the prebuilt binary. To verify the release artifacts, follow [these steps](docs/release.md#verify-a-release). `agentdust setup` shows a diff and asks before it changes anything ([details](#setting-up-claude-code)).

For releases with an npm package, npm installs the same native binary on macOS ARM64:

```bash
npm install -g agentdust
agentdust setup
```

The package contains the binary, README and license, with no JavaScript runtime dependency or install scripts. Installation does not register hooks or enable cleanup. Run `agentdust setup codex` to connect Codex, then review its hooks in Codex `/hooks`.

Use a global installation for hooks, MCP registration and the background worker. `npx` cache paths and project-local installs are not supported for persistent setup. Upgrading with `npm install -g agentdust@latest` keeps registrations valid within the same npm prefix. If a Node version manager changes that prefix, first pause automatic cleanup and remove each agent's setup from the old installation. Install under the new prefix, run setup for each agent again, review Codex hook trust, and run `agentdust auto resume` if cleanup was enabled. Choose one installation channel so the command and registered binary resolve to the same installation.

To uninstall an npm installation, run `agentdust auto disable`, remove setup for each connected agent (`agentdust setup --remove` and `agentdust setup codex --remove`), then run `npm uninstall -g agentdust`. The data directory stays until you delete it.

### Add the optional Claude Code skill

The MCP server exposes the tools. The optional plugin adds `/agentdust:cleanup` for processes and `/agentdust:disk` for read-only disk usage. It does not install the server or start cleanup by itself.

```bash
claude plugin marketplace add hamzahamidi/agentdust
claude plugin install agentdust@agentdust --scope user
```

Start a new Claude Code session, then run `/agentdust:cleanup` or ask Claude to inspect processes left by a Claude Code session. `agentdust_doctor` only reads the inventory. When you ask to clean up, the skill creates a fresh plan, shows its eligible items and evidence, then calls `agentdust_apply` to show AgentDust's typed approval form. Type the generated code yourself to approve. The skill never supplies an approval code. The plugin contains these two skills; `agentdust setup` separately registers the MCP server and hooks.

The plugin can also be reviewed or tested from a checkout of this repository with `claude plugin validate .`. See [Claude Code plugin installation](https://code.claude.com/docs/en/plugins/install) for install scopes and management.

### Find leftover processes

`agentdust doctor` lists the processes that look left behind by an agent session, each with a class and the evidence for it. `agentdust doctor --json` prints the same report as JSON. Neither changes anything.

| Class | Meaning | Can AgentDust signal it |
| --- | --- | --- |
| owned-ended | Started by an agent session that has ended | Yes, with one code for the batch, or automatically within enabled directory policy |
| suspect | Its parent is launchd or its launcher chain is dead, same user, old, idle and not managed | Yes, with one code per process |
| owned-live | Started by a session that is still running | Never |
| managed | A launchd job, a Homebrew service, a helper of a running app, or on the deny list | Never |
| unknown | Everything else | Never |

You can also ask the agent. Claude Code calls the `agentdust_doctor` tool and reports what it finds. The findings hold the class, the program name, the PID, the age and the kinds of evidence, and no command text or path.

### Clean up with your approval

Through the agent: ask Claude Code to clean up leftover processes. It calls `agentdust_plan`, which keeps only owned-ended and suspect processes, then `agentdust_apply`, which shows a form in Claude Code. The form lists the processes and a 4-character code that AgentDust generated. Type the code to approve. There is one code for the whole batch of owned-ended processes and one code per suspect, shown with its evidence. A call covers at most 10 items. A wrong or empty code, a decline, a cancel, and two minutes without an answer end the approval, and nothing is signalled. If the client cannot show the form, `agentdust_apply` returns `apply_not_supported` and points to the terminal command.

In a terminal: run `agentdust apply`. It runs the same steps in one process under the same rules. It refuses unless stdin, stdout and `/dev/tty` are terminals, the process is in the terminal's foreground, and no ancestor is a known agent, so run it from a terminal window that is not inside an agent. No flag or environment variable supplies a code.

Before each approved process is signalled, AgentDust classifies it again and compares boot session, PID, start time, user and executable path. It then sends `SIGTERM` to that one PID. It sends no other signal and never signals a process group. It waits up to 5 seconds and reports the process as terminated, a survivor, gone before the signal, or failed revalidation.

### Automatically clean proven leftovers

Automatic cleanup requires `1.1.0` or newer. Enable exact directories where you start Claude Code from your own foreground terminal:

```bash
agentdust version
agentdust auto enable /absolute/path/to/project /absolute/path/to/another-project
agentdust auto status
```

Review the directory scopes and type `ENABLE` once for the batch (binary `1.2.0` or newer). The local worker continues after normal or abrupt Claude Code exit. It signals only freshly proven owned-ended helpers belonging exclusively to an ended session in enabled scope. Kept, live, shared across sessions and uncertain cases remain untouched. Uncertain cases use the manual approval flow above.

`agentdust_auto_status` lets Claude explain worker state, recent outcomes and approval-required items. MCP status is read-only. With binary `1.2.0` or newer, a local AI agent can enable a user-authorized batch through `agentdust auto enable --yes --projects /absolute/path/to/projects`. This selects the parent and its immediate Git project directories, without recursive permission. The flag does not approve uncertain processes or remove keeps. Use `agentdust auto keep PID` for an intentional helper, `agentdust auto pause` to pause, `agentdust auto resume` to resume, or `agentdust auto disable` to clear permissions and remove the worker. See [commands, restart behavior and limits](docs/automatic-cleanup.md).

### Check, pause and remove

`agentdust status` shows the version, the data directory, the journal counters, whether setup is installed and whether apply is enabled. To turn cleanup off and keep everything else, put `apply = false` in `config.toml` in the data directory (`~/Library/Application Support/agentdust`). Before disconnecting Claude Code, run `agentdust auto disable` to remove the separately installed worker, then `agentdust setup --remove` and `brew uninstall agentdust`. The data directory stays until you delete it. If a release is withdrawn, see [SECURITY.md](SECURITY.md) and [docs/release.md](docs/release.md#roll-back).

## Setting up Claude Code

```bash
agentdust setup
agentdust status
```

`setup` prints a diff of `settings.json` and the `claude mcp add` command it will run, then asks before it writes anything (`--yes` approves without asking). It adds six hook entries for session start and end, subagent start and stop, and Bash tool start and end. It also registers the MCP server `agentdust`. It records what it added in a manifest, so `agentdust setup --remove` deletes only that, and `agentdust setup --check` exits 1 when the installation is missing or was modified. It does not edit a symlinked `settings.json`. A failed step is rolled back and the report says what was undone. The details are in [docs/m3/setup.md](docs/m3/setup.md).

## Running the development probe

Run the probe to check whether your agent client can collect a typed approval. It shows a typed-code form and reports how the client answered. It changes nothing on your machine. Building it needs Rust 1.99.0 (pinned in `rust-toolchain.toml`); the release candidate is a prebuilt binary that needs no Rust.

```bash
cargo build --release
```

Register it with the agent you use:

```bash
claude mcp add --scope user agentdust-probe -- "$PWD/target/release/agentdust" mcp
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
