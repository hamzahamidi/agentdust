# AgentDust roadmap

The binary is `agentdust`. M0 is complete with the Cursor coverage gap recorded in [the M0 report](docs/m0/report.md). M1 is complete, with its journal decision recorded in [ADR-1](docs/m1/adr-journal-format.md). M2 and M3 shipped in the [0.1.0 release](https://github.com/hamzahamidi/agentdust/releases/tag/v0.1.0). The 0.1.0 Homebrew formula is merged. Verify installation from a clean account before expanding the release matrix. Design: [the design spec](docs/superpowers/specs/2026-10-03-agentdust-design.md).

## Goal

A macOS developer who uses Claude Code installs the tool in two commands. Across concurrent sessions and subagents, Claude Code can analyse leftover processes and disk growth, show the findings, and clean stale processes after the user approves a typed code.

Multi-agent here means concurrent Claude Code sessions, foreground and background subagents within a session, and Agent Teams teammates. Teammates are separate Claude Code instances. Agent Teams are opt-in and experimental; M6 covers them only on Claude Code versions recorded in its test matrix. Codex, Cursor and other adapters remain deferred.

## Principles

- Release trust, security testing and distribution start in M0, not at the end.
- Only owned-ended and suspect processes can ever be signalled. Suspects need their own approval, and managed or unknown processes are never signalled.
- Evidence the tool stores is minimised at ingestion. Raw commands, command output and process environments are never persisted.
- Scope is cut before estimates are tightened. The first release is narrow and complete.

## v1.0 success criteria

1. Install: `brew install` pours a prebuilt binary and never compiles. `agentdust setup` finishes in under 5 seconds on a declared fixture account and prints a diff before changing anything.
2. Hook cost: p50 under 10 ms and p95 under 20 ms on the release binary under the three-session and subagent workload in criterion 9.
3. Idle: the MCP server does no periodic work when idle. CPU over a 60 second idle window stays under 0.1%. Resident memory is reported per release (target under 10 MB).
4. Actionability: `apply` rejects managed and unknown items, even when the model supplies their ID. Owned-ended items can be approved as one batch with one typed code. Every suspect item needs its own typed code, shows its evidence, and is never signalled as part of a process group.
5. Classifier: fixtures carry ground-truth labels (`true_owned_ended`, `true_live_owned`, `true_detached`, `true_managed`, `true_unknown`). No fixture outside `true_owned_ended` is classified owned-ended, no `true_managed` or `true_unknown` fixture is ever actionable, and every `true_owned_ended` fixture is classified owned-ended.
6. Privacy: fixture secrets never appear anywhere under the AgentDust data directory, checked by a test.
7. Supply chain: dependencies are locked from the first commit. Each release ships a checksum, an artifact attestation, an SBOM, and two independent macOS arm64 builds that produce a byte-identical binary.
8. Evidence: at least three independent machines run 0.x builds for two weeks, with sessions, proposed kills, false positives and failures counted.
9. Multi-agent isolation: test three concurrent Claude Code sessions, two active subagents in one session, and an Agent Team with two teammates. Interleaved events and helpers resolve only to their exact attribution scopes. Parent-ended/child-live, child-ended/parent-live, unrelated-session-live and ambiguous/shared ownership cases are covered. A helper is owned-live while any attributed owner is live, and owned-ended only after all attributed owners are proven gone. An unrelated live session does not change the candidate's classification. Ambiguous ownership stays non-actionable. After typed approval, apply revalidates owner liveness and process identity inside the per-process critical section before signaling.

## Decisions made

| Area | Decision |
| --- | --- |
| v1 scope | Process hygiene end to end, plus a read-only disk report |
| Language | Rust, one binary, Tokio only in the MCP subcommand |
| Interface | One MCP server with `agentdust_doctor`, `agentdust_plan`, `agentdust_apply`, `agentdust_disk`, plus a CLI |
| Agents | Claude Code first, including sessions, subagents and Agent Teams; Codex and Cursor deferred |
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

## Current priority

M7 shipped in 0.2.0 as a read-only disk report for Claude Code on macOS. Next is M6: prove safe attribution and cleanup across concurrent Claude Code sessions and subagents on macOS. Codex (M4), Cursor (M5) and operating system expansion remain deferred.

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

### M4 Codex adapter and setup (L, deferred)

- `hooks.json` under `CODEX_HOME` with `PreToolUse` and `PostToolUse` Bash matchers, MCP registration through `codex mcp add`.
- Its own acceptance matrix and setup rollback tests. The correlation engine ships here as likely-owned evidence only.

### M5 Cursor adapter and setup (L, deferred)

- `~/.cursor/hooks.json` with `afterShellExecution`, MCP registration.
- Its own acceptance matrix and setup rollback tests.

### M6 Multi-agent hardening and beta (L)

1. Capture fixtures from declared supported Claude Code versions for three concurrent sessions, two active subagents in one session, and one Agent Team with two teammates. Interleave shell and lifecycle events; cover foreground and background subagents, resume, `/clear` and abrupt exit. Record each client's version and hook payload.
2. Resolve the mismatch between subagent-generated tags and session-scope ownership. Decide and document whether subagents inherit parent ownership or receive independent scopes. Every tag resolves unambiguously to an attribution scope; a candidate may have multiple owners. Missing or ambiguous ownership stays non-actionable. Test parent-ended/child-live, child-ended/parent-live, unrelated-session-live and shared ownership. Assert owned-live while any attributed owner is live, owned-ended only after every attributed owner is proven gone, and no classification change from an unrelated session.
3. Record and test `SubagentStart` and `SubagentStop`. Include missing, duplicate, repeated and blocked stops, late events across resume and `/clear`, and abrupt exits. A stop or abrupt-exit event alone does not establish that its owner ended. Require fresh liveness evidence for the exact owner identity; unknown stays non-actionable, and stale events cannot close a newer lifecycle.
4. Race `apply` across separate MCP server processes, including owner or process identity changes during approval. After approval, revalidate owner liveness and process identity inside the per-process critical section before signaling. Assert zero signals if an owner resumes or ownership becomes ambiguous. At most one initial `SIGTERM` is sent per process identity, and no lock is held while waiting for user input.
5. Keep concurrent append and competing apply checks as regression gates. Expand fixtures for journal version drift and `brew upgrade`; unsupported or ambiguous evidence never becomes actionable. Review `unsafe` code and supported host and MCP version drift.
6. Meet the p50 and p95 hook budgets under this workload. Run one machine through two weeks of dogfood and record attribution errors, false proposals, blocked applies and signal counts.

Use `agent_id`, `agent_type`, `SubagentStart` and `SubagentStop` from the Claude Code [hooks reference](https://code.claude.com/docs/en/hooks). Agent Teams behavior is described in the [Agent Teams documentation](https://code.claude.com/docs/en/agent-teams).

Exit: the v1 multi-agent isolation criterion passes. No protected or ambiguous candidate receives a signal. Competing apply sends at most one initial `SIGTERM` per process identity. Hook latency meets criterion 2, and one machine completes two weeks of dogfood with the M6 metrics recorded.

### M7 Read-only disk report (M)

- Claude Code configuration and current-project worktree inventory through `agentdust disk [--json]` and `agentdust_disk`. Entries are classified rebuildable, history, worktree, application state or unknown. Logical and allocated size are reported separately. Partial scans are marked. Nothing is deleted. [Scope and limits](docs/m7/disk.md).

### M8 Claude Code plugin and upstream (S, external latency)

- Submit the optional Claude Code cleanup plugin, shipped in 0.1, to Anthropic's directory. Add one technical comment on anthropics/claude-code#1935 and request a lifecycle event carrying the PID and session of spawned processes.

Exit: the Claude Code plugin is submitted. Acceptance by third parties is not a gate.

### M9 1.0 readiness

- Criteria 8 and 9 met. Process-group signals and SIGKILL are added only if real failures justify them.

## Platform expansion after v1

The current target is Claude Code on macOS arm64. Keep Codex and Cursor deferred. Verify the Claude Code install and collect the two-week dogfood evidence before expanding to another operating system.

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

## Deferred capabilities

Codex and Cursor adapters, process-group signals, SIGKILL, listening ports, persisted plans, upstream outreach.

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
