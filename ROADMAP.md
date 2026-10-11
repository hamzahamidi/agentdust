# AgentDust roadmap

The latest stable release, AgentDust 1.7.1, supports Claude Code and Codex on Apple silicon macOS, Ubuntu 24.04 x86_64 and Ubuntu 24.04 x86_64 under WSL2. WSL2 uses the Linux process namespace, writable ext4 and one distribution for both AgentDust and the agent. The exact 1.7.1 archive and npm package passed WSL2 release acceptance in [run 38104539057](https://github.com/hamzahamidi/agentdust/actions/runs/38104539057). Automatic cleanup results include their record time. M0 through M3, M6, M7, M10, M11 and M13 are implemented for these support scopes. The Claude Code 2.1.292 fixture matrix covers concurrent sessions, subagents, resume, `/clear` and abrupt exit. The 1.0.0 Homebrew setup, live session discovery, idle measurement, human decline and approved apply passed on the maintainer's Mac using that exact release binary. See the [v1 readiness record](docs/v1/readiness.md), [fixture matrix](docs/m6/fixtures.md) and [design spec](docs/superpowers/specs/2026-10-03-agentdust-design.md).

## Project setup

Binary `1.2.0` adds one-confirmation directory batches and `auto enable --yes --projects ROOT` for user-authorized AI setup. Discovery covers the parent and immediate Git directories. Automatic eligibility still comes from classifier proof, not agent judgement. Uncertain cases require manual approval.

## Goal

The v1 goal was a macOS developer using Claude Code. Version 1.0 provides manual typed approval; version 1.1 adds opt-in automatic cleanup ([commands and limits](docs/automatic-cleanup.md)).

## Product target beyond v1

AgentDust should provide local process diagnosis, manual cleanup, opt-in automatic cleanup, TCP port recovery and outcome metrics on macOS, Linux and Windows. Claude Code and Codex are the baseline agent integrations. The shared CLI and MCP contracts carry common operations. Each operating system needs its own process inventory, identity checks, port listener source and release packages. Each agent needs setup and lifecycle attribution that can prove process ownership.

An MCP client can use the common diagnosis and manual tools where its client supports the required protocol features. Automatic cleanup requires a tested adapter that proves exact session ownership and exit. If a client cannot provide that evidence, its integration stays read only or uses explicit manual approval. A client or platform enters the support claim only after its installation, removal, cleanup safety and release checks pass.

| Target | Agent support | State |
| --- | --- | --- |
| macOS arm64 | Claude Code and Codex | Supported in 1.7.0. Codex automatic cleanup waits for every recorded host process to exit. |
| macOS x86_64 | Claude Code and Codex | Deferred until after M12 native Windows and architecture-specific release checks. |
| Linux x86_64 | Claude Code and Codex | Supported in 1.7.0 on Ubuntu 24.04 with the data directory on writable ext4. |
| Linux arm64 | Claude Code and Codex | Deferred until after M12 native Windows and architecture-specific safety and release checks. |
| Windows x86_64 | Claude Code and Codex | Unsupported. A feasibility spike must prove a safe single-process termination contract before implementation is planned. |
| Other MCP clients | Common diagnosis and manual tools where supported | Automatic cleanup needs a client specific lifecycle adapter and separate acceptance. |

The table separates current support from planned targets. A platform and agent pair becomes supported only after its installation, removal, cleanup safety and release checks pass. A host's session end event alone does not prove that a helper is safe to stop. Live, shared, ambiguous and unknown ownership remains protected.

## Principles

- Release trust, security testing and distribution start in M0, not at the end.
- Only owned-ended and suspect processes can ever be signalled. Suspects need their own approval, and managed or unknown processes are never signalled.
- Automatic cleanup requires a project scope enabled by the human and covers only freshly proven owned-ended processes within that scope. AgentDust's MCP tools cannot enable or broaden that policy. Live, ambiguous, shared across sessions, likely-owned, suspect, managed and unknown processes never enter automatic cleanup.
- Evidence the tool stores is minimised at ingestion. Raw commands, command output and process environments are never persisted.
- Scope is cut before estimates are tightened. The first release is narrow and complete.

## v1.0 success criteria

1. Install: `brew install` pours a prebuilt binary and never compiles. On the maintainer's Mac with real Claude Code, `agentdust setup` completes in under 5 seconds of execution time, excluding the person's review and consent wait, and prints a diff before changing anything.
2. Hook cost: p50 under 10 ms and p95 under 20 ms on the release binary under the concurrent-session and subagent workload in criterion 9.
3. Idle: the MCP server does no periodic work when idle. CPU over a 60 second idle window stays under 0.1%. Resident memory is reported per release (target under 10 MB).
4. Actionability: `apply` rejects managed and unknown items, even when the model supplies their ID. Owned-ended items can be approved as one batch with one typed code. Every suspect item needs its own typed code, shows its evidence, and is never signalled as part of a process group.
5. Classifier: fixtures carry ground-truth labels (`true_owned_ended`, `true_live_owned`, `true_detached`, `true_managed`, `true_unknown`). No fixture outside `true_owned_ended` is classified owned-ended, no `true_managed` or `true_unknown` fixture is ever actionable, and every `true_owned_ended` fixture is classified owned-ended.
6. Privacy: fixture secrets never appear anywhere under the AgentDust data directory, checked by a test.
7. Supply chain: dependencies are locked from the first code commit. Each release ships a checksum, an artifact attestation, an SBOM, and two independent macOS arm64 builds that produce a byte-identical binary.
8. End-to-end acceptance: before 1.0, run one controlled check on the maintainer's Mac with the exact v1 release candidate installed from Homebrew. Confirm `command -v agentdust` resolves to the formula install and `agentdust version` matches the `v1.0.0` tag. Record the archive checksum and formula `sha256`; do not change the tag, assets or formula between the check and stable promotion. Discover a live Claude session, decline one approval and verify zero signals, then approve a harness-created stale helper and verify only that PID receives SIGTERM. A three-Mac, two-week study is optional confidence evidence after 1.0, not a release gate.
9. Multi-agent isolation: the v1 target matrix covers three concurrent Claude Code sessions and foreground and background subagents in one session. Fixtures cover interleaved events, resume, `/clear` and abrupt exit. List a Claude Code version as supported for the lifecycle and attribution behavior represented in its fixtures only after those exact fixtures pass replay. Process signalling safety is established separately by the apply tests. Parent-ended/child-live, child-ended/parent-live, unrelated-session-live and ambiguous/shared ownership cases are covered. A helper is owned-live while any attributed owner is live, and owned-ended only after all attributed owners are proven gone. An unrelated live session does not change the candidate's classification. Ambiguous ownership stays non-actionable. After typed approval, apply revalidates owner liveness and process identity inside the per-process critical section before signaling. Agent Teams remain experimental and unsupported in v1.

## Original v1 decisions

| Area | Decision |
| --- | --- |
| v1 scope | Process hygiene end to end, plus a read-only disk report |
| Language | Rust, one binary, Tokio only in the MCP subcommand |
| Interface | One MCP server with `agentdust_doctor`, `agentdust_plan`, `agentdust_apply`, `agentdust_disk`, `agentdust_port`, and read-only `agentdust_auto_status`, plus a CLI |
| Agents | Claude Code first, including concurrent sessions and subagents; Codex native hooks and host-exit cleanup; Agent Teams experimental; Cursor deferred |
| Provenance | Layered evidence: event-driven process sampling, a journal of paired shell calls per adapter, and an environment tag on Claude Code as additive evidence only |
| Authorization | Manual MCP elicitation with a typed one-time code, or locally enabled directory policy for proven ended-session leftovers; fail closed |
| Plan state | Canonical plan in server memory with an opaque ID. `plan.json` is for inspection. A server restart invalidates plans |
| Actions in 0.1 | SIGTERM to one exactly revalidated PID. No process-group signals, no SIGKILL |
| Actionable classes | Owned-ended (one batch code) and suspect (one typed code per item). At most 10 items per apply call in total. Managed and unknown are never actionable. Likely-owned stays report-only until its precision is measured |
| Install | Homebrew formula that installs the attested prebuilt binary, then `agentdust setup` |
| Platform | macOS arm64 first |
| Journal | One O_APPEND file framed with RS, no lock for appenders and readers, an append recheck, 64 KiB frame cap, local APFS on macOS and writable ext4 on Linux |

## Open decisions

1. Suspect thresholds: the classifier currently uses a 30 minute age threshold and a two second idle sample. The M1 corpus verifies that the chosen age is applied, but does not establish that 30 minutes is the right threshold. Review false proposals during dogfood before treating the value as validated. See [fixture limits](docs/fixtures.md#the-suspect-thresholds).

## Current priority

M10 Linux x86_64 shipped in 1.7.0. M11 WSL2 certification completed in 1.7.1 after source acceptance on PR #70 and exact archive and npm acceptance in release run 38104539057. Journal rotation and retention run from the automatic worker. The next product priority is recording useful cleanups, incorrect proposals, deliberate keeps and successful port recovery during ordinary use. A cleanup count does not measure time, CPU or memory saved. M12 is a bounded native Windows feasibility decision. Diagnosis only or no go is acceptable; a full Windows provider is not committed. Do not schedule Linux arm64 or Intel Mac until there is demand and the shared runtime gates are complete. No multi Mac study is a release gate.

## Automatic cleanup in 1.1

The user-selected scope is automatic cleanup of proven leftovers, with approval for uncertain cases. The opt-in addition preserves the existing manual API and uses `1.1.0` under the [release policy](docs/release.md#semantic-versioning). Breaking public contracts would require a major version instead. All four items are implemented. The [implementation CI run](https://github.com/hamzahamidi/agentdust/actions/runs/37740053058) passed Linux and macOS checks, including isolated normal and abrupt owner exits and survivor restart. An isolated local GUI launchd check separately passed worker start and unload. Production setup acceptance and normal-use value remain unmeasured.

| Order | Deliverable | Completion evidence |
| --- | --- | --- |
| 1 | Human-enabled local policy for explicitly selected projects, pause/off control and a way to keep an intentional long-lived helper. The default manual flow stays available. | A model's MCP request cannot enable the policy, broaden its scope or override a keep decision. Disabled or unreadable policy and an unauthorized project produce no automatic signal. |
| 2 | Automatic owned-ended cleanup through the existing identity checks, owner revalidation, per-process claim and audit. | A proven leftover receives at most one initial `SIGTERM`. A live owner, shared or ambiguous attribution, changed identity, protected process or suspect produces no automatic signal. |
| 3 | A local worker triggered by lifecycle evidence and exact owner exit, with restart reconciliation and bounded work. Hooks remain short. | Cleanup works after normal and abrupt Claude exits without another model turn. Concurrent sessions and subagents remain protected. No full scan blocks a hook and the disabled mode starts no worker. |
| 4 | A short agent-visible result: stopped, kept, approval required, survivor or unavailable evidence. Record actions, their manual or automatic authorization source, and skipped reasons locally. | Claude can explain what happened using structured results. Declined uncertain cases remain untouched. A surviving helper is reported without escalation to `SIGKILL`. |

Steps 1 through 4 form one usable release. An MCP-only prototype can exercise the policy while Claude is active, but does not complete cleanup after the agent exits.

Measure useful cleanups, incorrect proposals, deliberate keep decisions and time saved during normal use on the available Mac. Controlled scenarios establish the execution rules; real incidents establish product value. No fixed multiweek study is a release gate. If normal use produces no useful incidents, record that result and reconsider further investment.

Port-conflict diagnosis and recovery are implemented in `1.4.0`: identify current-user-visible TCP listeners, classify through the full process inventory, apply only the existing automatic policy, then rescan listeners. Uncertain actionable listeners use the existing plan/apply approval flow. See [port scope and limits](docs/port-conflicts.md). Memory and CPU figures provide context, not permission to stop a process. Disk deletion, general resource dashboards and remote host bridges are outside this milestone.

## Remaining work by scope

| Scope | State |
| --- | --- |
| M0 through M3, M6, M7 and M13 | Implemented for the documented macOS arm64 scope |
| Claude Code on macOS arm64 | Supported; lifecycle and controlled Homebrew acceptance are recorded |
| Codex on macOS arm64 | Hook and MCP integration is available; automatic cleanup waits for exact host exit |
| Linux x86_64 | Supported in 1.7.0 on Ubuntu 24.04 with writable ext4 data directories |
| WSL2 | Supported in 1.7.1 on Ubuntu 24.04 x86_64. Source, exact archive and npm acceptance passed |
| Native Windows | Not supported; M12 is a feasibility decision, with no implementation commitment |
| Other AI agents | Common MCP diagnosis and manual tools are possible; automatic cleanup needs a client specific adapter |
| M9 controlled Homebrew acceptance | Complete; measurement limits remain in the [readiness record](docs/v1/readiness.md) |
| Plugin/setup guidance | Guidance package `0.8.0` explains setup readiness, agent-guided port recovery and automatic results |
| Automatic cleanup | Implemented for `1.1.0`; `1.5.0` adds per-result record times; measure value during normal use |
| M13 local outcome metrics | Included in 1.6.0; history begins when a metrics-enabled build records cleanup results |
| Port-conflict recovery | Implemented in `1.4.0`; measure useful recovery during normal use |
| M8 directory submission and upstream work | Optional |
| Codex desktop cleanup while its server stays alive | Deferred until session revocation and live subagent protection have a separate proof |
| M5 Cursor | Deferred until its lifecycle and ownership evidence can pass the same safety checks |
| Other MCP hosts | Manual diagnosis and approval tools are available where the client supports the required MCP approval behavior. Automatic cleanup requires a separate lifecycle adapter and proof of ownership and exit. |
| M10 through M12 | M10 shipped in 1.7.0; M11 completed in 1.7.1; M12 native Windows feasibility remains |

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

Exit: controlled Claude fixtures become owned-ended only after every exact attributed owner is proven gone. A `SessionEnd` event alone does not prove that its process ended. No protected or non-owned fixture becomes actionable.

### M3 Plan, apply and Homebrew: release 0.1 (L)

- `agentdust_plan` and `agentdust_apply` with typed-code elicitation, atomic per-item claim, revalidation after approval, SIGTERM to one PID, survivor report, audit log (0600, bounded size).
- Approval flow: one batch code for owned-ended items, then one code per suspect item with its evidence rendered from a fixed template. A deliberately detached process such as a tunnel can be classified suspect, so the per-item prompt is the control.
- Fail-closed behaviour on decline, cancel, timeout, unsupported clients and corrupt state.
- Terminal `agentdust apply` with the same rules, refusing without a foreground terminal or under an agent ancestor.
- `agentdust setup` for Claude Code: full diff, consent, surgical edits, idempotent, `--check`, `--remove`, ownership manifest, rollback report for partial failure.
- Homebrew formula, `agentdust support-bundle` (a local file, never uploaded), README, security and privacy documents.

Acceptance: the exact Homebrew 1.0 binary passed setup, live discovery, human decline and approved one-helper cleanup on the maintainer's Mac. A separate clean-account run and setup execution time are unmeasured, as recorded in the [readiness record](docs/v1/readiness.md). They are not additional 1.0 release gates.

### M4 Codex adapter and setup

- Six native user hooks under `CODEX_HOME`, paired Bash calls and native MCP registration through `codex mcp add`.
- `CODEX_SESSION_ID` is correlated with hook session IDs through a separate keyed digest. No command rewriting or transcript parsing.
- Exact runtime identities control liveness. Session end and subagent turn completion do not replace host exit. Concurrent and resumed hosts sharing a session marker all stay in the ownership set.
- Missing ancestry, a host switch without a recorded session start and mixed Claude/Codex markers cannot grant automatic cleanup.
- Setup install, removal, idempotence, rollback and user-setting preservation tests are in `crates/agentdust-agents/tests/codex_setup.rs`.

Remaining: immediate cleanup after a desktop chat ends while its shared server stays alive. This requires independently defensible session revocation, resume and live subagent protection. The current adapter waits for exact host exit. See [support and limits](docs/codex.md).

### M5 Cursor adapter and setup (L, deferred)

- `~/.cursor/hooks.json` with `afterShellExecution`, MCP registration.
- Its own acceptance matrix and setup rollback tests.

### M6 Multi-agent hardening and beta (L)

1. Record fixtures for three concurrent sessions and two active subagents in one session. Interleave shell and lifecycle events; cover foreground and background subagents, resume, `/clear` and abrupt exit. Record each client's version and sanitized hook data in the [M6 fixture log](docs/m6/fixtures.md). Support claims apply only to exact Claude Code versions with a complete core scenario matrix that passes replay. Agent Teams are experimental and are not part of the v1 support claim.
2. Route records by agent and session ID, then use exact process identity to resolve resumed scopes and additional owners. The keyed subagent ID groups lifecycle activity; it does not identify the process owner. If Agent Teams are evaluated later, teammates are separate Claude Code instances and need independent session and process scopes when those identities are present. Every tag resolves to exact attribution scopes; a candidate may have multiple owners. Missing or ambiguous ownership stays non-actionable. Test parent `SessionEnd` while its process is live, subagent stop while the parent is live, unrelated session live, resumed sessions, and shared ownership. Assert owned-live while any attributed owner is alive or unverified, owned-ended only after every exact owner is proven gone, and no classification change from an unrelated session.
3. Record and test `SubagentStart` and `SubagentStop`. Include missing IDs, failed ID digests, child events before parent start, duplicate starts, repeated and blocked stops, late events across resume and `/clear`, and abrupt exits. A stop or abrupt-exit event alone does not establish that its owner ended. Require fresh liveness evidence for every exact owner identity; unknown stays non-actionable, and stale events cannot close a newer owner scope.
4. Race `apply` across separate MCP server processes, including owner or process identity changes during approval. After approval, revalidate owner liveness and process identity inside the per-process critical section before signaling. Assert zero signals if an owner resumes or ownership becomes ambiguous. At most one initial `SIGTERM` is sent per process identity, and no lock is held while waiting for user input.
5. Keep concurrent append and competing apply checks as regression gates. Expand fixtures for journal version drift and `brew upgrade`; unsupported or ambiguous evidence never becomes actionable. Review `unsafe` code and supported host and MCP version drift.
6. Measure hook p50 and p95 against the release binary target under the supported session and subagent workload. After beta, use the [dogfood record](docs/m6/dogfood.md) to note attribution errors, false proposals, blocked applies, signals, failures and latency when available. Field observations inform follow-up work but do not extend the v1 release gate.

Use `agent_id`, `agent_type`, `SubagentStart` and `SubagentStop` from the Claude Code [hooks reference](https://code.claude.com/docs/en/hooks). Agent Teams behavior is described in the [Agent Teams documentation](https://code.claude.com/docs/en/agent-teams).

Implementation note: new `agent_id` values are stored as keyed digests for activity correlation. Record routing uses the agent and raw `session_id`, then exact process identity. `agent_type`, transcript paths and transcript contents are ignored. Missing IDs, failed digests and child-first events make that session non-actionable. Schema version 2 fences older readers, which otherwise could skip the only record of a live additional owner. Apply stores the exact attribution owner set with the approved plan item and compares it with a fresh survey while holding the per-process lock.

Exit: at least one exact Claude Code version has a complete core scenario matrix that passes replay, and criterion 9 passes for every version listed as supported. No protected or ambiguous candidate receives a signal. Competing apply sends at most one initial `SIGTERM` per process identity. Hook latency meets criterion 2. Agent Teams remain experimental until separately validated.

### M7 Read-only disk report (M)

- Claude Code configuration and current-project worktree inventory through `agentdust disk [--json]` and `agentdust_disk`. Entries are classified rebuildable, history, worktree, application state or unknown. Logical and allocated size are reported separately. Partial scans are marked. Nothing is deleted. [Scope and limits](docs/m7/disk.md).

### M8 Optional Claude Code plugin and upstream work (S, external)

- The cleanup plugin code was added in [PR #20](https://github.com/hamzahamidi/agentdust/pull/20). Optionally submit it to Anthropic's directory and add a technical comment on [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935) requesting a lifecycle event with the PID and session of spawned processes.

Exit: optional. Directory acceptance and upstream response are not v1 gates.

### M9 1.0 readiness

- The controlled 1.0 release acceptance is complete. Passed checks and measurement limits are recorded separately in the [readiness record](docs/v1/readiness.md). A release, merge or test pass closes only the criterion it directly verifies. Process-group signals and `SIGKILL` remain deferred.

### M13 Local outcome metrics (S, included in 1.6.0)

- Record manual and automatic cleanup results in a private JSONL log with only time, mode, classifier and result.
- Provide `agentdust metrics [--json]` and a read-only `agentdust_metrics` MCP tool with totals and daily chart buckets.
- Bound history to a 5 MiB active file and one rotated generation. Do not upload metrics or estimate time, CPU or memory savings.

Exit: privacy checks cover the stored fields and file permissions; CLI and MCP return the same aggregate contract; daily buckets cover every retained result and show an empty history before the first metrics event.

## Deferred MCP host distribution

The MCP tool contract is shared, but MCP does not provide session lifecycle evidence. Claude Code and Codex remain the first class integrations. Other compatible clients can use diagnosis and manual tools where their approval behavior is supported. Automatic cleanup stays disabled until a client specific adapter proves ownership and exit.

AgentDust currently exposes a local stdio MCP server and can signal local processes after typed approval. Publishing it to another AI host needs a client-specific package or transport and a client-specific safety review. This work does not add another operating system or declare a host supported before its acceptance checks pass.

1. Claude Desktop: build a private `.mcpb` package and verify binary installation, version reporting, Claude Code hook setup, typed elicitation, decline, cancellation, timeout, unsupported capabilities and post-approval revalidation in that client. Submit a local desktop extension to Anthropic's extension directory only after the package and approval checks pass. The [Claude Desktop guide](https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop) distinguishes local desktop extensions from remote connectors.
2. Slack: implement and test an authenticated Streamable HTTP bridge that remains attached to the user's Mac. Slack's [MCP client requires HTTPS Streamable HTTP](https://docs.slack.dev/ai/slackbot-mcp-client/) and does not accept stdio. Verify signed Slack requests before trusting caller metadata. Keep `agentdust_apply` hidden and reject direct apply calls in remote mode. Test account binding, revocation, tunnel loss and sanitized responses before requesting Slack Marketplace review.
3. ChatGPT: test a private Secure MCP Tunnel from the user's machine, with remote mode limited to read-only tools. Omit apply from discovery and reject direct calls server-side. Bind the tunnel to the intended user and Mac, and document that tool results pass through ChatGPT. OpenAI's [MCP app guide](https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt) distinguishes private tunnel testing from public app distribution. Public ChatGPT directory submission is deferred and needs a separate design with a stable, publicly reachable HTTPS endpoint.
4. Keep process signaling local until that host has a proven user identity, typed approval, decline and cancellation behavior, timeout handling, and fresh process identity checks after approval. A host's generic confirmation prompt does not replace AgentDust's typed approval.
5. Submit Claude Desktop and Slack directory listings only after each host's package, authentication, privacy and safety review passes. Workspace publication and public directory listing are separate release steps. ChatGPT directory publication is outside this milestone.

Exit: Claude Desktop package works on a clean account with the required local setup; Slack read-only mode cannot reach apply, including by direct tool call; the private ChatGPT tunnel passes a read-only test and rejects direct apply calls. Public ChatGPT listing and remote apply are not exit requirements.

## Platform expansion

macOS arm64 and Linux x86_64 are shipping platforms. Linux support is limited to Ubuntu 24.04 x86_64 with writable ext4 data directories. The next sequence is WSL2, then native Windows x86_64. Linux arm64 and Intel macOS remain deferred until after M12, followed by architecture-specific safety and packaging checks. Agent support is tested with Claude Code and Codex on each target. Other operating systems and agents stay out of the support claim until their matrix entries pass.

The controlled Homebrew acceptance is complete for Claude Code on Apple silicon. Codex automatic cleanup still waits for exact host exit. Field observations on the available Mac continue in parallel with platform work. A multi Mac study is optional confidence evidence.

### M10 Linux x86_64 with Claude Code and Codex (L), complete in 1.7.0

Linux support in 1.7.0 is limited to Ubuntu 24.04 x86_64 with the AgentDust data directory on writable ext4.

PR #68 added the Linux process provider, Claude Code and Codex setup, lifecycle hooks, MCP diagnosis and approved apply, opt-in automatic cleanup, TCP port diagnosis and recovery, systemd user worker, and Linux x86_64 release packaging. The exact merged main CI run [38091638966](https://github.com/hamzahamidi/agentdust/actions/runs/38091638966) passed Linux and macOS checks. The 1.7.0 release workflow [38092195368](https://github.com/hamzahamidi/agentdust/actions/runs/38092195368) passed the Linux and macOS build, package, acceptance, checksum, SBOM, provenance and Homebrew smoke gates.

1. Linux inventory uses boot ID, `/proc/<pid>/stat` start time, effective UID, executable path, CPU time and exact identity rechecks. Bounded reads and ambiguous or changing process data fail closed. Systemd cgroup names remain heuristic evidence.
2. Classifier and owner liveness tests cover PID churn and reuse, reparenting, credentials, permission denial, service and scope cgroups, containers, and zombies.
3. Linux journal writes require writable ext4. Ubuntu CI exercises private permissions, concurrent append, rotation, locking, torn tail recovery and refusal after a read-only remount.
4. Claude Code and Codex install, check, remove and hook behavior are tested separately. Linux setup configures hooks and the local MCP server only when its data directory passes the filesystem gate. Removal remains available when it does not.
5. Manual apply uses pidfd identity revalidation, authoritative systemd unit lookup and `pidfd_send_signal`; missing evidence refuses the signal. The Ubuntu tests cover decline, cancellation, identity changes, pidfd behavior and competing apply.
6. Automatic cleanup is opt in, uses the shared project policy, and runs through a systemd user service. Live owners and kept processes survive; eligible helpers from ended sessions are stopped once. Port inventory rechecks process identity, listener inode and network namespace, and fails closed when it cannot establish scope.
7. Release Actions build two independent Linux x86_64 binaries, package the native archive and npm launcher, compare binaries, test native installs, and attach checksums, SBOM and provenance.

Exit: complete. Ubuntu 24.04 x86_64 acceptance passed for Claude Code and Codex, including setup, removal, lifecycle, approval, cleanup, port recovery, metrics, pidfd identity and race checks, ext4 journal checks, and native and npm release package installation.

### M11 WSL2 certification with Claude Code and Codex (S, 2 to 4 days)

Run AgentDust and the agent inside one Ubuntu 24.04 WSL2 distribution. Use the existing Linux x86_64 release package and process provider. Keep discovery, identity checks and signals inside the WSL2 Linux process namespace. Reject data directories on `/mnt/c` and other non-ext4 volumes. See the [WSL2 runtime limits](docs/wsl2.md).

Acceptance uses controlled fixtures for Claude Code and Codex setup, check and removal; hooks and MCP; doctor, plan and approved apply; opt-in cleanup after normal and abrupt owner exit; and controlled TCP port recovery. These lifecycle fixtures do not launch the Claude Code or Codex client binaries. The exact release archive is checked for version, `/mnt/c` refusal, worker lifecycle and Windows process isolation. The exact npm tarball is installed and its Linux binary is compared with the archive. Launch a Windows PowerShell process through WSL interop, verify it is absent from Linux `/proc` and AgentDust findings, and confirm it stays alive during the inventory. Verify that the default WSL data path uses ext4. Document that stopping WSL stops its worker and that systemd does not keep the distro alive.

Exit: complete. The controlled source matrix passed on PR #70 in a clean WSL2 distribution. The exact 1.7.1 archive and npm package passed release acceptance in [run 38104539057](https://github.com/hamzahamidi/agentdust/actions/runs/38104539057). WSL2 remains a Linux deployment, not native Windows support.

### M12 Native Windows feasibility spike (S, 4 to 7 days)

Do not implement a Windows process provider until a prototype answers whether AgentDust can identify one process exactly, revalidate it immediately before action, prove its session ownership, map TCP listeners, and stop only that process with a graceful action. Test Windows process handles and creation identity, Claude Code and Codex lifecycle evidence, install and removal behavior, and console control event scope. `TerminateProcess` is forceful and does not meet the current graceful single-process contract by itself. WSL2 does not count as native Windows support.

Exit: record the prototype, tests and a yes or no decision. A yes opens a separately estimated implementation milestone with manual approval and automatic cleanup as separate gates. A no leaves Windows unsupported or diagnosis only. Do not claim safe cleanup until per-process graceful termination and identity revalidation pass.

## Architecture coverage after the platform baseline

After the WSL2 and Windows feasibility decisions, rank Linux arm64 and Intel macOS by user demand, shared safety coverage and release cost. Build `aarch64-unknown-linux-gnu` or `x86_64-apple-darwin` artifacts only when the demand justifies the ongoing matrix. Each architecture needs the Claude Code and Codex setup, lifecycle and apply gates before it is listed as supported.

## Deferred capabilities

Immediate per-chat Codex cleanup while a shared host remains alive, the Cursor adapter, process-group signals, `SIGKILL`, UDP and non-listening socket conflicts, persisted plans, disk deletion or quarantine, daemons, scheduled cleanup, self-update, remote MCP transport.

## Optional external work

The M8 plugin directory submission and upstream issue comment are optional. Neither is required for v1.

## Risks

| Risk | Mitigation |
| --- | --- |
| MCP clients differ in elicitation behaviour | M0 matrix, pinned SDK, typed-code regression tests, refuse `apply` on any failing client |
| `CLAUDE_ENV_FILE` is missing on resume, `/clear` or in plugin hooks | Tag is additive evidence only, with explicit acceptance cases |
| Hook latency grows under journal contention | 3 and 16 writer benchmark in M1, hard budget in CI |
| Missing Codex native markers or host identity, and weaker Cursor provenance | Codex requires its native marker and every exact recorded host gone. Incomplete evidence stays report-only. Cursor remains deferred |
| A suspect is a deliberately detached process | Evidence shown per item, one typed code per item, SIGTERM only, audit log, deny list, false positives counted in the 1.0 evidence |
| Secrets in commands reach the journal | Ingestion minimisation, privacy invariant test |
| Config patching damages a user file | Surgical edits, diff plus consent, per-product rollback, `--check`, `--remove` |
| Release trust discovered too late | Release skeleton and reproducibility experiment in M0 |
| Host formats change | Versioned adapter code tested against old and current client versions |

## Cross-cutting rules

- Test layers: unit and property tests, recorded fixtures, live process-tree tests, MCP contract tests, packaged install acceptance, release-artifact smoke tests.
- One CI run per change, no scheduled workflows. Linux CI runs lint, unit and property tests, fuzz seed replay, ext4, systemd, pidfd, port, automatic cleanup and npm checks. macOS CI runs full tests, package checks, hook latency, automatic cleanup and port recovery. M11 adds a Windows hosted runner that provisions Ubuntu 24.04 under WSL2 and runs the WSL acceptance matrix.
- No new runtime dependency without an allowlist line explaining why.
- Every milestone ends with a measured exit criterion, not a feature list.
