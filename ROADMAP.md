# AgentDust roadmap

The binary is `agentdust`. M0 is complete with the Cursor coverage gap recorded in [the M0 report](docs/m0/report.md). M1 is complete, with its journal decision recorded in [ADR-1](docs/m1/adr-journal-format.md). M2 and M3 shipped in the [0.1.0 release](https://github.com/hamzahamidi/agentdust/releases/tag/v0.1.0). The Homebrew formula is not on the tap's default branch. Verify that install path before expanding the release matrix. Design: [the design spec](docs/superpowers/specs/2026-10-03-agentdust-design.md).

## Goal

A macOS developer who uses Claude Code, Codex and Cursor installs the tool in two commands. Any of those agents can then analyse leftover processes and disk growth, show the findings, and clean stale processes after the user approves a typed code.

## Principles

- Release trust, security testing and distribution start in M0, not at the end.
- Only owned-ended and suspect processes can ever be signalled. Suspects need their own approval, and managed or unknown processes are never signalled.
- Evidence the tool stores is minimised at ingestion. Raw commands, command output and process environments are never persisted.
- Scope is cut before estimates are tightened. The first release is narrow and complete.

## v1.0 success criteria

1. Install: `brew install` pours a prebuilt binary and never compiles. `agentdust setup` finishes in under 5 seconds on a declared fixture account and prints a diff before changing anything.
2. Hook cost: p50 under 10 ms and p95 under 20 ms on the release binary, including three agents writing the journal at once.
3. Idle: the MCP server does no periodic work when idle. CPU over a 60 second idle window stays under 0.1%. Resident memory is reported per release (target under 10 MB).
4. Actionability: `apply` rejects managed and unknown items, even when the model supplies their ID. Owned-ended items can be approved as one batch with one typed code. Every suspect item needs its own typed code, shows its evidence, and is never signalled as part of a process group.
5. Classifier: fixtures carry ground-truth labels (`true_owned_ended`, `true_live_owned`, `true_detached`, `true_managed`, `true_unknown`). No fixture outside `true_owned_ended` is classified owned-ended, no `true_managed` or `true_unknown` fixture is ever actionable, and every `true_owned_ended` fixture is classified owned-ended.
6. Privacy: fixture secrets never appear anywhere under the AgentDust data directory, checked by a test.
7. Supply chain: dependencies are locked from the first commit. Each release ships a checksum, an artifact attestation, an SBOM, and two independent macOS arm64 builds that produce a byte-identical binary.
8. Evidence: at least three independent machines run 0.x builds for two weeks, with sessions, proposed kills, false positives and failures counted.

## Decisions made

| Area | Decision |
| --- | --- |
| v1 scope | Process hygiene end to end, plus a read-only disk report |
| Language | Rust, one binary, Tokio only in the MCP subcommand |
| Interface | One MCP server with `agentdust_doctor`, `agentdust_plan`, `agentdust_apply`, plus a CLI |
| Agents | Claude Code, Codex and Cursor, delivered one at a time in that order |
| Provenance | Layered evidence: event-driven process sampling, a journal of paired shell calls per adapter, and an environment tag on Claude Code as additive evidence only |
| Approval | MCP elicitation with a typed one-time code, fail closed |
| Plan state | Canonical plan in server memory with an opaque ID. `plan.json` is for inspection. A server restart invalidates plans |
| Actions in 0.1 | SIGTERM to one exactly revalidated PID. No process-group signals, no SIGKILL |
| Actionable classes | Owned-ended (one batch code) and suspect (one typed code per item). At most 10 items per apply call in total. Managed and unknown are never actionable. Likely-owned stays report-only until its precision is measured |
| Install | Homebrew formula that installs the attested prebuilt binary, then `agentdust setup` |
| Platform | macOS arm64 first |
| Journal | One O_APPEND file framed with RS, no lock for appenders and readers, an append recheck, 64 KiB frame cap, local APFS only |

## Open decisions

1. Suspect rules: the exact age, idleness and launcher-chain thresholds, set from the M1 fixture corpus.

## Milestones

Sizes are rough: S about 3 days, M about 1 week, L about 2 weeks. Validation work, not code, dominates every milestone.

### M0 Risk spike and release skeleton (M)

- Hook binary that reads JSON on stdin and appends one event.
- `KERN_PROCARGS2` parser with a fuzz target, reading one named variable from another process.
- Process start time and process group for a live PID.
- MCP stdio server running a real typed-code elicitation on Claude Code, Codex and Cursor, with a recorded matrix of client versions and behaviours (empty form, auto-accept, decline, cancel, timeout).
- Release skeleton on a public-repo macOS arm64 runner: `cargo build --release --locked`, two clean builds compared byte for byte, tarball, checksum, attestation, and a minimal Homebrew formula that installs it and runs `agentdust version`.
- Repository basics: licence, SECURITY.md, squash-only merge settings, dependency policy (`cargo deny`, `cargo audit`, no unpinned git dependencies), toolchain and SDK versions recorded.

Exit: hook p50 under 10 ms on the real binary, the elicitation matrix recorded, and the build, attestation and Homebrew install proven or their blockers written down. No public release.

### M1 Safety foundation and test harness (L)

- Process identity (PID, start time, UID, executable), with property tests.
- Journal: minimal fields only (session ID, wall and monotonic timestamps, keyed working directory digest, tool-call ID, executable basename), file permissions 0600, retention and rotation, schema version that fails closed on unknown versions, corrupt-state recovery.
- The 3 and 16 writer benchmark that picks the ledger format.
- Written threat model covering a malicious model, malicious process metadata, a buggy MCP client, same-user tampering, PID reuse, concurrent apply, stale plans and a compromised release artifact.
- Live process-tree test harness that spawns real fixture processes (cooperative and SIGTERM-ignoring children, detached sessions, children that exit during approval) and only signals PIDs it created.
- Fixture schema with ground-truth labels.

Exit: primitives and storage survive unit, property, fuzz and concurrency tests, and the privacy test passes.

### M2 Claude Code doctor and provenance (L)

- Process inventory and classifier (owned-ended, likely-owned, suspect, managed, unknown). Listening-port evidence is deferred.
- Suspect rules: parent is launchd or the launcher chain is dead, same UID, older than the age threshold, idle, not managed by launchd or Homebrew services, and not on a deny list (system processes, the agent that is asking, the AgentDust server itself).
- Claude Code adapter: `SessionStart` environment tag through `CLAUDE_ENV_FILE`, `PreToolUse` and `PostToolUse` with a Bash matcher paired by `tool_use_id`, event-driven process sampling.
- Acceptance cases: fresh session, resumed session, after `/clear`, subagent Bash, `CLAUDE_ENV_FILE` missing, abrupt termination, normal session end.
- Sanitised, typed, redacted `doctor` output.

Exit: controlled Claude fixtures whose session has ended become owned-ended and no protected or non-owned fixture does.

### M3 Plan, apply and Homebrew: release 0.1 (L)

- `agentdust_plan` and `agentdust_apply` with typed-code elicitation, atomic per-item claim, revalidation after approval, SIGTERM to one PID, survivor report, audit log (0600, bounded size).
- Approval flow: one batch code for owned-ended items, then one code per suspect item with its evidence rendered from a fixed template. A deliberately detached process such as a tunnel can be classified suspect, so the per-item prompt is the control.
- Fail-closed behaviour on decline, cancel, timeout, unsupported clients and corrupt state.
- Terminal `agentdust apply` with the same rules, refusing without a foreground terminal or under an agent ancestor.
- `agentdust setup` for Claude Code: full diff, consent, surgical edits, idempotent, `--check`, `--remove`, ownership manifest, rollback report for partial failure.
- Homebrew formula, `agentdust support-bundle` (a local file, never uploaded), README, security and privacy documents.

Exit: on a clean account, install, setup, a controlled stale process found by a Claude Code session, typed approval, and exactly that process receives SIGTERM.

### M4 Codex adapter and setup (L)

- `hooks.json` under `CODEX_HOME` with `PreToolUse` and `PostToolUse` Bash matchers, MCP registration through `codex mcp add`.
- Its own acceptance matrix and setup rollback tests. The correlation engine ships here as likely-owned evidence only.

### M5 Cursor adapter and setup (L)

- `~/.cursor/hooks.json` with `afterShellExecution`, MCP registration.
- Its own acceptance matrix and setup rollback tests.

### M6 Hardening and beta: release 0.2 (L)

- Broader fixture corpus, race tests, host and MCP version drift tests, `unsafe` review, mixed-version process handling after `brew upgrade`.
- Measured dogfood on the first machine.

### M7 Read-only disk report (M)

- Inventory of known agent roots, each classified rebuildable, history, worktree, application state or unknown. Logical and allocated size are reported separately. Nothing is deleted.

### M8 Plugins, listings and upstream (S, external latency)

- Thin plugins for the three agents. Submission to Anthropic's directory and the Cursor marketplace. One technical comment on anthropics/claude-code#1935. A request for a lifecycle event carrying the PID and session of spawned processes.

Exit: submitted and validated locally. Acceptance by third parties is not a gate.

### M9 1.0 readiness

- Criterion 8 met. Process-group signals and SIGKILL are added only if real failures justify them.

## Platform expansion after v1

The current release target is macOS arm64 with Claude Code. Finish the Codex and Cursor adapter matrices in M4 and M5 on that target before adding another operating system. Close the Homebrew install gap and verify installation from a clean account before expanding the release matrix.

### M10 Intel macOS (M)

* Build `x86_64-apple-darwin` beside the current Apple silicon target.
* Run process identity, PID reuse, privacy, setup and live apply checks on Intel Mac hardware.
* Update the release workflow and Homebrew formula to select the correct archive for each architecture. Attest each binary and publish its checksum and SBOM.

Exit: a clean Intel Mac installs the formula, runs setup, finds controlled stale processes, completes typed approval, and signals only the process approved.

### M11 Linux x86_64 with Claude Code (L)

* Add a Linux `ProcessProvider` that reads PID, start time, UID, executable path and boot identity from Linux process interfaces. Revalidate the same identity before signalling.
* Treat unreadable or changing `/proc` data as unavailable evidence. It must not make a process more actionable.
* Add fixtures for PID reuse, reparenting, systemd services, containers, permission denial and processes that outlive a session. Keep managed and unknown processes non actionable.
* Define which local file systems support the journal guarantees. Refuse a data directory on a file system that has not passed the journal and privacy checks.
* Add Claude Code setup and hook coverage on Ubuntu LTS. Publish an `x86_64-unknown-linux-gnu` release archive with checksum, SBOM and build attestation. Add a Linux package manager after users ask for one.

Exit: the Linux fixtures preserve the classifier safety criteria, live process tests pass on Ubuntu LTS, the clean account flow completes, and the release artifact installs without compiling Rust.

### M12 WSL2 with Claude Code (M)

* Run AgentDust and Claude Code inside the same WSL2 distribution. Use the Linux process provider and release archive.
* Keep process discovery and signalling inside that distribution. Do not inspect or signal Windows host processes.
* Check hook setup, journal permissions, terminal approval and MCP approval in a clean WSL2 distribution. Reject data locations whose file system does not meet the journal guarantees.

Exit: the full setup, doctor, plan and approved apply flow passes in a clean WSL2 distribution, and tests prove that a Windows host process cannot become actionable.

### Native Windows

Native Windows is not scheduled. Start it only after user requests justify a separate Windows process provider, identity revalidation, fixture harness, setup path and release artifact. WSL2 support does not count as native Windows support.

## Not in 0.1

Codex, Cursor, disk report, process-group signals, SIGKILL, listening ports, persisted plans, plugins, upstream outreach.

## Not in v1

Disk deletion or quarantine (1.x, rebuildable caches only), Linux and Windows, daemons, scheduled cleanup, self-update, any network access, remote MCP transport.

## Risks

| Risk | Mitigation |
| --- | --- |
| MCP clients differ in elicitation behaviour | M0 matrix, pinned SDK, typed-code regression tests, refuse `apply` on any failing client |
| `CLAUDE_ENV_FILE` is missing on resume, `/clear` or in plugin hooks | Tag is additive evidence only, with explicit acceptance cases |
| Hook latency grows under journal contention | 3 and 16 writer benchmark in M1, hard budget in CI |
| Weaker provenance for Codex and Cursor | Correlation evidence is likely-owned and report-only until its precision is measured |
| A suspect is a deliberately detached process | Evidence shown per item, one typed code per item, SIGTERM only, audit log, deny list, false positives counted in the 1.0 evidence |
| Secrets in commands reach the journal | Ingestion minimisation, privacy invariant test |
| Config patching damages a user file | Surgical edits, diff plus consent, per-product rollback, `--check`, `--remove` |
| Release trust discovered too late | Release skeleton and reproducibility experiment in M0 |
| Host formats change | Versioned adapter code tested against old and current client versions |

## Cross-cutting rules

- Test layers: unit and property tests, recorded fixtures, live process-tree tests, MCP contract tests, clean-account install tests, release-artifact smoke tests.
- One CI run per change, no scheduled workflows. Linux jobs for lint and unit tests, one macOS job for build, hook benchmark and live tests.
- No new runtime dependency without an allowlist line explaining why.
- Every milestone ends with a measured exit criterion, not a feature list.
