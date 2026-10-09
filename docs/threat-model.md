# Threat model

AgentDust finds processes that AI coding agents leave behind and signals them through manual typed approval or a previously enabled automatic directory policy. This page says who or what could make it do harm, what they can reach, what stops them and what risk is left.

It has twelve sections against [the design spec](superpowers/specs/2026-10-03-agentdust-design.md). Sections 1 to 8 are the eight adversaries that [ROADMAP.md](../ROADMAP.md) names for M1. Sections 9 to 12 cover the storage under the journal. Their failures come from the environment, from an accident, from a planted file or from the timing of a short-lived process, so they are named for the failure and not for one attacker: an unsupported filesystem, journal poisoning and short writes, stale writers and rotation, and durability limits.

Numbers such as 6.4 are spec sections. IDs such as S7 are the safety requirements of spec section 9.1. [ADR-1](m1/adr-journal-format.md) proposes three more rows for 9.1, for the journal. They are not in the spec, so no row here cites them, and the journal controls they would cover say `none` in the Requirement column.

The page describes the repository after M1, M2 and M3. M1 holds the process and journal primitives. M2 adds provenance, classification, the doctor command, plan and apply core. M3 adds Claude Code setup, status and release automation. The apply clients use the same core approval protocol ([apply](m3/apply.md), [setup](m3/setup.md)). Rows marked planned identify work that remains.

## How to read this page

Each adversary section has four parts: what the adversary can do, what is at risk, a table of controls, and the risk that remains. A control table has five columns.

| Column | Meaning |
| --- | --- |
| Control | What is done about the threat |
| Spec | The spec section that defines it |
| Requirement | The S-id of spec 9.1 that it serves, or `none` |
| Status | `implemented`, `planned M2` or `planned M3` |
| Tests | For an implemented row, the repository paths of the code and tests that pin it. For a planned row, the test that will pin it, in words |

A control that is half built appears as two rows: the built half as `implemented` and the rest as planned. A planned test is described in words, because no file exists for it yet. In an implemented row, every code span that contains a slash is read as a repository path. A file in the repository root has no slash, so a name such as `deny.toml` is not checked.

`scripts/check_threat_model.py` enforces the page. It requires the twelve section headings, a "Residual risk." part with at least one bullet in each of them, a Requirement cell of `none` or S-ids in every control row, and every S-id of spec 9.1 in the Requirement cell of at least one row. It rejects an S-id that spec 9.1 does not define, and a path in an implemented row that is not in the repository. The Linux CI job runs it together with its tests.

## Limits that stay open

These hold whatever else is built. Each is stated again, with its numbers, in the section named.

- **PID reuse (section 5).** Between the identity read and the `kill` call there is a window of microseconds in which the target can exit and its PID go to another process of the same user. The design narrows the window and does not close it: macOS has no pidfd and `kill` takes a number.
- **Programs of the same user (sections 4, 10, 11).** Anything that runs as the user can forge journal records, read `install.secret`, plant or hold files in the data directory and signal processes without AgentDust. Nothing authenticates a journal writer. Spec 7.4 puts this out of scope, and the controls only bound the effect.
- **The host (sections 1, 3).** The application that shows the approval form stays in the trusted computing base. A host that answers the form itself defeats approval.
- **Durability (section 12).** An acknowledged journal record can be lost on an OS crash or a power failure, and nothing promises otherwise.
- **Lost evidence (section 12).** Missing ownership evidence moves a process out of the owned classes. By the table in spec 3.4 it can then be a suspect, which can be signalled after a per-item code. Lower confidence means "not known to be owned". It does not mean "cannot be signalled".
- **Other volumes (section 9).** The journal is written only on a local APFS volume. A home directory on NFS or SMB gets no journal, and no network volume was available to observe the refusal.
- **Shared build inputs (section 8).** Two identical builds can share one malicious input.

## Assets and trust

| Asset | What goes wrong if it is lost |
| --- | --- |
| The user's running processes | A wrong SIGTERM ends work that was not abandoned |
| Cleanup authority | A process is signalled without the person's manual approval or outside an enabled automatic policy |
| Commands, paths, environments and command output | They can hold secrets. The design never stores them (7.2) and never shows them to the model (3.5) |
| The data directory: the journal, its generations and `install.secret` | The journal is the evidence for ownership decisions and the secret keys its digests |
| Journal records | A lost record lowers what is known about a process. A forged one can raise it |
| Agent latency | Hooks run inside every agent turn (4.4) |
| The release artifact | It runs as the user |

Trusted without checking: the macOS kernel and the login session, the host application that shows the approval form to the person (7.1 keeps it in the trusted computing base), the person at the keyboard, GitHub with its hosted runners for the release, and the semantics of a local APFS volume: an `O_APPEND` write lands whole at the end of the file, `rename` is atomic, and `fstat` and `lstat` report the file that is there. The last is stress tested and is not an APFS guarantee (section 11).

Untrusted: everything a model writes, process metadata, the contents of hook payloads, any file in the data directory until it is opened safely, the volume under the data directory until `statfs` reports it local APFS, and the release artifact until it is verified.

## 1. Malicious model

**What it can do.** A model that is prompt-injected, or only wrong, controls every tool argument it sends and every word it tells the person. It can call `agentdust_doctor`, `agentdust_plan` and `agentdust_apply` in any order and as often as it likes. It can send plan IDs, item IDs and nonces it was never given, guess codes, and ask the person to approve. It can run shell commands through the agent, so it chooses what the agent hands to the hook, including output of any size.

**At risk.** The user's processes, approval authority, the commands and paths the model might try to read back through AgentDust, hook latency and disk space.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| Approval is a code the person types into a form the host shows, and tool arguments cannot carry it. The M0 probe implements the form: 4 characters from a 25 character alphabet, an exact match, and a refusal for a missing form capability, a wrong code, an empty code, decline and cancel | 6.3, 7.1 | S2, S3 | implemented | `crates/agentdust-mcp/tests/probe.rs`, `crates/agentdust-mcp/tests/codes.rs`. The forged retry run with Codex 0.156.1 is recorded in `docs/m0/client-matrix.md`: the model did not answer the form in 60 seconds |
| `agentdust_apply` accepts approval only through elicitation or the terminal challenge | 6.3, 6.6 | S2, S3 | implemented | `crates/agentdust-core/src/apply/server.rs`, `crates/agentdust-core/tests/apply_server.rs`, `crates/agentdust-core/tests/apply_server_run.rs`, `crates/agentdust-mcp/src/tools.rs`, `crates/agentdust/src/apply.rs` |
| Only owned-ended and suspect items can be signalled. One total function maps each class to its actionability, and the corpus expects nothing that must never be signalled to be actionable | 3.4, 6.2 | S1 | implemented | `crates/agentdust-core/src/class.rs`, `crates/agentdust-core/tests/class.rs`, `crates/agentdust-testkit/tests/fixture_corpus.rs` |
| `apply` refuses managed, owned-live, likely-owned and unknown items even when the model supplies their ID | 6.2 | S1 | implemented | `crates/agentdust-core/src/plan.rs`, `crates/agentdust-core/tests/plan.rs`, `crates/agentdust-core/tests/apply_server.rs` |
| At most 10 items per call, and the owned batch prompt lists every item | 6.2 | S5 | implemented | `crates/agentdust-core/src/apply/server.rs`, `crates/agentdust-core/tests/apply_server.rs` |
| `ModelFinding` carries typed fields only (item ID, class, `exe_base`, PID, age, evidence kinds, `cwd_relation`), so the model never receives command text or a path | 3.5, 7.1 | S15 | implemented | `crates/agentdust-core/tests/finding.rs`, `crates/agentdust-core/tests/doctor_report.rs`, `crates/agentdust-core/tests/apply_privacy.rs` |
| Commands and command output that the model chooses are never written. The hook reads a payload as a stream, keeps a fixed set of fields and drops the rest unread. The journal has 13 named keys, and a key outside that list fails the test. The privacy test reads every byte of the data directory after the hooks, after a rotation and after a retention rewrite, and what the run left in the system temp directories | 3.2, 7.2 | S14 | implemented | `crates/agentdust/tests/privacy.rs`, `crates/agentdust-core/tests/journal_keys.rs`, `crates/agentdust/tests/hook.rs`, `docs/m1/privacy-test.md` |
| The hook reads a payload of any size without holding it, drains stdin so the host never sees a closed pipe, exits 0 and prints nothing. An ignored release test asserts p50 under 10 ms and p95 under 20 ms | 4.4 | S13 | implemented | `crates/agentdust/tests/hook.rs`, `crates/agentdust/tests/hook_latency.rs`, `crates/agentdust-agents/tests/claude_bounds.rs`, `crates/agentdust-agents/tests/claude_leading_value.rs`, `fuzz/fuzz_targets/claude_payload.rs` |
| Payloads of 1, 10 and 100 MB through the Claude Code host, with latency and normal host behaviour asserted | 4.4, 9.1 | S13 | planned M2 | Performance test with each size (Codex follows in M4 and Cursor in M5) |
| Terminal `apply` refuses without a terminal on stdin, stdout and its controlling terminal, outside the foreground process group, and under an agent ancestor. This guards against automation by accident and is not proof of a human | 6.6 | S12 | implemented | `crates/agentdust/src/apply.rs`, `crates/agentdust/tests/apply_terminal.rs` |
| The only signal is SIGTERM to one exact PID: no process group and no SIGKILL | 6.4, 7.1 | S8 | implemented | `crates/agentdust-core/src/apply/signal.rs`, `crates/agentdust-core/tests/apply_signal.rs`, `crates/agentdust-testkit/tests/apply_live.rs` |

**Residual risk.**

- The host stays in the trusted computing base (7.1). It shows the code to the person. A host that fills the form itself, or hands the form text to the model and lets the model answer, defeats approval. Codex 0.156.1 on protocol 2025-06-18 and Claude Code 2.1.289 on protocol 2026-07-28 did not do this in the recorded runs. Cursor has not been tested.
- A model with free shell access can signal any process of the user without AgentDust and can open a pseudo-terminal (7.4). The ancestor check of 6.6 makes running `agentdust apply` that way harder and does not prove a human is present.
- The model can ask the person to type the code. The damage is bounded: at most 10 items per call, processes of the same user, SIGTERM only, and a separate code for each suspect.
- A model that runs shell commands in a loop makes the hook append one record per session or Bash tool event, each at most 65,536 bytes. Nothing calls `prune`, so the journal grows until a caller exists. The limit of 20,000,000 bytes is the default policy of `prune` and applies only when it runs.

## 2. Malicious process metadata

**What it can do.** Any process of the user picks its own executable name and path, argument vector, environment and working directory. It can fill them with control characters, ANSI escapes, bidirectional overrides, newlines, text that reads like an instruction to a model, or very large blocks. It can fork, daemonize or reparent itself to look detached, take the name of a system service, or exit and be replaced. A hostile repository's build script can do all of this from inside a session.

**At risk.** The model's context as an injection channel, the person's terminal and approval prompt, the parser's memory and time, and the classifier's answer.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| `KERN_PROCARGS2` parsing is bounded. A short buffer, a negative argc, an argc larger than the arguments present, an exec path without a terminator and a huge argc return errors or small allocations. The parser is fuzzed | 9.2 | none | implemented | `crates/agentdust-core/src/procargs.rs`, `crates/agentdust-core/tests/procargs.rs`, `crates/agentdust-core/tests/procargs_corpus.rs`, `fuzz/fuzz_targets/procargs.rs`, `docs/m0/report.md` |
| An environment is never stored. The parser reads session markers by exact name, the keyed digest exists, and the hook reads no other process's environment. The privacy test puts a sentinel in the hook's `AGENTDUST_SESSION` and finds it in no file | 3.2, 4.3, 7.1 | S14 | implemented | `crates/agentdust-core/tests/procargs.rs`, `crates/agentdust-core/tests/digest.rs`, `crates/agentdust/tests/privacy.rs` |
| Sampling reads `AGENTDUST_SESSION` or `CODEX_SESSION_ID` from a process, hashes the value in its adapter domain into `session_tag_key` and drops the raw value | 3.2, 4.3 | S14 | implemented | `crates/agentdust-testkit/tests/inventory_live.rs`, `crates/agentdust-core/tests/procargs.rs` |
| Terminal and prompt text come from fixed templates, every value has C0, C1, escape and bidi controls escaped, and no process text appears in an elicitation prompt | 3.5, 7.1 | S15 | implemented | `crates/agentdust-core/src/finding.rs`, `crates/agentdust-core/src/apply/server.rs`, `crates/agentdust-core/tests/apply_privacy.rs`, `crates/agentdust-core/tests/finding_display.rs` |
| Values the journal keeps from processes are validated by type. `exe_base` is at most 64 bytes with no C0, C1 or bidi controls, and `cwd_key` is lowercase hex. A line that breaks a rule is malformed on read | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_record.rs`, `crates/agentdust-core/tests/journal_decode.rs`, `crates/agentdust-core/tests/journal_codec_props.rs` |
| The classifier does not trust one signal. A loaded launchd job, a Homebrew service, an app helper and an exact deny identifier are managed and never actionable, and a parent of launchd is one input among several | 3.4 | S1 | implemented | `crates/agentdust-testkit/tests/classify_fixtures.rs`, `crates/agentdust-testkit/tests/fixture_corpus.rs`, `crates/agentdust-core/tests/classifier_provenance.rs` |
| The corpus carries the protected cases as data: PID 1, another user's process, the agent itself, a launchd job, a Homebrew service and an app helper | 3.4, 9.2 | S1 | implemented | `fixtures/m1`, `crates/agentdust-testkit/tests/fixture_corpus.rs`, `docs/fixtures.md` |

**Residual risk.**

- The terminal shows a redacted command (120 characters) and the working directory (80 characters) after escaping. Both are text the process chose and can still mislead a reader. Elicitation prompts carry no path-derived text for that reason.
- A process can arrange to look detached, with a parent of launchd and a session of its own, and so become a suspect. The per-item code is the control, and a deliberately detached tunnel is a known case.
- A process inside a session inherits that session's tag, so it is owned by design. A process can only copy another session's tag by reading that session's environment, which is same-user access (section 4).
- The fuzz runs are smoke runs: 61 seconds for the `KERN_PROCARGS2` parser and 16 seconds per target for the journal decoder, the journal reader and the payload parser. The 10 minute run that spec 9.2 asks for before a release is not part of M1.

## 3. Buggy MCP client

**Scope.** A client that lies about elicitation, or answers it on purpose while the person is absent, is a malicious client and is out of scope (7.4). This section covers a client that is wrong by accident.

**What it can do.** Auto-accept a form with an empty or default answer, render no code or cut the message, return `cancelled` for a valid form because it dislikes the schema, have no elicitation support, replay or reorder a request, retry a call after its own timeout, drop or rewrite the request state, cache `tools/list` wrongly, keep the prompt text in its transcript, start two servers, or reconnect during approval.

**At risk.** Approval integrity, the user's processes, and the privacy of prompt text in host transcripts.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| An empty answer, a wrong code, decline and cancel refuse. The code uses an alphabet without look-alike characters and only an exact match approves. A timeout is recorded in the client matrix | 6.3 | S3 | implemented | `crates/agentdust-mcp/tests/codes.rs`, `crates/agentdust-mcp/tests/probe.rs`, `docs/m0/client-matrix.md` |
| Apply codes are single use and valid for 2 minutes. An exact match is required and empty or default content is rejected | 6.3 | S3 | implemented | `crates/agentdust-core/src/apply/server.rs`, `crates/agentdust-core/tests/apply_server.rs` |
| A client without elicitation gets a refusal and never an approval | 6.5 | S2 | implemented | `crates/agentdust-mcp/tests/probe.rs` |
| `apply` answers `apply_not_supported` and hints at `agentdust apply` in a terminal. Decline, cancel and timeout are one refusal, because Codex offers only Esc | 6.5 | S2 | implemented | `crates/agentdust-mcp/src/tools.rs`, `crates/agentdust-core/tests/apply_server.rs` |
| An unknown request state, a replayed one, and a retry without the approval response are rejected, and a state is consumed on first use | 6.3 | S4 | implemented | `crates/agentdust-mcp/tests/probe.rs` |
| `apply` checks that the nonce exists, is unused and unexpired, and that the arguments match. Modified, reordered, added and duplicate item IDs fail | 6.3 | S4 | implemented | `crates/agentdust-core/src/apply/server.rs`, `crates/agentdust-core/tests/apply_server.rs` |
| Two client quirks are pinned. The form schema carries only `$schema`, `type`, `properties` and `required`, because Codex cancels anything else without showing it. The tool list result carries `ttlMs` and `cacheScope`, because Claude Code rejects a list without them | 5, 6.3 | none | implemented | `crates/agentdust-mcp/tests/probe.rs`, `docs/m0/client-matrix.md` |
| Every `apply` form is built by one schema function, and the server enforces each rule itself whatever a client does with annotations such as `destructiveHint` | 5, 6.3 | S2 | implemented | `crates/agentdust-mcp/src/tools.rs`, `crates/agentdust-core/tests/apply_server.rs` |
| Elicitation prompts show only `ModelFinding` fields, because a host can keep them in its transcript | 3.5 | S15 | implemented | `crates/agentdust-core/src/apply/server.rs`, `crates/agentdust-core/tests/apply_privacy.rs` |

**Residual risk.**

- The code reaches the host in the clear. A client that reads it from the message and fills the form, or forwards it to the model, approves without the person. This is the trusted host assumption of 7.1, and AgentDust cannot detect it.
- The client matrix has two tested clients. Codex 0.156.1 on protocol 2025-06-18 and Claude Code 2.1.289 on protocol 2026-07-28 completed their recorded scenarios. Cursor is not installed on the test machine.
- Protocol 2026-07-28 delivers the code inside the tool result. Claude Code 2.1.289 completed the retry-path scenarios without observed model approval; Cursor remains untested.
- Codex has no decline control, so Esc is the only refusal a Codex user can give.

## 4. Same-user tampering

**Scope.** Spec 7.4 puts a same-user or root attacker out of scope, including one that forges journal records. This section covers what the code does about accident, corruption and low-effort planting by another program of the user: a symlink, a hard link, a FIFO, a loose mode, a deleted or replaced secret. It does not stop an attacker who runs as the user. Sections 10 and 11 cover the journal bytes and the rotation files.

**What it can do.** Replace the data directory or a file in it with a symlink, a hard link or a FIFO. Loosen modes. Plant files named like journal generations, the maintenance lock or the compaction file. Delete or replace `install.secret`. Point a configuration file that `setup` edits at another file. Append valid lines to the journal.

**At risk.** Journal accuracy, the install secret, files elsewhere on disk that a redirected write could reach, agent configuration, and the privacy of the data directory.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| Every persistent file that exists is opened with `O_NOFOLLOW` and `O_NONBLOCK` and then judged on the descriptor: a regular file, one link, owned by the effective user, no mode bit outside 0600 (0700 for a directory), and no extended ACL allow entry on macOS. A FIFO is refused at once. The hook writes nothing and exits 0 when it meets any of these | 7.3 | S16 | implemented | `crates/agentdust-core/src/safe_open.rs`, `crates/agentdust-core/src/acl.rs`, `crates/agentdust-core/tests/safe_open.rs`, `crates/agentdust-core/tests/safe_open_dir.rs`, `crates/agentdust-core/tests/safe_open_acl.rs`, `crates/agentdust-core/tests/journal_append.rs`, `crates/agentdust/tests/hook.rs` |
| A reader does not follow an unsafe generation. It counts the file in `unsafe_files` and reads the rest. An unsafe active file or data directory fails the read | 7.3 | S16 | implemented | `crates/agentdust-core/tests/journal_read.rs` |
| Rotation and retention refuse before any change when the data directory, `journal.maint`, the active file or any generation is a symlink, a FIFO, a directory, hard linked or too loose. A FIFO is refused without blocking | 7.3 | S16 | implemented | `crates/agentdust-core/tests/journal_rotate.rs`, `crates/agentdust-core/tests/journal_retain.rs` |
| `manifest.json` and `config.toml` are opened with the same checks. A symlink, a hard link, a directory or a mode outside 0600 makes `setup` refuse and makes the apply switch read as unusable, which disables apply | 7.2, 7.3 | S16 | implemented | `crates/agentdust-core/src/manifest.rs`, `crates/agentdust-core/src/config.rs`, `crates/agentdust-core/tests/manifest.rs`, `crates/agentdust-core/tests/config.rs`, `crates/agentdust/tests/status.rs` |
| Audit, inspection and lock files use the same safe opens. Symlinks, hard links, foreign ownership and loose modes refuse the operation | 7.2, 7.3 | S16 | implemented | `crates/agentdust-core/tests/apply_audit.rs`, `crates/agentdust-core/tests/apply_lock.rs`, `crates/agentdust-core/tests/plan.rs`, `crates/agentdust-core/tests/safe_open_private.rs` |
| Commands that read trusted state refuse an unsafe data directory without repairing it | 7.3 | S16 | implemented | `crates/agentdust-core/tests/doctor_sessions.rs`, `crates/agentdust-core/tests/safe_open_dir.rs` |
| `install.secret` holds 32 bytes from `getentropy`, is installed by an exclusive rename so it never exists with fewer bytes, and is never replaced while present. A present but unusable secret is an error and the file is left alone | 7.2 | S16 | implemented | `crates/agentdust-core/tests/secret.rs`, `crates/agentdust-core/tests/secret_install.rs`, `crates/agentdust-core/tests/secret_visibility.rs`, `crates/agentdust/tests/hook_cwd.rs` |
| An unknown manifest version, a field the version does not define or a version that is not a whole number makes `setup`, `setup --check` and `setup --remove` refuse before they read `settings.json` or run the claude CLI | 8.2 | S18 | implemented | `crates/agentdust-core/tests/manifest.rs`, `crates/agentdust-agents/tests/claude_setup.rs`, `crates/agentdust/tests/setup.rs` |
| A lost or unusable evidence source lowers confidence and never raises it. With an unusable secret the hook writes the record without `cwd_key`, leaves the file alone and exits 0 | 4.4, 6.5 | S19 | implemented | `crates/agentdust/tests/hook_cwd.rs`, `crates/agentdust/tests/hook.rs` |
| Missing or degraded provenance lowers confidence and never raises it | 6.5 | S19 | implemented | `crates/agentdust-core/tests/classifier_provenance.rs`, `crates/agentdust-core/tests/doctor_report.rs`, `crates/agentdust-testkit/tests/apply_end_to_end.rs` |
| `setup` never edits a symlinked, hard-linked or non-regular file, never removes an entry it did not create, and checks a native command by listing the configuration again. A stub CLI that exits 0 without changing anything, one that registers another command and one whose listing cannot be read each fail the step and roll back the earlier ones | 8.2 | S17 | implemented | `crates/agentdust-core/tests/user_file.rs`, `crates/agentdust-agents/tests/hook_config.rs`, `crates/agentdust-agents/tests/native_cli.rs`, `crates/agentdust-agents/tests/claude_setup.rs`, `crates/agentdust/tests/setup.rs` |
| Everything in the data directory is 0600 inside a 0700 directory, and a new kind of file fails the privacy test until someone gives it a rule | 7.2 | none | implemented | `crates/agentdust/tests/privacy.rs` |

**Residual risk.**

- Nothing authenticates a journal writer. A program that runs as the user can append well-formed records, and it can read `install.secret` and test guesses against `cwd_key`. Spec 7.4 accepts both. The effect is bounded: a forged record can at most change the class a process is given, a signal still needs fresh identity and ownership checks plus either manual typed approval or satisfaction of the enabled automatic directory policy, and the target is a process of the same user that the attacker could already signal.
- `O_NOFOLLOW` covers the last path component. A symlink in an ancestor of the data directory is followed, and files are opened by path after the directory check. A same-user program that swaps the directory between the check and the open can redirect one write to another file of the same user that passes the owner, mode and link checks.
- The foreign-owner refusal is tested by giving the check another expected user, not with a second account.
- One planted file can stop rotation and retention until a person removes it, and the other same-user effects on the journal are in sections 10 and 11.

## 5. PID reuse

**What it can do.** The kernel reuses PIDs. Between the inventory, the approval and the signal, a target can exit and a different process can take its PID: an important process of the user, a restarted copy of the same program, or a process that an attacker started on purpose by churning PIDs. After a reboot every old PID is meaningless.

**At risk.** The user's processes.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| A signal needs the boot session UUID, PID, start time in microseconds, UID and executable path to equal one fresh read. The first field that differs is named, the path is compared byte for byte, and an unreadable path, a failed read and a gone process all fail closed | 3.1, 6.4 | S7 | implemented | `crates/agentdust-core/src/identity.rs`, `crates/agentdust-core/src/revalidate.rs`, `crates/agentdust-core/tests/revalidate.rs`, `crates/agentdust-core/tests/revalidate_props.rs`, `crates/agentdust-core/tests/identity.rs` |
| PID reuse is tested deterministically. A scripted provider returns a different identity on the second read: a new start time, another user, another executable, the same binary restarted, a path that becomes unreadable. A live churn test can pass without any reuse | 9.2 | S7 | implemented | `crates/agentdust-core/tests/revalidate.rs`, `crates/agentdust-core/tests/common/mod.rs` |
| The macOS provider reads identity, then path, then identity again, so a PID reused between the reads is never reported as present, even by the same binary | 6.4 | S7 | implemented | `crates/agentdust-core/src/provider.rs`, `crates/agentdust-core/src/darwin.rs`, `crates/agentdust-core/tests/provider.rs` |
| A live fixture revalidates as a match, and once it has exited as gone | 3.1 | S7 | implemented | `crates/agentdust-testkit/tests/identity.rs`, `crates/agentdust-testkit/tests/harness.rs` |
| A PID of 0 or below is never read and never matches, so it cannot reach `kill`, which with 0 or -1 addresses a process group or every process | 6.4 | S8 | implemented | `crates/agentdust-core/tests/revalidate.rs` |
| The boot session UUID makes every PID of an earlier boot invalid, and retention drops earlier-boot records on every run | 3.1, 3.2 | S7 | implemented | `crates/agentdust-core/tests/revalidate.rs`, `crates/agentdust-core/tests/journal_retention.rs`, `crates/agentdust-core/tests/journal_retain_rules.rs` |
| The test harness signals only processes it started. Every signal goes through one private function that looks the PID up in its registry, revalidates the stored identity and logs the signal only after `kill` succeeded. A recycled PID, an unregistered PID and an exited process are refused | 9.2 | S8 | implemented | `crates/agentdust-testkit/tests/harness.rs`, `crates/agentdust-testkit/tests/harness_tree.rs`, `crates/agentdust-testkit/src/harness/tests.rs` |
| `apply` revalidates and calls `kill(pid, SIGTERM)` back to back, with no logging, allocation or I/O between them, and reports `ESRCH` as gone | 6.4 | S7 | implemented | `crates/agentdust-core/src/apply/exec.rs`, `crates/agentdust-core/tests/apply_exec.rs`, `crates/agentdust-testkit/tests/apply_live.rs` |
| A live PID churn stress run beside the deterministic test | 9.2 | none | planned M3 | Stress run that churns PIDs while a plan is applied |

**Residual risk.**

- **This race remains possible.** Between the identity read and the `kill` call there is a window of microseconds. If the target exits in that window and the kernel gives its PID to a new process of the same user, that process receives SIGTERM. macOS has no pidfd and `kill` takes a number, so the design narrows the window (6.4) and cannot close it. The test harness has the same window when it signals a fixture child.
- The harm is bounded: one process, the same user (a signal to another user's process fails), and SIGTERM only, which a process can handle.
- A new process with the same boot session and PID has a later start time. An `exec` keeps all three and changes the path, which the path check catches. A process that executes the same binary again keeps its identity and is the same process.

## 6. Concurrent apply

**What it can do.** Two MCP servers (each agent starts its own), the terminal command and a client that retries after its own timeout can all try to apply the same item, or overlapping plans, at once. A model can send parallel calls. Hooks from several agents can write the journal in the same instant, and a rotation can run beside them (section 11).

**At risk.** A second SIGTERM after the first can land on a recycled PID. Approval state and journal integrity are also at risk.

Spec 3.2 and 4.4 describe a `flock` taken by every journal writer. The code follows [ADR-1](m1/adr-journal-format.md), under which appenders and readers take no lock.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| A non-blocking per-identity `flock` is taken before an item is applied, and a second server reports "handled elsewhere" | 6.4 | S9 | implemented | `crates/agentdust-core/src/apply/lock.rs`, `crates/agentdust-core/tests/apply_lock.rs`, `crates/agentdust-testkit/tests/apply_live.rs` |
| No lock is held during approval, so a second apply proceeds while the first waits | 6.3 | S10 | implemented | `crates/agentdust-core/tests/apply_server.rs`, `crates/agentdust-testkit/tests/apply_live.rs` |
| The nonce is consumed before any signal | 6.3 | S4 | implemented | `crates/agentdust-core/tests/apply_server.rs` |
| Journal appends take no lock. One `write(2)` of one whole frame goes to an `O_APPEND` descriptor, at most 65,536 bytes. Sixteen hook processes started together leave no torn line and no duplicate record, and sixteen threads appending 4,000 byte records tear and lose nothing | 3.2, 4.4, 9.2 | none | implemented | `crates/agentdust/tests/hook.rs`, `crates/agentdust-core/tests/journal_append.rs`, `crates/agentdust-core/tests/journal_append_rotation.rs` |
| A held `journal.lock` or `journal.maint` stops neither a hook nor a read, because appenders and readers open no lock file | 3.2, 4.4 | none | implemented | `crates/agentdust-core/tests/journal_append.rs`, `crates/agentdust/tests/hook.rs`, `crates/agentdust/tests/hook_never_prunes.rs` |
| Hooks started together on an empty directory create one install secret and all key with it | 7.2 | none | implemented | `crates/agentdust/tests/hook_cwd.rs`, `crates/agentdust-core/tests/secret_install.rs`, `crates/agentdust-core/tests/secret_visibility.rs` |

**Residual risk.**

- `flock` is advisory and local to the machine. It does not stop a signal sent by another tool, and a same-user program can hold the lock files to block `apply`. That denies service and never causes a kill.
- An apply that starts after another has finished sees a gone process or a new identity, and revalidation refuses it. The PID reuse window of section 5 applies to each apply.
- The atomic append is an assumption about POSIX append semantics that was stress tested. Section 11 gives its limits.

## 7. Stale plans

**What it can do.** A plan can be applied minutes after it was made. In between, a target can exit and its PID be reused, a process can become managed or lose its evidence, an agent can resume, the server can restart, the user can set `apply = false`, a client or a model can replay an old plan ID, and the inspection report can be copied back to `apply`.

**At risk.** A wrong process is signalled because the world changed since the person looked.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| A plan lives in server memory under a random 128-bit ID and expires after 10 minutes or when the server exits. An unknown, expired or restarted plan signals nothing. Plans are never written to disk, so none survives a restart or an upgrade | 6.1, 6.5 | S11 | implemented | `crates/agentdust-core/src/plan.rs`, `crates/agentdust-core/tests/plan.rs`, `crates/agentdust-core/tests/apply_server.rs` |
| The inspection report (0600) holds sanitised display data and no approval state, cannot be passed back to `apply`, is deleted at expiry, and stale copies are removed at startup | 6.1 | none | implemented | `crates/agentdust-core/tests/plan.rs`, `crates/agentdust-core/tests/apply_privacy.rs` |
| After approval each item is reclassified from scratch, including managed detection, and a change to a less certain class aborts the item | 6.4 | S6 | implemented | `crates/agentdust-core/tests/apply_server.rs`, `crates/agentdust-testkit/tests/apply_live.rs`, `crates/agentdust-testkit/tests/apply_end_to_end.rs` |
| `apply = false`, or a configuration file that exists and cannot be read or parsed, refuses everything for both MCP and terminal apply. The setting is read when `apply` runs | 6.5 | S21 | implemented | `crates/agentdust-core/tests/config.rs`, `crates/agentdust-core/tests/apply_server.rs`, `crates/agentdust/tests/status.rs` |
| Evidence for an active session is pinned when the journal is trimmed, and the loss of such evidence is listed in the `prune` report as degraded provenance | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_retention.rs`, `crates/agentdust-core/tests/journal_retain_rules.rs` |
| One framed journal record is written in one append call, is capped at 65,536 bytes, and short or failed writes cost only that record | 3.2, 4.4, 9.2 | S22 | implemented | `crates/agentdust-core/tests/journal_frame.rs`, `crates/agentdust-core/tests/journal_append_faults.rs`, `crates/agentdust-core/tests/journal_truncation_props.rs`, `crates/agentdust/tests/hook.rs` |
| An acknowledged append remains present exactly once across append, read, rotation and retention interleavings | 3.2, 9.2 | S23 | implemented | `crates/agentdust-core/tests/journal_append_recheck.rs`, `crates/agentdust-core/tests/journal_rotate_interleave.rs`, `crates/agentdust-core/tests/journal_retain_interleave.rs`, `crates/agentdust-core/tests/journal_snapshot.rs` |
| Journal writes and secret operations require a local APFS volume | 3.2, 7.2, 7.3 | S24 | implemented | `crates/agentdust-core/tests/journal_volume.rs`, `crates/agentdust-core/tests/journal_append_volume.rs`, `crates/agentdust-core/tests/secret_volume.rs` |
| Journal readers return a deterministic order and collapse exact duplicate records | 3.2 | S25 | implemented | `crates/agentdust-core/tests/journal_read.rs` |
| `doctor` shows degraded provenance and owned classes follow it | 3.2, 6.5 | none | implemented | `crates/agentdust-core/tests/doctor_report.rs`, `crates/agentdust-core/tests/doctor_sessions.rs`, `crates/agentdust-core/tests/classifier_provenance.rs` |

The identity check right before the signal (section 5) is what catches an exit and PID reuse since the plan was made.

**Residual risk.**

- The person approves what was displayed, using live data read at display time (3.5). A plan can still be 10 minutes old when it is applied, and a code stays valid for 2 minutes.
- Revalidation compares five identity fields and the class. It cannot see that the user started to rely on a process after the plan was made, when nothing it compares has changed.
- `degraded` is a list in the `prune` report. It is not written to the journal, so something has to carry it to `doctor`.

## 8. Compromised release artifact

**What it can do.** Replace the tarball behind a release or the formula in the tap. Ship a binary built from other source or from a poisoned toolchain. Slip a malicious crate or GitHub Action version into the build. Steal the tap token. Withdraw a good release and publish a bad one. A compromised binary runs as the user and can do everything the user can.

**At risk.** Everything on the machine that the user can reach. The controls make a bad artifact detectable and replaceable. They cannot contain one that already runs (7.4).

Spec 7.1 points this threat at section 8.3. The release controls are in 8.4, and 8.3 (versioned formats) covers mixed versions during a rollback, which sections 4 and 10 list under S18.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| Builds run on a named runner image (`macos-15`, never `macos-latest`). The Rust toolchain is pinned to 1.99.0, and the runner image, `rustc -Vv`, cargo, Xcode, SDK and flags are recorded and compared with the expected record | 8.4 | S20 | implemented | `rust-toolchain.toml`, `release/toolchain.json`, `scripts/toolchain.py`, `.github/workflows/release-dry-run.yml` |
| Two independent builds are compared byte for byte, the tarball is deterministic (sorted entries, fixed mtime, `gzip -n`), and the binary and the tarball get a build provenance attestation. In the dry run both builds had the same SHA-256 and `gh attestation verify` accepted the result. The release workflow also builds a tarball from each binary and compares the two | 8.4 | S20 | implemented | `.github/workflows/release-dry-run.yml`, `.github/workflows/release.yml`, `scripts/compare_builds.py`, `scripts/package.py`, `scripts/test_package.py`, `docs/m0/report.md` |
| Every action is pinned to a full commit SHA, workflow permissions default to `contents: read`, only the package job gets `id-token` and `attestations` write access, and only the release job gets `contents` write access, which it uses to create a draft release. The release workflow has no schedule and starts only on a pushed `vMAJOR.MINOR.PATCH` tag | 8.4 | S20 | implemented | `.github/workflows/ci.yml`, `.github/workflows/release-dry-run.yml`, `.github/workflows/release.yml`, `scripts/test_release_workflow.py` |
| The Homebrew formula template pins the tarball URL, version and SHA-256 and installs one binary. A local tap install of the dry-run tarball printed the expected version | 8.4 | S20 | implemented | `scripts/formula.py`, `scripts/test_formula.py`, `scripts/brew_smoke.py`, `docs/m0/report.md` |
| Dependencies are locked (`cargo build --locked`), and every pull request runs `cargo deny` (licences MIT, Apache-2.0 and Unicode-3.0, the crates.io registry only, no git sources, no yanked or wildcard versions) and `cargo audit`. Each direct dependency has a line in the allowlist | 7.1, 8.4 | none | implemented | `deny.toml`, `docs/dependencies.md`, `.github/workflows/ci.yml` |
| A release only comes from a pushed `vMAJOR.MINOR.PATCH` tag whose version equals the crate version, whose commit `main` contains, and whose commit has passing `linux` and `macos` check runs from GitHub Actions. A commit with no check run is refused. The builds select the recorded Xcode and SDK and fail when any recorded toolchain value differs, including the `Cargo.lock` hash | 8.4 | S20 | implemented | `.github/workflows/release.yml`, `scripts/check_release.py`, `scripts/test_check_release.py`, `scripts/test_release_workflow.py`, `scripts/toolchain.py`, `scripts/test_toolchain.py` |
| `cargo audit` runs in the release workflow before any release is created, the tap update keeps a versioned formula for the previous minor release, and SECURITY.md and the release document carry the rollback note | 8.4 | S20 | implemented | `.github/workflows/release.yml`, `scripts/tap_update.py`, `scripts/test_tap_update.py`, `SECURITY.md`, `docs/release.md`, `scripts/test_release_docs.py` |
| A CycloneDX SBOM ships with each release, its checksum is listed in `SHA256SUMS`, its provenance is attested, and the tap pull request uses a secret from the `release` environment | 8.4 | S20 | implemented | `.github/workflows/release.yml`, `scripts/test_release_workflow.py`, `docs/release.md` |
| npm ships the same native binary in a deterministic archive without install scripts. Publication requires a public stable release, verified checksums and a tag-bound `release.yml` attestation. OIDC is limited to the `npm` environment, and retries refuse an existing version with different bytes | 8.4 | S20 | implemented | `.github/workflows/npm-publish.yml`, `scripts/npm_package.py`, `scripts/npm_publish.py`, `scripts/npm_smoke.py`, `scripts/test_npm.py` |
| Rollback: `apply = false` stops all signalling while a bad release is withdrawn | 6.5, 8.4 | S21 | implemented | `crates/agentdust-core/tests/config.rs`, `crates/agentdust-core/tests/apply_server.rs`, `docs/release.md` |

**Residual risk.**

- Reproducibility catches a build that differs between two runs. It does not catch a bad input that both builds share: the runner image, Xcode, a crate or an action at a malicious commit gives two identical bad binaries. The attestation names the workflow that built a file and says nothing about the intent of its source.
- A compromised binary acts as the user, and nothing inside AgentDust can stop it (7.4).
- Homebrew does not verify the attestation, so a person has to run `gh attestation verify`. The formula's digest protects the download, and whoever can change the tap can change the URL and the digest together.
- `scripts/toolchain.py check` compares every recorded value with `release/toolchain.json`. The release passes `--include-lock`, so a `Cargo.lock` that differs from the record fails the build. The dry run compares the lock hash only between its two builds, because the dry run is how a new record is taken.
- The release gate trusts check runs that the GitHub Actions app created on the tag commit. A commit that weakens `ci.yml` and passes the weakened jobs satisfies it, so review of changes under `.github/workflows` is the control. The gate checks that `main` contains the commit and does not read branch protection, which is a repository setting.
- A person publishes the draft release and merges the tap pull request. Nothing in the workflow checks that they verified the draft first.
- `cargo audit` knows published advisories only. `cargo deny` does not cover the fuzz crate, which is outside the workspace and is not shipped.
- The binary is meant to contain no network code (7.2). A `cargo tree` of the `agentdust` package shows tokio built without its `net` feature and no `mio`, `hyper` or `reqwest` crate, and no automated check enforces it.

## 9. Unsupported filesystem

**What it can do.** The volume under the data directory decides what `write(2)`, `rename` and `fstat` mean. A home directory on a file server (an NFS or SMB network home), an `AGENTDUST_DATA_DIR` that points at one, a mount placed over the directory, an external disk formatted HFS+ or exFAT, and the automounted `/System/Volumes/Data/home` (which reports `autofs`) all put the journal on a volume AgentDust did not test. ADR-1 treats a network volume as breaking the premise of an atomic append, SMB as unproven and a local volume that is not APFS as untested.

**At risk.** Journal integrity (torn, interleaved or lost lines), the install secret, hook latency, and the evidence itself: on a refused volume there is none.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| The writer asks `statfs` about the journal directory, or about its nearest existing ancestor when the directory is missing, and writes only when `f_fstypename` is `apfs` and `MNT_LOCAL` is set. On any other volume there is no record, no file and no directory, and the error names the volume. A probe that fails stops the append | 3.2, 7.3 | none | implemented | `crates/agentdust-core/src/journal/volume.rs`, `crates/agentdust-core/tests/journal_append_volume.rs`, `crates/agentdust-core/tests/journal_volume.rs` |
| The install secret, rotation and retention follow the same gate. On a refused volume the secret is neither read nor created, and a maintenance run creates nothing | 7.2, 7.3 | none | implemented | `crates/agentdust-core/tests/secret_volume.rs`, `crates/agentdust-core/tests/journal_rotate.rs`, `crates/agentdust-core/tests/journal_retain.rs` |
| A hook whose data directory is on a refused volume exits 0, prints nothing and creates nothing | 4.4 | S13 | implemented | `crates/agentdust/tests/hook.rs` |
| The gate is injectable. The decision runs on injected facts for apfs local, apfs not local, hfs, devfs, nfs, smbfs and autofs and needs no real volume, so it runs on any platform, and `SystemVolume` answers "unknown, not local, not supported" off macOS | 9.2 | none | implemented | `crates/agentdust-core/tests/journal_volume.rs`, `crates/agentdust-core/tests/secret_volume.rs` |
| A reader does not refuse an unsupported volume, because a read tears nothing. The report carries the file system name, whether it is local and whether it is supported, and `Journal::status()` returns the same facts | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_read.rs` |
| `agentdust status` prints filesystem type, locality and support, and hook health records `unsupported_filesystem` with the name | 4.4 | none | implemented | `crates/agentdust/src/status.rs`, `crates/agentdust-core/tests/journal_volume.rs`, `crates/agentdust-core/tests/journal_append_volume.rs` |
| A refused volume counts as missing provenance: owned classes become unavailable and nothing becomes more certain | 4.4, 6.5 | S19 | implemented | `crates/agentdust-core/tests/doctor_sessions.rs`, `crates/agentdust-core/tests/classifier_provenance.rs` |

**Residual risk.**

- A person whose home directory is on a network volume gets no journal at all, so AgentDust never learns which processes a session owns. Refusing is the safe default. It is also a loss of the feature for that person until the data directory moves to a local APFS volume, and section 12 says what missing evidence does to the classes.
- The refusal of NFS and SMB rests on the `MNT_LOCAL` flag and the file system name. No network volume was available, so no refusal was observed on one. On the benchmark machine `$TMPDIR`, `$HOME` and `/private/tmp` reported apfs and local, `/dev` reported devfs (local, refused) and `/System/Volumes/Data/home` reported autofs (not local, refused).
- A local volume that is not APFS (HFS+, exFAT, FAT) is refused even though it might work. It was not tested.
- The check is a point in time. `statfs` runs before the open of each append, so a mount that appears over the directory between the check and the open is not seen by that append. Each later append asks again. The gate judges the file system the kernel reports for the path and does not look behind a local volume.
- The gate has no time limit. `statfs` and the open have none of their own, and `realpath` of a working directory on a stalled network mount can hold the hook until the host gives up (`docs/m1/install-secret.md`). The hook has no bound of its own. The host's 10 second timeout (4.4) is the only one.
- The gate costs 0.0015 to 0.0038 ms at p50, measured on a local volume (`docs/m1/journal-schema.md`).

## 10. Journal poisoning and short writes

**What it can do.** Two sources. An accident ends a write early (a full disk, an I/O error, a signal, a killed process) and leaves part of a frame. A planted or damaged journal holds bytes that are not records: garbage, text that is not UTF-8, a tail without its end, a line of a megabyte, a line of a newer schema version, a record of an unknown kind, records that are valid and false, a symlink, FIFO or hard link where a generation belongs, or a directory where `journal.compact.tmp` goes. A hostile process or model can also choose the text of a value that reaches a record, such as a session id, and put the frame delimiters in it.

**At risk.** The record after a bad one, the reader's memory and time, the class a process is given, retention's ability to run, and the privacy of the file.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| A record is one frame: the byte 0x1E, compact JSON, the byte 0x0A, in one `write(2)`. The decoder splits on both bytes, so a write cut at any length costs the cut record and no other. A tail without its 0x0A is torn and never parsed, even when it is complete JSON | 3.2, 4.4 | none | implemented | `crates/agentdust-core/tests/journal_frame.rs`, `crates/agentdust-core/tests/journal_decode.rs`, `crates/agentdust-core/tests/journal_decode_props.rs`, `crates/agentdust-core/tests/journal_truncation_props.rs`, `crates/agentdust-core/tests/journal_corpus.rs`, `fuzz/fuzz_targets/journal_decode.rs` |
| Write faults are injected into the real append: `EINTR` before any transfer is restarted, and a zero byte write, a write of 1 byte and of N minus 1 bytes, `ENOSPC` and `EIO` each leave record N+1 readable. A short write is never completed by a second call, because that call would not be atomic with other appenders. A frame over 65,536 bytes is refused before anything is opened | 3.2, 4.4 | none | implemented | `crates/agentdust-core/tests/journal_append_faults.rs`, `crates/agentdust-core/tests/journal_append.rs`, `docs/m1/journal-schema.md` |
| A value cannot forge a frame boundary, because the encoder writes 0x1E and 0x0A as escapes. Fields are validated by type and limit: identifiers at most 256 bytes while the hook reads them, `exe_base` at most 64 bytes without controls, `cwd_key` lowercase hex | 3.2, 4.4 | none | implemented | `crates/agentdust-core/tests/journal_frame.rs`, `crates/agentdust-core/tests/journal_record.rs`, `crates/agentdust-agents/tests/claude_bounds.rs` |
| The decoder holds at most 65,536 bytes of one segment and counts malformed, torn, unknown-kind and newer-version lines in separate counters. The decoder and the reader are fuzzed under a counting allocator that fails when memory is not bounded by the input and by what the call kept | 9.2 | none | implemented | `crates/agentdust-core/tests/journal_decode.rs`, `fuzz/fuzz_targets/journal_decode.rs`, `fuzz/fuzz_targets/journal_read.rs`, `fuzz/src/meter.rs`, `fuzz/src/bounds.rs`, `docs/m1/fuzzing.md` |
| A line of a newer schema version is never interpreted, even when it has the shape of a supported version and even when it is longer than 65,536 bytes, which is recognised by its leading `{"v":`. It is counted and raises `unsupported_version`, and retention never rewrites, copies or deletes a generation that holds one. Version 2 writes also fence version 1 readers before subagent data can be silently skipped | 3.2, 8.3 | S18 | implemented | `crates/agentdust-core/tests/journal_read.rs`, `crates/agentdust-core/tests/journal_retain_recovery.rs`, `crates/agentdust-core/tests/journal_corpus.rs`, `crates/agentdust-core/tests/journal_frame.rs`, `fuzz/fuzz_targets/journal_decode.rs` |
| `doctor` reports a journal of an unknown version and makes owned classes unavailable, and `apply` refuses owned-ended items | 6.5 | S18 | implemented | `crates/agentdust-core/tests/doctor_sessions.rs`, `crates/agentdust-core/tests/apply_server.rs`, `crates/agentdust/tests/doctor.rs` |
| A line of an unknown kind is skipped by readers, counted apart and kept byte for byte by retention | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_decode.rs`, `crates/agentdust-core/tests/journal_retain.rs` |
| Retention copies a generation byte for byte (mode 0600, synced) before a rewrite or a delete drops a line that does not parse. A torn frame is dropped without a copy. Damage alone never causes a rewrite, and a generation that cannot be read is never deleted | 3.2, 6.5 | none | implemented | `crates/agentdust-core/tests/journal_retain_recovery.rs`, `crates/agentdust-core/tests/journal_retain.rs`, `crates/agentdust-core/tests/journal_retain_adr.rs` |
| Readers order records by boot rank, `mono_ts`, `wall_ts` and then every field of the record, and collapse records that are equal in every field. The order does not depend on which file holds a record, and a consumer pairs events by `tool_use_id` and never by position | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_read.rs` |

**Residual risk.**

- **Nothing authenticates the writer.** A program that runs as the user can append well-formed records that say anything. Spec 7.4 puts that out of scope. A forged record can at most change the class a process is given, and a signal still needs fresh identity and ownership checks plus either manual typed approval or satisfaction of the enabled automatic directory policy.
- A line of a newer schema version is a switch that any program of the user can throw. One planted line makes owned classes unavailable (fail closed) and keeps its generation from ever being shrunk. By design the effect is lost cleanup and not a wrong signal. Every future schema has to keep `v` as the first key of its compact JSON, because a line over 65,536 bytes whose `v` is not first reads as malformed and not as newer.
- A short write loses the record being written. The hook drops it and does not retry. The cut bytes stay in the file as one torn frame: in the active file until rotation moves it, and in a generation until a retention run rewrites that generation for another reason.
- A read that runs while an append is in flight can see a cut tail. It is reported as a truncated last line and the record can be missing from that read.
- A planted symlink, FIFO, directory or hard link in the place of a generation stops every rotation and retention run until a person removes it. Readers skip the file and count it.
- Retention keeps unknown-kind lines for ever, because it cannot tell their session or age, and it never deletes the `journal.jsonl.corrupt-<n>` copies. A program that keeps writing such lines grows the directory.
- Nothing calls `prune`, so a flood of valid records grows the journal until a caller exists. The size limit counts the frames of records that parse and applies only when `prune` runs.
- `sample` records are not built. The cap of 65,536 bytes was measured with padded records of 150 and 4,000 bytes. A real `procs` list can pass it, and a record over the cap is dropped. A process tree large enough to do that could hide itself from a sample, so the M2 sampler needs a byte budget with `procs_total` and `procs_recorded` fields (ADR-1).
- Order is for presentation. A forged or skewed `mono_ts` or `wall_ts` reorders the presentation, and the order across boots is weak because boots are ranked by wall time, which can go backwards.
- The fuzz runs are 16 second smoke runs per target. The 10 minute pre-release run of spec 9.2 is not part of M1.

## 11. Stale writers and rotation

**What it can do.** A hook is a short-lived process that can stop at any instruction: a SIGSTOP, a suspended laptop, a slow disk, a host that is slow to schedule it. It can hold an open file for longer than any fixed delay while rotation renames the active file, retention replaces or deletes closed generations and a reader lists the directory. Two appenders can race the creation of the active file. A maintenance run can be killed halfway, or hold its lock for ever.

**At risk.** An acknowledged record, a reader's coherent view of the files, and the state that retention leaves behind.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| After each write the appender compares `fstat` of its descriptor with `lstat` of `journal.jsonl`. When the path names another file, or none, it writes the frame again, up to 3 attempts in all. There is no lock and no clock | 3.2, 4.4 | none | implemented | `crates/agentdust-core/src/journal/append.rs`, `crates/agentdust-core/tests/journal_append_recheck.rs`, `crates/agentdust-core/tests/journal_rotate_interleave.rs`, `crates/agentdust-core/tests/journal_retain_interleave.rs` |
| No step of rotation or retention depends on elapsed time. The tests pause a writer after its open, before its write and after its write at barriers, run a rotation, a compaction and a deletion, and let it go: the acknowledged record is present exactly once, however many rotations the writer sleeps through and whether retention replaced or deleted the generation it held | 9.2 | none | implemented | `crates/agentdust-core/tests/journal_rotate_interleave.rs`, `crates/agentdust-core/tests/journal_retain_interleave.rs`, `crates/agentdust-core/tests/journal_append_recheck.rs` |
| A reader takes no lock. It opens `journal.jsonl` first, then lists the generations, opens each, skips one that vanished, reads a file it already holds once by inode and collapses exact duplicates, so it holds every record acknowledged before it began that no retention rule dropped. Tests pause a reader after its open and after its listing | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_snapshot.rs`, `crates/agentdust-core/tests/journal_read.rs`, `crates/agentdust-core/tests/journal_rotate_interleave.rs`, `crates/agentdust-core/tests/journal_retain_interleave.rs` |
| Rotation renames the active file and never rewrites it. Retention visits only closed generations, replaces one under its own name or deletes it, and never moves a record. One algorithm is stated in ADR-1 with an outcome table for 15 kinds of generation, and a test runs every row against the code | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_rotate.rs`, `crates/agentdust-core/tests/journal_retain.rs`, `crates/agentdust-core/tests/journal_retain_adr.rs` |
| One maintenance run at a time. `journal.maint` is taken with a non-blocking exclusive `flock` by rotation and retention only, and a second run gets `Busy` and changes nothing. Appenders and readers never open it, so a stopped rotator delays no hook and a stopped hook delays no rotation | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_rotate.rs`, `crates/agentdust-core/tests/journal_retain_interleave.rs` |
| A hook never rotates, compacts or deletes, takes no maintenance lock, and the binary has no maintenance command | 4.4 | none | implemented | `crates/agentdust/tests/hook_never_prunes.rs` |
| The creation of the active file can race a rotation. The file is created exclusively, reopened and never written through the creating descriptor, and the open repeats up to 3 rounds before it fails with nothing written | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_append_create.rs` |
| A crash during maintenance is safe to repeat. A replacement is written to `journal.compact.tmp` (exclusive, 0600), synced and renamed over the generation. A leftover temporary file is removed by the next run, and a symlink in its place is removed and not followed. The old generation is intact until the rename | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_retain_recovery.rs`, `crates/agentdust-core/tests/journal_retain.rs` |
| Stress runs with fixed work and a forced final retention run: three writers, a straggler that writes into a rotated generation, a rotator and a reader, as threads and as processes. Every acknowledged record is present once, and no assertion has a time bound or depends on throughput | 9.2 | none | implemented | `crates/agentdust-core/tests/journal_retain_stress.rs`, `crates/agentdust-core/tests/journal_append_rotation.rs`, `crates/agentdust-testkit/tests/journal_stress.rs`, `crates/agentdust-testkit/src/bin/fixture-journal.rs` |

**Residual risk.**

- An append whose active file is replaced under each of its 3 attempts fails with `Stale`. The hook drops the record and counts it, and the copies written into rotated files can still be read. It takes three rotations inside one append, and no benchmark run produced one.
- A record can exist twice between a rotation and the next retention run: the first copy in a generation and the second in the new active file. Readers collapse records that are equal in every field, and retention drops an exact duplicate of a record it kept earlier in the same run. Anything that reads the files directly sees both. In the benchmark's stress runs 1.1% of the appends of the recheck candidate were written twice.
- A read does not see every record acknowledged while it runs.
- The atomic append is a reliance on POSIX append semantics for local regular files. It is stress tested on APFS and is not an APFS guarantee. The benchmark ran 3 and 16 writer processes against its own copy of the append, and the repository tests run 16 threads, 16 hook processes and 3 writer processes against the shipped one. No torn or interleaved line appeared. The runs were on one machine and one macOS version, with writers that do not pause between appends, and no write was killed in the middle.
- A same-user program that holds `journal.maint` for ever denies rotation and retention, and one that stops a hook stops only that hook. Neither changes the journal. Spec 7.4 puts both out of scope.
- Retention reads the whole journal into memory, about 20 MB of frames and the same again for their records at the default limit. A journal of that size was not measured.
- Nothing calls `rotate`, `retain` or `prune`. The CLI and the server set the cadence (M3), and each run that finds bytes in the active file adds one generation.
- The interleaving and stress tests were run on macOS. The platform independent ones compile for `x86_64-unknown-linux-gnu` and were not run there.

## 12. Durability limits

**What it can do.** An operating system crash, a kernel panic, a power failure, a forced shutdown or a full disk can stop the machine or a write at any point. No attacker is needed.

**At risk.** The records written just before the stop, `install.secret`, and the generation that a retention run is replacing.

| Control | Spec | Requirement | Status | Tests |
| --- | --- | --- | --- | --- |
| An append is acknowledged when the kernel holds the bytes. Appends are not synced, so a record survives the death of the hook process and not an OS crash or a power failure. Per record the cost is 0.002 ms at p50 with no sync, 0.044 ms with `fsync(2)` and 4.0 ms with `File::sync_all`, against a hook budget of p50 10 ms and a tool call that writes at least two records | 3.2, 4.4 | none | implemented | `crates/agentdust-bench/src/durability.rs`, `crates/agentdust-bench/tests/durability.rs`, `crates/agentdust/tests/hook_latency.rs`, `docs/m1/adr-journal-format.md` |
| Rotation and retention sync. The replacement file is synced before it is renamed over a generation and the directory is synced at the end of the run, so a crash before the rename leaves the old generation intact. A crash after the copy leaves a copy, and the next run makes another | 3.2 | none | implemented | `crates/agentdust-core/tests/journal_retain_recovery.rs`, `docs/m1/journal-retention.md` |
| `install.secret` is written to a synced temporary file and installed by an exclusive rename, so a crash never leaves a short secret. The directory is not synced afterwards, so a crash can lose the secret, and the next hook draws a new one | 7.2, 6.5 | none | implemented | `crates/agentdust-core/tests/secret.rs`, `crates/agentdust-core/tests/secret_install.rs` |
| Lost evidence only lowers confidence. A journal cut at its last records, a lost secret and a missing record are run through the classifier and compared with full provenance | 4.4, 6.5 | S19 | implemented | `crates/agentdust-core/tests/classifier_provenance.rs`, `crates/agentdust-testkit/tests/apply_end_to_end.rs`, `crates/agentdust-core/tests/journal_retention.rs` |

**Residual risk.**

- **An acknowledged record can be lost on an OS crash or a power failure, and nothing promises otherwise.** The last records before the stop are the likeliest to go. They describe the processes that were running when the machine stopped.
- When ownership evidence is lost, an item leaves owned-live and can qualify as suspect when it meets the suspect conditions. A background job of a live agent that was reparented to launchd is one such case. Suspect items require a per-item code, and apply still checks identity immediately before SIGTERM. Classifier tests cover missing records and degraded provenance.
- A secret lost this way cannot be recovered. Every `cwd_key` already in the journal stops matching the new secret, so the working directory evidence of earlier records is gone.
- No power was pulled and no write was killed in a test. Crash states were planted as files, and short writes and failed writes were injected through the `FrameWriter` trait. The kernel did not produce them.
- Neither `fsync(2)` nor `File::sync_all` per append is adopted. ADR-1 says `fsync(2)` could be added later without changing the format.
- A crash during retention can leave `journal.compact.tmp` and `journal.jsonl.corrupt-<n>` copies until the next run. Retention never deletes the copies.

## Coverage of spec 7.1

| Threat in spec 7.1 | Section of this page |
| --- | --- |
| A model, injected or not, calls `apply` | 1 |
| The host auto-accepts elicitation | 1 and 3 |
| Malicious process metadata | 2 |
| Secrets in commands, paths or environments | 1 and 2 |
| PID reuse | 5 |
| Concurrent apply | 6 |
| Wrong target | 1 and 5 |
| A hook slows the agent | 1, 9 and 11 |
| Compromised dependency or artifact | 8 |

The buggy client (section 3), same-user tampering (section 4), stale plans (section 7) and the four storage sections (9 to 12) are not rows of 7.1. Their controls come from spec 3.2, 4.4, 6.1 to 6.5, 7.2, 7.3 and 8.2.

## What this model does not cover

From spec 7.4:

- An agent with free shell access. It can signal processes without this tool.
- A malicious MCP client, including one that answers the approval form without the person.
- A same-user or root attacker, including one that forges journal records.

The host application stays in the trusted computing base (7.1).

## Automatic cleanup permission and restart

Opt-in automatic cleanup uses the existing ownership classifier and executor. Only owned-ended helpers attributed exclusively to one Claude or Codex session, including its recorded subagents, can qualify. Every owner must be gone, every recorded session-start directory must be enabled, and the target identity must not be kept. Fresh class, ownership and identity checks still occur inside the per-process critical section. See [automatic cleanup](automatic-cleanup.md).

Interactive CLI policy writes reject non-terminal and known-agent-originated callers. `auto enable --yes` intentionally permits unattended enablement, including an AI shell acting within a user-authorized scope. AgentDust does not authenticate the user authorization or protect against an injected agent using that flag. The same-UID caller can broaden automatic scope through this supported interface, but cannot use the flag to approve uncertain processes or remove keeps. Classifier proof, fresh identity checks, keeps and attempt receipts still apply. MCP exposes read-only automatic status and no policy mutation. These interfaces do not isolate programs sharing a user ID.

Policy mutation and execution share a private lock. Pause cannot retract a signal already sent; it waits for the bounded in-flight action and blocks subsequent actions. Keeps bind to boot, PID, start time and UID and also block manual apply. Missing, malformed or unsafe automatic state cannot authorize a signal.

An automatic attempt receipt is synced before signaling. A restart cannot automatically signal that exact identity again. A crash between receipt creation and signaling can leave a helper requiring manual handling. Receipts remain while identity liveness is unknown or alive and are pruned only when the exact identity is proven gone. The receipt and keep files contain kernel identity fields, including a boot ID, but no command, environment or path.

The worker polls known owner identities and journal metadata, not a full machine inventory every second. Work and storage have explicit limits. A pending or unavailable result means automatic cleanup is incomplete and manual inspection remains necessary. Automatic audit uses an `auto-` plan-ID prefix without changing manual record keys.

Policy and executor checks are in `crates/agentdust-core/tests/automatic.rs`. CLI and MCP checks are in `crates/agentdust/tests/automatic.rs`. The isolated owner-exit and restart scenarios are in `crates/agentdust/tests/automatic_worker.rs`; their worker uses only the test journal, fixture session marker and private test policy. No real user project is enabled by those checks.

Codex native hooks and `CODEX_SESSION_ID` add attribution evidence with a separate HMAC domain. The marker is public metadata, not an authentication boundary against a same-UID actor. A dual Claude/Codex marker, missing ancestry or a host change without a recorded session start cannot authorize cleanup. SessionEnd and SubagentStop do not replace exact host death. Desktop helpers stay protected while any recorded shared host remains alive. Setup preserves user hooks, verifies native MCP changes and does not bypass Codex hook trust. Tests are in `crates/agentdust-agents/tests/codex_setup.rs`, `crates/agentdust-core/tests/inventory.rs`, `crates/agentdust-core/tests/session.rs` and `crates/agentdust/tests/automatic_worker.rs`.

Not modelled: file systems other than local APFS beyond the refusal in section 9, immediate per-chat Codex cleanup while a shared server remains live, the Cursor adapter (M5), and agent storage outside the M7 roots. The M7 metadata scanner is covered below.

## 16. Read-only disk inventory (M7)

A planted directory, symlink, mount, FIFO or changing directory can redirect or delay an inventory. Names and file contents may hold credentials or project paths.

The scanner reads directory entries and metadata only. Each root path component is opened with `O_DIRECTORY | O_NOFOLLOW`; descendants use `fstatat(AT_SYMLINK_NOFOLLOW)` and `openat(O_DIRECTORY | O_NOFOLLOW)`. The opened directory's device and inode must match its metadata before traversal. Entries on another filesystem, symlinks and special files are excluded. No regular file is opened.

The CLI and MCP return fixed root labels, categories, counters and statuses. Raw entry names and filesystem errors are not returned. The scanner writes no journal, report or approval state and performs no deletion. Root overrides come from the launching user's environment, never a model argument.

The scan checks its budget between directory entries and before each root: 100,000 entries, 64 directory levels or 10 seconds, with a partial result. One MCP disk scan runs at a time. A filesystem call can block beyond the budget. Concurrent modifications can change totals; the report is not a snapshot or a reclaimable-space calculation. See [M7 scope and limits](m7/disk.md).

## TCP listener recovery

The TCP listener inventory is an additional constraint, not ownership or authorization evidence. The full process classifier resolves every owner before listener filtering. Before each automatic attempt, the executor repeats that classification and the exact listener identity check. The existing enabled-project policy, keeps, apply switch, owner revalidation, per-process lock and durable receipt control signaling.

The macOS adapter invokes `/usr/sbin/lsof` with fixed TCP LISTEN arguments, numeric addresses and ports, and only PID/socket-name fields. It clears the child environment, bounds each output stream to 64 KiB and imposes a five-second capture deadline. Warnings, malformed output, identity churn, more than 16 listeners or failed scans cannot authorize cleanup. Commands and environments are not collected. Native identities before and after the scan must match.

The final scan reports current-user-visible listeners. An empty result does not prove that another user has no listener or that a subsequent bind will succeed. TCP socket state can change after any scan; this feature is not an atomic port reservation. UDP and remote hosts are outside its scope. Uncertain actionable listeners use the existing human approval flow, with the requested exact item selected from a fresh plan.
