# agent-hygiene design

Date: 2026-10-03. Working name: agent-hygiene, binary `hygiene`. Companion to [ROADMAP.md](../../../ROADMAP.md), which holds milestones and success criteria.

## 1. Context

AI coding agents start processes that outlive the session that started them: stdio MCP servers, dev servers, watchers and background shell jobs. On macOS these orphans are reparented to launchd (PPID 1) and keep memory, ports and sometimes CPU. The problem is reported for every major agent:

- Claude Code: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935), open since June 2025.
- Codex: [openai/codex#21008](https://github.com/openai/codex/issues/21008), 501 stale MCP helpers holding about 4 GB under launchd.
- Cursor: a [known issue](https://forum.cursor.com/t/mcp-process-leak-orphaned-children-on-restart/156478) confirmed by Cursor staff in April 2026.

Agent data directories also grow without bound (session transcripts, worktrees, scratch directories, VM bundles).

### Goal

A developer installs the tool with `brew install` and `hygiene setup`. Any connected agent can then analyse leftover processes and disk growth, show findings, and terminate stale processes after the developer approves each action with a typed code.

### v1 scope

- Process hygiene end to end: inventory, provenance, classification, plan, approval, SIGTERM.
- A read-only disk report.
- Adapters for Claude Code, Codex and Cursor, delivered in that order.
- macOS on Apple silicon.

### Non-goals for v1

Disk deletion or quarantine, Linux and Windows, daemons or scheduled cleanup, self-update, any network access, remote MCP transport, SIGKILL, process-group signals.

## 2. Architecture

One Rust binary built from a three-crate workspace. The split keeps Tokio and `rmcp` out of the core crate so the core can be audited without them. Tokio starts only in the `mcp` subcommand.

```
hygiene (bin)
 ├─ hygiene-core    std + serde: platform, journal, evidence, classify, plan, apply, sanitize
 ├─ hygiene-mcp     tokio + rmcp: hygiene_doctor, hygiene_plan, hygiene_apply
 └─ hygiene-agents  claude, codex, cursor: hook payload parsers and setup patchers
data dir: ~/Library/Application Support/hygiene (0700)
```

| Subcommand | Behaviour |
| --- | --- |
| `hygiene hook <agent>` | Synchronous. Reads one hook event, appends journal records, prints nothing, exits 0 |
| `hygiene mcp` | Stdio MCP server. Holds plans and approval state in memory |
| `hygiene doctor`, `hygiene status` | Read-only report, including provenance health and running versions |
| `hygiene apply` | Human-only terminal path over the same core (section 6.6) |
| `hygiene setup`, `hygiene support-bundle`, `hygiene version` | Local only |

Two apply front ends share one core: the MCP server, approved through elicitation, and the terminal command. Sampling of process trees is event driven (hook calls, MCP server start, each MCP tool call). There are no timers, so an idle server does no work.

## 3. Evidence model

### 3.1 Identities

```
KernelIdentity   { boot_session_uuid, pid, start_time_us, uid }
IdentityEvidence { exe_path }
AgentIdentity    { kind: claude | codex | cursor, kernel: KernelIdentity, session_id, subagent_id? }
```

A signal requires every field of `KernelIdentity` and `exe_path` to match a fresh read. A path that cannot be read fails closed. The boot session UUID invalidates every PID from an earlier boot.

### 3.2 Journal

Append-only records, one JSON object per line, schema version 1.

```
{ v: 1, kind, agent, session_id, subagent_id?, agent_identity?, tool_use_id?,
  wall_ts, mono_ts, boot,
  session_tag_key?: HMAC(secret, "HYG-SESSION-v1\0" || tag),
  cwd_key?:         HMAC(secret, "HYG-CWD-v1\0" || canonical cwd),
  exe_base?:        "node",
  procs?:           [{ pid, start_time_us, ppid, exe_base }] }
kind: session_start | session_end | shell_start | shell_end | sample | server_start
```

- Readers skip unknown kinds and fail closed on an unknown schema version.
- The raw session tag, raw commands, command output and process environments are never written.
- `mono_ts` is a system-wide monotonic clock read, used to order events within one boot. `wall_ts` is for display.
- Same-user forging of journal records is out of scope (section 7.4).

Storage until the M1 contention benchmark decides otherwise: `journal.jsonl` plus a separate `journal.lock` taken with `flock` by every reader, writer and rotator before opening the data file. Rotation never truncates the active file. A truncated last line is ignored and counted in hook health.

Retention: records from earlier boots are pruned first, records of ended sessions after 14 days or when the journal passes 20 MB, and records of active sessions are pinned. When evidence for an active session is lost, its provenance is marked degraded.

### 3.3 Session state machine

| State | Entered by | Notes |
| --- | --- | --- |
| unknown | first record for a session ID | |
| active | `session_start` or any event with a live agent identity | |
| ended | agent identity no longer alive, or `session_end` | `session_end` alone never makes a process actionable |

A duplicate `session_start` with the same agent identity is ignored. A `session_start` with a different agent identity for the same session ID opens a new resumed scope. Late events never move an ended scope back to active. Subagents share their parent's session scope and record `subagent_id`.

### 3.4 Classes

Checked from top to bottom. The first match wins.

| Class | Evidence | Actionable |
| --- | --- | --- |
| managed | `managed.launchd`: PID matches a loaded launchd job. `managed.homebrew`: exact Homebrew service identity. `managed.app_helper`: verified running owning app. `managed.deny`: exact deny identifiers | never |
| owned-live | ownership evidence and the agent identity is alive | never |
| owned-ended | session tag match or sampled as a descendant, and the agent identity is gone | batch code |
| likely-owned | cwd key and executable match a paired shell window of an ended session | report only |
| suspect | parent is launchd or the launcher chain is dead, same UID, older than the age threshold, idle, not managed, not on the deny list | one code per item |
| unknown | everything else | never |

The deny list contains PID 1, processes owned by UID 0 or another user, the hygiene server itself, live agent processes, Apple system paths and helpers of running apps. Thresholds for age and idleness are set from the M1 fixture corpus.

### 3.5 Representations

| Type | Seen by | Content |
| --- | --- | --- |
| `RawIdentity` | operating system calls only | full paths, identities, command lines |
| `ModelFinding` | MCP tool results, so the model | item ID, class, `exe_base`, PID, age, evidence kinds, `cwd_relation` (`same_repo`, `other_repo`, `home`, `temp`, `other`). No command text and no path |
| `HumanDisplay` | elicitation prompts and the terminal | fixed templates. Elicitation adds the cwd basename (at most 40 characters). The terminal adds the redacted command (120 characters) and cwd (80 characters). Every value has C0, C1, escape and bidi controls escaped |

Privacy for the model is structural: it never receives command text or paths, so redaction rules are defence in depth and not the guarantee. Elicitation prompts pass through the host application and can appear in its transcripts, which the documentation states. Live data such as cwd is read from the process at display time and never taken from the journal.

## 4. Provenance and adapters

### 4.1 Agent identity

Each adapter accepts only its own host. `hygiene hook claude` walks its ancestry to the first process that matches a known Claude Code executable, and records that `AgentIdentity`. A Codex or Cursor process found on the way is ignored. If no valid anchor exists, the record carries no agent identity and can never support an owned class.

Known executables start from setup and are refreshed by every genuine adapter invocation, because an agent update replaces its binary. Cursor gets no "agent ended" upgrade until M0 shows which process lifetime covers a Cursor conversation.

### 4.2 Hook events

| Agent | Events | Tag |
| --- | --- | --- |
| Claude Code | `SessionStart`, `PreToolUse` and `PostToolUse` with a Bash matcher, `SessionEnd` | `HYGIENE_SESSION` written to `CLAUDE_ENV_FILE` when it is present |
| Codex | `SessionStart`, `PreToolUse` and `PostToolUse` with a Bash matcher, `SessionEnd` | none |
| Cursor | `sessionStart`, `afterShellExecution`, `sessionEnd` | none unless the M0 experiment shows that `sessionStart` env reaches shell processes |

Shell windows pair `shell_start` and `shell_end` by `tool_use_id`. Cursor windows come from its reported duration. An unpaired event lowers confidence and no window is invented.

### 4.3 What each source proves

- A session tag read from a process environment proves the process descends from that session, including detached children, because the environment survives reparenting. The tag is extra evidence: `CLAUDE_ENV_FILE` is known to be missing in plugin hooks and not sourced after resume or `/clear`.
- A sample proves attachment at that moment. A job that detaches before the next sample is never sampled, because it is reparented to launchd at once while the agent keeps running.
- A journal window match is correlation and only reaches likely-owned.

### 4.4 Hook input, timing and health

Hooks deserialize by streaming. Output fields (`tool_response`, Cursor's `output`) are skipped without being materialised, and kept fields have length limits. The hook reads stdin to the end, so the host never sees a closed pipe.

Timing has three separate limits:

- Service level for normal input: p50 under 10 ms and p95 under 20 ms.
- Host timeout: the hook configuration written by `setup` sets a 10 second timeout, the hard bound on how long an agent can wait.
- Journal write budget: the lock is taken without blocking, with retries for at most 20 ms. On failure the record is dropped and hook health is updated.

Benchmarks with 1, 10 and 100 MB payloads assert latency and that each host handles the hook normally.

A hook always exits 0 and prints nothing. Each agent has a health record (last success, last error, error count, error kind) updated under the journal lock. `doctor` and `status` report each agent as healthy, degraded or unavailable, and a provenance failure only lowers confidence.

## 5. MCP tools

| Tool | Annotations | Result |
| --- | --- | --- |
| `hygiene_doctor` | `readOnlyHint` | `ModelFinding` list, class counts, provenance health |
| `hygiene_plan` | not read-only, not destructive | opaque plan ID and actionable items only |
| `hygiene_apply` | `destructiveHint` | per-item results |

Annotations are hints for the client. The server enforces every rule itself.

## 6. Plan and apply

### 6.1 Plan

`hygiene_plan` takes a fresh inventory and keeps only owned-ended and suspect items. The plan lives in server memory under a random 128-bit ID and expires after 10 minutes or when the server exits. An inspection report (0600) is written for the user to read. It holds sanitised display data and no approval state, cannot be passed back to `apply`, is deleted at expiry, and stale copies are removed at startup.

### 6.2 Apply limits

- At most 10 items per call.
- Owned-ended items are approved as one batch. The prompt lists every item with its ID, PID, executable, age and provenance, and says "started by session X, which has ended".
- Each suspect item gets its own prompt and code, with its evidence.

### 6.3 Approval protocol

1. Reject an unknown or expired plan and any item ID that is not in it.
2. Mark the items pending in server memory. No lock is held during approval.
3. Ask through elicitation: a form with one required string field and no default. The code is 4 characters from an unambiguous alphabet, single use, valid for 2 minutes.
4. `requestState` carries only a random nonce. Server memory maps the nonce to the exact call arguments, plan ID, ordered item IDs, approval index, expiry and expected code. On re-entry the nonce must exist and be unused, the arguments must match, the expiry must hold and the response must validate. The nonce is consumed before any signal.
5. Decline, cancel, timeout, an empty answer or a wrong code ends approval for that batch or item.

### 6.4 Execution

For each approved item, in order:

1. Take a non-blocking `flock` on the per-identity lock file. If another server holds it, report "handled elsewhere".
2. Take a fresh inventory and reclassify from scratch, including managed detection. Any change to a less certain class, such as owned-ended to suspect, aborts the item.
3. Read the identity and call `kill(pid, SIGTERM)` back to back, with no logging, allocation or I/O in between. `ESRCH` means the process is gone.
4. Poll for up to 5 seconds and report terminated, survivor, gone before signal or revalidation failed.
5. Append to the audit log and release the lock.

`kill` takes a numeric PID, so a residual race remains if the process exits and its PID is reused between the read and the signal. The design narrows that window and tests it. It does not claim to remove it.

Items succeed or fail independently and nothing is rolled back. A failure on one item does not stop independently approved items whose revalidation passes.

### 6.5 Failure modes

| Situation | Result |
| --- | --- |
| Client has no elicitation | `apply_not_supported`, with a hint to run `hygiene apply` in a terminal |
| Server restarts during approval | Plan and nonce are gone, nothing is signalled, the agent must plan again |
| Corrupt or unknown-version state | `doctor` reports what it can, owned classes are unavailable, `apply` refuses owned-ended items |
| Lost install secret | HMAC evidence is unverifiable and owned classes downgrade |
| `apply` disabled in `config.toml` | `apply` refuses everything, `doctor` still works |

### 6.6 Terminal apply

`hygiene apply` runs doctor, plan and approval in one process with the same rules and codes. It refuses unless stdin, stdout and `/dev/tty` are terminals, a controlling terminal exists, the process is in the terminal's foreground process group, and no ancestor is a known agent. Agent recognition combines executables observed by adapters, executables recorded by setup and a built-in list of agent executable and bundle identifiers, and refuses when in doubt. No flag, environment variable or pipe supplies a code. This guards against automation by accident and is documented as such, not as proof of a human.

## 7. Security and privacy

### 7.1 Threats and controls

| Threat | Control |
| --- | --- |
| A model, injected or not, calls `apply` | Typed code through elicitation. Tool arguments cannot carry approval |
| The host auto-accepts elicitation | Required random code. Empty or default content is rejected. The host stays in the trusted computing base |
| Malicious process metadata | The model never receives text values. `HumanDisplay` escaping. Raw values never appear in approval prose |
| Secrets in commands, paths or environments | Structural: no command text or path in model output, nothing raw persisted, only `HYGIENE_SESSION` read from an environment and hashed at once, shell output never captured. Argument redaction in terminal output is defence in depth |
| PID reuse | Section 6.4 |
| Concurrent apply | Per-identity `flock` |
| Wrong target | SIGTERM only, one exact PID, actionable classes only, deny list |
| A hook slows the agent | Section 4.4 limits, exit 0, nothing printed |
| Compromised dependency or artifact | Section 8.3 |

### 7.2 Privacy contract

The data directory is 0700 and every file in it is 0600.

| File | Content | Retention |
| --- | --- | --- |
| `install.secret` | 32 random bytes, created atomically with exclusive, no-follow semantics | until purge |
| `journal.jsonl`, `journal.lock` | section 3.2 fields only | section 3.2 |
| `health.json` | per-agent hook health, no input snippets | overwritten |
| `audit.log` | plan ID, item, identity, class, evidence kinds, result | rotates at 5 MB |
| `inspection/` | sanitised plan reports | deleted at plan expiry |
| `locks/` | empty per-identity lock files | removed when unused |
| `manifest.json` | versioned record of entries written by `setup` | until removal |
| `config.toml` | user settings, including `apply = false` | user managed |

A test plants fake API keys, user names and repository names in commands, paths, arguments and environments, then asserts that none of them appear in any file above, in the support bundle or in `ModelFinding` output. The binary contains no network code.

### 7.3 Startup validation

Every command that reads trusted state first checks the data directory: it must be a real directory owned by the current UID, `install.secret`, the journal and the manifest must be regular files that are not symlinks, and the secret must have no group or world access. A wrong mode on the directory is corrected. Anything else makes the command refuse with a specific message.

`hygiene setup --remove --purge-data` removes the agent integrations first and verifies the removal. Only then does it delete the data directory. If removal fails, the manifest and secret stay so the user can retry.

### 7.4 Out of scope

An agent with free shell access (it can signal processes without this tool), a malicious MCP client, and a same-user or root attacker, including one that forges journal records.

## 8. Setup, upgrades and release

### 8.1 Configuration roots

| Agent | Root |
| --- | --- |
| Claude Code | `CLAUDE_CONFIG_DIR` when set, otherwise `~/.claude` |
| Codex | `CODEX_HOME` when set, otherwise `~/.codex` |
| Cursor | `~/.cursor` |

Because shell aliases or direnv can set these only when an agent starts, `setup` accepts `--claude-config-dir` and `--codex-home`, and the consent screen prints every resolved destination.

### 8.2 `hygiene setup`

Each resource has one mutation owner:

| Resource | Changed by |
| --- | --- |
| Claude Code MCP server | `claude mcp add --scope user hygiene -- <prefix>/bin/hygiene mcp` and `claude mcp remove` |
| Codex MCP server | `codex mcp add hygiene -- <prefix>/bin/hygiene mcp` and `codex mcp remove` |
| Cursor MCP server | direct patch of `mcp.json` |
| Hooks for all three agents | direct patch of `settings.json`, `hooks.json` and `hooks.json` |

Flow:

1. Detect agents and record their executables.
2. Build the change set and print unified diffs for file patches and the exact native commands.
3. Ask y/N.
4. Apply one product at a time. Products succeed independently: if Codex fails after Claude Code succeeded, Claude Code stays installed, Codex is rolled back, Cursor is reported as not attempted, and running `setup` again completes the rest.

Rules for file patches:

- `lstat` first. A symlinked, hard-linked or non-regular file is never edited automatically. Setup prints the change for the user to apply by hand.
- Re-read the file and abort if it changed since the diff. Write a temp file, sync it, rename, sync the directory, keep the mode. This protects against application and OS crashes, not every power loss.
- Insert only the tool's own entries beside existing ones.

Rules for native commands: snapshot the current state, refuse if a `hygiene` server exists that the manifest does not own, run the command, then list the configuration again and compare command and arguments exactly. An exit code of 0 alone is never success. Rollback uses the native remove command.

Ownership: the manifest (versioned) records each inserted node and a hash of the value installed. An equivalent entry that existed before setup is recorded as pre-existing and never removed. `--remove` deletes an entry only if the manifest created it and its current value still matches. An edited entry is reported as drift. An unknown manifest version makes `--remove` refuse. `--check` reports drift, conflicts and partial installs. Every path uses the stable Homebrew prefix, never the Cellar.

### 8.3 Upgrades and mixed versions

After `brew upgrade`, an old MCP server can keep running while new hooks run the new binary. Every persistent format is versioned. A process that meets a newer version only lowers confidence or refuses, and never interprets records it does not know. `hygiene status` lists running hygiene processes with their versions.

### 8.4 Release

- Builds run on the pinned `macos-26` arm64 image with the toolchain pinned in `rust-toolchain.toml`. Each build records the runner image, `rustc -Vv`, the Cargo version, `xcodebuild -version`, the SDK version, `SDKROOT`, the `Cargo.lock` hash and the build flags.
- Two separate jobs run `cargo build --release --locked`. The raw binaries are compared first, then the deterministic tarballs (sorted entries, fixed mtime, `gzip -n`).
- A release only proceeds when the tag commit is reachable from protected `main`, the tag matches the crate version, and CI passed for that commit. Every action is pinned to a full commit SHA, and workflow permissions are least privilege.
- Both the raw binary and the tarball get a GitHub artifact attestation. `SHA256SUMS` lists the tarball, and the Homebrew formula pins the same digest. `cargo audit` runs again in the release job.
- The tap pull request uses a token scoped to the tap repository only. The formula holds only `url`, `sha256` and `bin.install`.
- Dependency graph, Dependabot alerts and the dependency review action watch for new advisories without scheduled workflows.
- Rollback: the tap keeps a versioned formula for the previous minor release, and SECURITY.md tells users to install it, or to set `apply = false`, when a release is withdrawn.

## 9. Testing

### 9.1 Safety requirements and their tests

| ID | Requirement | Tests |
| --- | --- | --- |
| S1 | `apply` never signals managed, owned-live, likely-owned or unknown items, even when their ID is supplied | per-class refusal fixtures |
| S2 | No signal without approval through elicitation or the terminal challenge | contract: no elicitation capability, decline, cancel, timeout |
| S3 | Codes are single use, expire after 2 minutes and must match exactly. Empty or default content is rejected | contract: wrong code, empty content, expired code, reused code |
| S4 | The nonce must exist, be unused and unexpired, and the arguments must match | contract: unknown, malformed and replayed nonce, modified arguments, reordered, added and duplicate item IDs |
| S5 | At most 10 items per call, and the owned batch enumerates every item | contract: 11 items, prompt snapshot |
| S6 | Full reclassification after approval, and a downgrade aborts | live: process becomes managed or loses evidence during approval |
| S7 | Kernel identity and executable path match right before the signal, and a missing path fails closed | deterministic provider: identity changes between reads. Live: process exits and restarts during approval |
| S8 | SIGTERM only, one exact PID, no group, no SIGKILL | live harness signal log |
| S9 | Exactly one of several concurrent applies signals an identity | live: two servers apply the same item |
| S10 | No lock is held during approval | contract: second apply proceeds while the first waits |
| S11 | Plan expiry and server restart prevent any signal | contract: expired plan, restart during approval |
| S12 | Terminal apply refuses without terminals, controlling terminal or foreground group, or under an agent ancestor | PTY tests for each condition and one success case |
| S13 | Hooks exit 0, print nothing and stay within section 4.4 limits | performance: normal and 1, 10, 100 MB payloads in each host |
| S14 | Raw tags, commands, output and environments are never persisted | privacy invariant test |
| S15 | `ModelFinding` holds no command text and no path | privacy invariant test on tool output |
| S16 | Startup validation refuses unsafe data directory state | symlinked secret, foreign owner, loose modes |
| S17 | Setup never edits a symlinked or non-regular file, never removes what it did not create, and verifies native command results | setup fixtures for each case, including a stubbed CLI that exits 0 without changing anything |
| S18 | Unknown schema or manifest versions fail closed | journal and manifest from a future version |
| S19 | Provenance failures only lower confidence | hook failures injected, classifier output compared |
| S20 | A release only comes from protected `main` with passing CI, pinned actions and attested artifacts | release workflow dry run on a fork |

Every normative "never", "must" or "refuse" in this document maps to a row above. A new rule adds a row before it is implemented.

### 9.2 Layers and cadence

| Layer | Scope |
| --- | --- |
| Unit and property | identity, journal codec, state machine, classifier rules, sanitiser |
| Fuzz | `KERN_PROCARGS2` parser, journal decoder, hook payload parsers, sanitiser, config patchers |
| Recorded fixtures | ground-truth labels: `true_owned_ended`, `true_live_owned`, `true_detached`, `true_managed`, `true_unknown` |
| Live harness | spawns real process trees and signals only PIDs it created, including SIGTERM-ignoring and detaching children |
| Stress | three concurrent journal writers, rotation under load, PID churn |
| Release | two-job reproducibility check, clean-account install checklist |

Every pull request compiles all fuzz targets and runs their regression corpora. Before each release, the procargs, payload and config parsers are fuzzed for at least 10 minutes each. PID reuse is tested deterministically through a simulated process provider, because a live churn test can pass without any reuse happening.

CI on every pull request runs one Linux job (fmt, clippy, unit and property tests for platform-independent code, `cargo deny`, `cargo audit`, fuzz regression) and one macOS job (build, live harness, stress, contract tests, performance budget). There are no scheduled workflows.

### 9.3 Support bundle

`hygiene support-bundle` writes a versioned, local, redacted JSON file (version, macOS version, detected agents, configuration health, class counts, hook latency statistics, error codes) and prints its path. `--stdout` shows the same content without writing a file. Nothing is uploaded.

## 10. Questions answered by M0 and M1

| Question | Answered by |
| --- | --- |
| Project name | M0 availability check on GitHub, crates.io, Homebrew and npm |
| How each client renders and answers the typed-code form | M0 client matrix |
| Which process anchors a Cursor conversation | M0 experiment |
| Whether Cursor `sessionStart` env reaches shell processes | M0 experiment |
| Journal format | M1 contention benchmark |
| Suspect age and idleness thresholds | M1 fixture corpus |
