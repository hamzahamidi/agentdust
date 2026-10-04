# AgentDust design

Date: 2026-10-03. Binary `agentdust`. Companion to [ROADMAP.md](../../../ROADMAP.md), which holds milestones and success criteria.

## 1. Context

AI coding agents start processes that outlive the session that started them: stdio MCP servers, dev servers, watchers and background shell jobs. On macOS these orphans are reparented to launchd (PPID 1) and keep memory, ports and sometimes CPU. The problem is reported for every major agent:

- Claude Code: [anthropics/claude-code#1935](https://github.com/anthropics/claude-code/issues/1935), open since June 2025.
- Codex: [openai/codex#21008](https://github.com/openai/codex/issues/21008), 501 stale MCP helpers holding about 4 GB under launchd.
- Cursor: a [known issue](https://forum.cursor.com/t/mcp-process-leak-orphaned-children-on-restart/156478) confirmed by Cursor staff in April 2026.

Agent data directories also grow without bound (session transcripts, worktrees, scratch directories, VM bundles).

### Goal

A developer installs the tool with `brew install` and `agentdust setup`. Any connected agent can then analyse leftover processes and disk growth, show findings, and terminate stale processes after the developer approves each action with a typed code.

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
agentdust (bin)
 ├─ agentdust-core    std + serde: platform, journal, evidence, classify, plan, apply, sanitize
 ├─ agentdust-mcp     tokio + rmcp: agentdust_doctor, agentdust_plan, agentdust_apply
 └─ agentdust-agents  claude, codex, cursor: hook payload parsers and setup patchers
data dir: ~/Library/Application Support/agentdust (0700)
```

| Subcommand | Behaviour |
| --- | --- |
| `agentdust hook <agent>` | Synchronous. Reads one hook event, appends journal records, prints nothing, exits 0 |
| `agentdust mcp` | Stdio MCP server. Holds plans and approval state in memory |
| `agentdust doctor`, `agentdust status` | Read-only report, including provenance health and running versions |
| `agentdust apply` | Human-only terminal path over the same core (section 6.6) |
| `agentdust setup`, `agentdust support-bundle`, `agentdust version` | Local only |

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
  session_tag_key?: HMAC(secret, "AGENTDUST-SESSION-v1\0" || tag),
  cwd_key?:         HMAC(secret, "AGENTDUST-CWD-v1\0" || canonical cwd),
  exe_base?:        "node",
  procs?:           [{ pid, start_time_us, ppid, exe_base }] }
kind: session_start | session_end | shell_start | shell_end | sample | server_start
```

- Readers skip unknown kinds and fail closed on an unknown schema version.
- The raw session tag, raw commands, command output and process environments are never written.
- `mono_ts` is a system-wide monotonic clock read, used to order events within one boot. `wall_ts` is for display.
- Same-user forging of journal records is out of scope (section 7.4).

Storage: `journal.jsonl` and closed generations `journal.<stamp>.jsonl`, in a directory on a local APFS volume (section 7.3). A record is one frame: the byte 0x1E, the record as compact JSON, the byte 0x0A. An append opens `journal.jsonl` with `O_APPEND` and `O_NOFOLLOW`, writes the frame with one `write(2)` and compares `fstat` of the descriptor with `lstat` of the path. When the path names another file, or none, the frame is written again, up to 3 attempts in all. An appender takes no lock. A call interrupted by a signal before it transferred anything may be restarted. A write that returns fewer bytes than the frame is an error and is never completed by a second call. A frame longer than 65,536 bytes is dropped. Hook health (section 4.4) is planned for M2 and counts each kind of drop. A record is acknowledged when the append returns after a complete write and a recheck that finds the path still naming the file written, and an earlier write that a recheck rejected is tentative and not acknowledged. Appends are not synced: an acknowledged record can be lost on an OS crash or a power failure, and lost evidence only lowers confidence (S19). `journal.maint`, taken with `flock` by rotation and retention only, keeps two maintenance runs apart. Rotation renames `journal.jsonl` to `journal.<stamp>.jsonl`, where `<stamp>` is the wall clock in milliseconds moved up to the next free number, and never truncates or rewrites the active file. Rotation and retention open the active file and every generation with the safe opens of section 7.3 before they rename, replace or delete anything, and refuse when one is unsafe. Readers take no lock. A reader opens `journal.jsonl`, lists the generations, opens each one that still exists, collapses records that are equal in every field, and orders the rest by the wall time at which their boot first appears, then `mono_ts`, then the remaining fields in a fixed order. This order is for presentation. It does not show which event caused which, and the order of boots is weak because wall time can move backwards. A frame without its closing 0x0A, a line that does not parse and a line of an unknown kind are skipped and counted. A line of a newer schema version is counted apart, including one over the size limit, which is recognised by its leading `{"v":`.

Retention: records from earlier boots are pruned first, records of ended sessions after 14 days or when the journal passes 20 MB, and records of active sessions are pinned. When evidence for an active session is lost, its provenance is marked degraded. A session is ended only when its last `session_start` or `session_end` record is a `session_end`, so a `session_start` after an end reopens it. Retention visits each closed generation, oldest first, and never the active file. A generation that holds a line of a newer schema version is left as it is. A record whose bytes repeat those of a record read earlier in the run is dropped, and so are the records the rules above drop. A generation with nothing to drop is left as it is, and one with no line left is deleted. In any other case the lines that stay are written, with their original bytes, to `journal.compact.tmp`, which is synced and renamed over the generation, and the directory is synced. Before a rewrite or a delete drops a line that does not parse, the generation is copied byte for byte to `journal.jsonl.corrupt-<n>`. A line of an unknown kind stays. A record is never moved to another file and never appended again. No step depends on elapsed time. Only rotation and retention take `journal.maint`, and the hook never runs either.

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

The deny list contains PID 1, processes owned by UID 0 or another user, the AgentDust server itself, live agent processes, Apple system paths and helpers of running apps. Thresholds for age and idleness are set from the M1 fixture corpus.

### 3.5 Representations

| Type | Seen by | Content |
| --- | --- | --- |
| `RawIdentity` | operating system calls only | full paths, identities, command lines |
| `ModelFinding` | MCP tool results, so the model | item ID, class, `exe_base`, PID, age, evidence kinds, `cwd_relation` (`same_repo`, `other_repo`, `home`, `temp`, `other`). No command text and no path |
| `HumanDisplay` | elicitation prompts and the terminal | fixed templates. Elicitation prompts show only `ModelFinding` fields. The terminal adds the redacted command (120 characters) and cwd (80 characters). Every value has C0, C1, escape and bidi controls escaped |

Privacy for the model is structural: it never receives command text or paths, so redaction rules are defence in depth and not the guarantee. Elicitation prompts pass through the host application and can appear in its transcripts, so they carry no path-derived text either. Live data such as cwd is read from the process at display time and never taken from the journal.

## 4. Provenance and adapters

### 4.1 Agent identity

Each adapter accepts only its own host. `agentdust hook claude` walks its ancestry to the first process that matches a known Claude Code executable, and records that `AgentIdentity`. A Codex or Cursor process found on the way is ignored. If no valid anchor exists, the record carries no agent identity and can never support an owned class.

Known executables start from setup and are refreshed by every genuine adapter invocation, because an agent update replaces its binary. Cursor gets no "agent ended" upgrade until M0 shows which process lifetime covers a Cursor conversation.

### 4.2 Hook events

| Agent | Events | Tag |
| --- | --- | --- |
| Claude Code | `SessionStart`, `PreToolUse` and `PostToolUse` with a Bash matcher, `SessionEnd` | `AGENTDUST_SESSION` written to `CLAUDE_ENV_FILE` when it is present |
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
- Journal write budget: the lock is taken without blocking, with retries for at most 20 ms. On failure the record is dropped and hook health is updated. The health update follows the same budget and is best effort.

Benchmarks with 1, 10 and 100 MB payloads assert latency and that each host handles the hook normally.

A hook always exits 0 and prints nothing. Each agent has a health record (last success, last error, error count, error kind) updated under the journal lock. `doctor` and `status` report each agent as healthy, degraded or unavailable, and a provenance failure only lowers confidence.

## 5. MCP tools

| Tool | Annotations | Result |
| --- | --- | --- |
| `agentdust_doctor` | `readOnlyHint` | `ModelFinding` list, class counts, provenance health |
| `agentdust_plan` | not read-only, not destructive | opaque plan ID and actionable items only |
| `agentdust_apply` | `destructiveHint` | per-item results |

Annotations are hints for the client. The server enforces every rule itself.

## 6. Plan and apply

### 6.1 Plan

`agentdust_plan` takes a fresh inventory and keeps only owned-ended and suspect items. The plan lives in server memory under a random 128-bit ID and expires after 10 minutes or when the server exits. An inspection report (0600) is written for the user to read. It holds sanitised display data and no approval state, cannot be passed back to `apply`, is deleted at expiry, and stale copies are removed at startup.

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
| Client has no elicitation | `apply_not_supported`, with a hint to run `agentdust apply` in a terminal |
| Server restarts during approval | Plan and nonce are gone, nothing is signalled, the agent must plan again |
| Corrupt or unknown-version state | `doctor` reports what it can, owned classes are unavailable, `apply` refuses owned-ended items |
| Lost install secret | HMAC evidence is unverifiable and owned classes downgrade |
| `apply = false` in `config.toml`, or a config file that exists but cannot be read or parsed | MCP and terminal apply both refuse everything, `doctor` still works. A missing setting means enabled |

### 6.6 Terminal apply

`agentdust apply` runs doctor, plan and approval in one process with the same rules and codes. It refuses unless stdin, stdout and `/dev/tty` are terminals, a controlling terminal exists, the process is in the terminal's foreground process group, and no ancestor is a known agent. Agent recognition combines executables observed by adapters, executables recorded by setup and a built-in list of agent executable and bundle identifiers, and refuses when in doubt. No flag, environment variable or pipe supplies a code. This guards against automation by accident and is documented as such, not as proof of a human.

## 7. Security and privacy

### 7.1 Threats and controls

| Threat | Control |
| --- | --- |
| A model, injected or not, calls `apply` | Typed code through elicitation. Tool arguments cannot carry approval |
| The host auto-accepts elicitation | Required random code. Empty or default content is rejected. The host stays in the trusted computing base |
| Malicious process metadata | The model never receives text values. `HumanDisplay` escaping. Raw values never appear in approval prose |
| Secrets in commands, paths or environments | Structural: no command text or path in model output, nothing raw persisted, only `AGENTDUST_SESSION` read from an environment and hashed at once, shell output never captured. Argument redaction in terminal output is defence in depth |
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

Every command that reads trusted state first checks the data directory: it must be a real directory owned by the current UID, and the secret must have no group or world access. A wrong mode on the directory is corrected. Anything else makes the command refuse with a specific message.

Every file the tool writes (secret, journal, lock files, health, audit log, inspection reports, manifest, config) is opened with no-follow semantics and must be a regular file with a single link. New files are created exclusively. A symlink, hard link or non-regular file in their place makes the command refuse.

`agentdust setup --remove --purge-data` removes the agent integrations first and verifies the removal. Only then does it delete the data directory. If removal fails, the manifest and secret stay so the user can retry.

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

### 8.2 `agentdust setup`

Each resource has one mutation owner:

| Resource | Changed by |
| --- | --- |
| Claude Code MCP server | `claude mcp add --scope user agentdust -- <prefix>/bin/agentdust mcp` and `claude mcp remove` |
| Codex MCP server | `codex mcp add agentdust -- <prefix>/bin/agentdust mcp` and `codex mcp remove` |
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

Rules for native commands: snapshot the current state, refuse if a `agentdust` server exists that the manifest does not own, run the command, then list the configuration again and compare command and arguments exactly. An exit code of 0 alone is never success. Rollback uses the native remove command.

Ownership: the manifest (versioned) records each inserted node and a hash of the value installed. An equivalent entry that existed before setup is recorded as pre-existing and never removed. `--remove` deletes an entry only if the manifest created it and its current value still matches. An edited entry is reported as drift. An unknown manifest version makes `--remove` refuse. `--check` reports drift, conflicts and partial installs. Every path uses the stable Homebrew prefix, never the Cellar.

### 8.3 Upgrades and mixed versions

After `brew upgrade`, an old MCP server can keep running while new hooks run the new binary. Every persistent format is versioned. A process that meets a newer version only lowers confidence or refuses, and never interprets records it does not know. `agentdust status` lists running AgentDust processes with their versions.

### 8.4 Release

- Builds run on an explicitly named macOS arm64 runner image, never `macos-latest`. `DEVELOPER_DIR` and `SDKROOT` select an exact Xcode and SDK, and the toolchain is pinned in `rust-toolchain.toml`. Each build records the runner image, `rustc -Vv`, the Cargo version, `xcodebuild -version`, the SDK version, the `Cargo.lock` hash and the build flags, and the release fails if any of them differs from the expected values.
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
| S13 | Hooks exit 0, print nothing and stay within section 4.4 limits | performance: normal and 1, 10, 100 MB payloads in each host. A hook run in a directory that holds a generation full of droppable records changes no file but the active journal and takes no maintenance lock (`crates/agentdust/tests/hook_never_prunes.rs`) |
| S14 | Raw tags, commands, output and environments are never persisted | privacy invariant test |
| S15 | `ModelFinding` and elicitation prompts hold no command text and no path | privacy invariant test on tool output and prompt snapshots |
| S16 | Startup validation and safe opens refuse unsafe state for every persistent file | symlinked and hard-linked state files, foreign owner, loose modes, and on macOS an extended ACL with an allow entry. A rotation or retention run in a directory that holds an unsafe generation refuses and changes nothing |
| S17 | Setup never edits a symlinked or non-regular file, never removes what it did not create, and verifies native command results | setup fixtures for each case, including a stubbed CLI that exits 0 without changing anything |
| S18 | Unknown schema or manifest versions fail closed | journal and manifest from a future version. A journal line of a future version longer than 65,536 bytes is counted as a newer version, and retention leaves its generation as it is |
| S19 | Provenance failures only lower confidence | hook failures injected, classifier output compared |
| S20 | A release only comes from protected `main` with passing CI, pinned actions, the expected Xcode and SDK, and attested artifacts | release workflow dry run on a fork, including a toolchain drift case |
| S21 | `apply = false` or an unreadable config disables both MCP and terminal apply | contract and PTY tests with the switch set and with a malformed config |
| S22 | A journal record is one frame (the byte 0x1E, compact JSON, the byte 0x0A) written by one write call per attempt, and a frame is never longer than 65,536 bytes. Concurrent appenders never tear or interleave a line, and a cut write costs one record and never the next | `journal_frame.rs`, `journal_decode.rs`, `journal_truncation_props.rs` and `journal_append_faults.rs` in `crates/agentdust-core/tests`: byte level cuts after 0, 1, 2, half, all but 2 and all but 1 bytes, and injected short and failed writes. `journal_append.rs`: `an_append_is_one_write_call_of_one_whole_frame`, `a_frame_over_the_cap_is_refused_before_anything_is_created` and `sixteen_threads_appending_4000_byte_records_tear_and_lose_nothing`. `crates/agentdust/tests/hook.rs`: `concurrent_hooks_never_interleave_records`. `crates/agentdust-bench/tests/harness.rs`: `sixteen_writer_processes_with_4000_byte_records_tear_nothing` |
| S23 | An acknowledged record is present exactly once after any interleaving of an append with rotation, a read and retention. A record is acknowledged when its append returned success after a complete write and a recheck that found the active file still at its path. Earlier writes that a recheck rejected are tentative, and an append that ends in `Stale` is not acknowledged | `journal_append_recheck.rs`, `journal_rotate_interleave.rs`, `journal_retain_interleave.rs` and `journal_snapshot.rs` in `crates/agentdust-core/tests`: pause points after the open, before the write and after the write, a reader paused after its open and after its listing, retention deleting and compacting. Stress with a rotator, a reader and writers: `journal_append_rotation.rs`, `journal_retain_stress.rs` and `crates/agentdust-testkit/tests/journal_stress.rs` |
| S24 | The journal writes nothing on a volume that is not a local APFS volume | `journal_volume.rs` in `crates/agentdust-core/tests`: `the_injected_volume_matrix_of_s24_decides_support` with apfs local, apfs not local, hfs, devfs, nfs, smbfs and autofs. `journal_append_volume.rs`, `secret_volume.rs`, `journal_rotate.rs` and `journal_retain.rs`: no record, file or directory is created on a refused volume. `crates/agentdust/tests/hook.rs`: `a_data_directory_on_an_unsupported_volume_gets_no_record_and_no_noise` |
| S25 | A reader returns the records of a journal in one fixed order that does not depend on which file holds a record or in which order the files were opened, and it collapses records that are equal in every field | `journal_read.rs` in `crates/agentdust-core/tests`: `records_are_ordered_by_monotonic_time_and_not_by_file_position`, `records_equal_in_boot_and_stamps_get_a_fixed_order_that_does_not_depend_on_the_files`, `the_final_key_compares_every_field_in_declaration_order_starting_with_the_kind` and `exact_duplicates_in_one_file_and_across_files_collapse_and_are_counted` |

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

`agentdust support-bundle` writes a versioned, local, redacted JSON file (version, macOS version, detected agents, configuration health, class counts, hook latency statistics, error codes) and prints its path. `--stdout` shows the same content without writing a file. Nothing is uploaded.

## 10. Questions answered by M0 and M1

| Question | Answered by |
| --- | --- |
| How each client renders and answers the typed-code form | M0 client matrix |
| Which process anchors a Cursor conversation | M0 experiment |
| Whether Cursor `sessionStart` env reaches shell processes | M0 experiment |
| Journal format | M1 contention benchmark |
| Suspect age and idleness thresholds | M1 fixture corpus |
