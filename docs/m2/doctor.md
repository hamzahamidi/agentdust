# Inventory, classifier and doctor

This page describes how `agentdust doctor` finds the processes of a user, gives each one a class and prints the result. Spec sections 3.4, 3.5, 5 and 7.2 hold the design, and S1, S15 and S19 of section 9.1 are the safety requirements it serves. The last sections list where the code differs from the spec and what is not built. Nothing here signals a process: the command reads and prints, and no code path of it can call `kill`.

## The command

| Command | Output |
| --- | --- |
| `agentdust doctor` | The text report of the section below, with the redacted command and the directory of each finding |
| `agentdust doctor --json` | One JSON object on standard output and nothing else. It holds typed fields only |
| `agentdust doctor` with any other argument | The usage line on standard error and exit status 2 |

The command runs four steps and exits 0 when they finish, also when owned classes are unavailable:

1. Load the install secret without creating it, and take a snapshot of the processes (below). The snapshot takes 2 seconds because of the idle sample, so the command ran for 2.06 s on the development Mac.
2. Read the journal and derive the session scopes of [provenance](provenance.md). The liveness of each agent is probed after the snapshot, so it is as recent as the classification.
3. Classify every process.
4. Print. The working directory of each listed finding is read at this point, and so is its command in the text report.

It exits 1 with a message on standard error when the data directory path cannot be built (`HOME` is not set) or the process list cannot be read. It creates nothing: a missing data directory stays missing, the secret is never installed, and the journal is never rotated or repaired. A test compares every file of the data directory byte for byte and by mode before and after.

## The snapshot

`agentdust_core::inventory::take` scans once, waits `IDLE_SAMPLE_GAP` (2 seconds), reads the cumulative CPU time of each process again, asks for the launchctl list, and reads the clock last. The classifier is a pure function of this value, so a test can place a process at any age and idleness.

The scan (`DarwinSource`) lists PIDs with `proc_listallpids` and reads each process through libproc. Another user's processes cannot be read without privilege and are not in the scan: on the development Mac a scan listed 1,410 PIDs and read 1,181, and the rest belong to root. A process whose reads fail is left out, because a finding without an identity cannot be signalled.

| Field | Source | Notes |
| --- | --- | --- |
| Kernel identity | `PROC_PIDTBSDINFO` and the boot session UUID | PID, start time in microseconds, user, boot |
| Parent and process group | `PROC_PIDTBSDINFO` | |
| Executable path | `proc_pidpath` | `None` when unreadable. A process without it is never actionable |
| Cumulative CPU time | `PROC_PIDTASKINFO`, user plus system | Converted to nanoseconds with `hw.tbfrequency`, because the kernel reports it in timer ticks on Apple silicon |
| Session tag | `KERN_PROCARGS2`, the variable `AGENTDUST_SESSION` only | Hashed with the install secret at once, see below |
| `agent_script` | the argument part of `KERN_PROCARGS2`, for `node` only | Whether the script path names `claude` |
| Working directory | `PROC_PIDVNODEPATHINFO` | Not part of the snapshot. Read at display time |

Every process is read between two reads of its kernel identity. A process whose identity changed between them is dropped, so the attributes of a recycled PID are never mixed with those of its predecessor.

The tag is one of four states:

| State | When |
| --- | --- |
| `Absent` | The variable is not in the environment, or the process hides its environment |
| `Keyed` | The variable is there, and its value was hashed with the install secret into a `SessionTagKey`. The raw value is dropped and appears in no `Debug` output |
| `Unkeyed` | The variable is there and there is no install secret to hash it with |
| `Unreadable` | The buffer cannot be parsed, the read failed, or the process belongs to another user |

A process that hides its environment gives `Absent`. A scan of the 1,181 readable processes on the development Mac gave `Absent` for every one of them, since none carried a tag, and `Unreadable` for none, including those under `/System` and `/usr`.

The idle test needs two reads: a process is idle when both exist and are equal. A read that fails, or a process that is gone or replaced at the second read, is not idle. A process whose first read failed is not read again.

`launchctl list` is run as `/bin/launchctl list` with a 10 second limit, and its output is parsed strictly: the header `PID`, `Status`, `Label` must come first, every row has three tab separated columns, a PID is `-` or a positive number and a status is `-` or a number. Anything else is an error. A failed or unparsable list is `Launchd::Unavailable`, which is not the same as an empty list.

## Classes

`agentdust_core::classifier::classify` takes the snapshot, the session scopes (or "unavailable") and a policy, and returns one finding per process in snapshot order. The rules run from top to bottom and the first match wins.

| Class | Condition | Evidence kinds |
| --- | --- | --- |
| managed | Any of the following holds. All that hold are listed, in this order | |
| | PID 1 or below | `managed.deny.init` |
| | The user is not the user of the tool | `managed.deny.other_user` |
| | It is the tool itself | `managed.deny.self` |
| | It is an ancestor of the tool | `managed.deny.ancestor` |
| | A live agent: the executable is named `claude` or sits at `.../claude/versions/<version>`, a `node` runs a script that names `claude`, or its identity is that of a session scope | `managed.deny.agent` |
| | The executable path is under `/System`, `/usr`, `/bin`, `/sbin`, `/Library/Apple` or `/Applications`, on whole path components | `managed.deny.system_path` |
| | Its PID is in the launchctl list | `managed.launchd` |
| | Else, its parent is in the list | `managed.launchd_child` |
| owned-live | The tag matches a usable session and the agent of at least one is not known to be gone | `owned.tag`, then `owned.agent_alive` or `owned.agent_unverified` |
| owned-ended | The tag matches usable sessions only, every agent is known to be gone, the executable path is readable and the launchctl list was read | `owned.tag`, `owned.agent_gone` |
| likely-owned | Never produced | |
| suspect | No tag, the parent is PID 1, the age is at least `SUSPECT_MIN_AGE`, the process is idle, the executable path is readable and the launchctl list was read | `suspect.parent_launchd`, `suspect.same_user`, `suspect.age`, `suspect.idle` |
| unknown | Everything else | See below |

How a tag is matched to the scopes:

| The tag matches | Result |
| --- | --- |
| No scope, or any scope while provenance is unavailable | unknown, `tag.unmatched` or `tag.unverifiable` |
| Only scopes without an agent identity (degraded) | unknown, `tag.degraded_session` |
| At least one exact owner is alive or unverified | owned-live, with `tag.degraded_session` added when an ambiguous scope also matches |
| Exact owners are gone, and a degraded or ambiguous scope also matches | unknown, with `owned.tag`, `owned.agent_gone` and `tag.degraded_session` |
| Every exact owner from every matching scope is proven gone, with no degraded or ambiguous scope | owned-ended |

"Gone" requires fresh liveness evidence for the primary Claude process and every additional exact process identity attributed to that scope. A `session_end` or `SubagentStop` record alone does not make an owner gone, and an unreadable probe does not either, so both leave a process owned-live. An ambiguous owner set is not actionable.

A process with a tag in any state but `Absent` is never a suspect. The tag is the only ownership evidence in release 0.1, so a tag that no usable session explains means the ownership evidence was lost, and a process whose ownership is in doubt is not offered as a leftover.

When a finding would reach owned-ended or suspect and the executable path is unreadable or the launchctl list is unavailable, it stays unknown and its evidence ends with `identity.path_unreadable` or `launchd.unavailable`, so the report says what held it back. Another user's processes, which cannot be read, are not in the scan at all.

The age is the snapshot time minus the start time, and zero when the start is later than the snapshot.

### Thresholds

These values are provisional until the fixture corpus bounds them from both sides, which is tracked in GitHub issue 17. They are named constants, and the policy carries the age so a test can change it.

| Constant | Value | Where |
| --- | --- | --- |
| `SUSPECT_MIN_AGE` | 30 minutes, inclusive | `agentdust_core::classifier` |
| `IDLE_SAMPLE_GAP` | 2 seconds | `agentdust_core::inventory` |
| Ancestor walk of the tool | 64 parents | `agentdust_core::classifier` |
| `SYSTEM_PREFIXES` | six paths, see above | `agentdust_core::classifier` |

The corpus bounds the age from one side only. Every process that must stay out of suspect fails a condition that no threshold changes (a live owner, a live parent that is not launchd, a deny list entry), so no fixture marks the lower edge. A test classifies seven fixtures at ages of 0, 10, 20, 29, 31, 90 and 600 minutes and requires that a process is a suspect exactly when it is `true_detached` and the age reaches 30 minutes. A busy process cannot be started yet, so the idleness boundary is pinned by table tests on scripted samples and not by a live process.

## What the model and the person see

| Type | Seen by | Content |
| --- | --- | --- |
| `RawIdentity` and `Finding` | Operating system calls and the code that classifies | Kernel identity, executable path, parent, class, evidence, age |
| `ModelFinding` | The JSON output, and later the MCP tool result | The fields below and nothing else |
| `HumanDisplay` | The terminal and, later, elicitation prompts | Fixed templates. Every value has C0, C1, escape and bidi controls escaped |

`ModelFinding` has seven fields, in this order:

| Field | Value |
| --- | --- |
| `item_id` | `p<pid>-<start time in microseconds, hexadecimal>`. It names one incarnation of a process, so a reused PID has another ID |
| `class` | The class name |
| `exe_base` | The executable file name, only when it is at most 64 bytes of letters, digits, `.`, `_`, `+`, `@` and `-`. Otherwise `null`. A process picks its own name, so a name with a space, a control or text that reads as an instruction is not shown to the model |
| `pid` | The PID |
| `age_secs` | Whole seconds |
| `evidence` | Evidence kinds from the table above, in the order of the table |
| `cwd_relation` | `same_repo`, `other_repo`, `home`, `temp` or `other` |

The relation is computed from the working directory at display time and the path is dropped. A directory with a `.git` entry (file or directory) in it or above it, up to 64 levels, is a repository, and the search never looks at the home directory or above it, so a `.git` in the home directory does not make everything below it one repository. The checks run in this order: a repository that is the repository of the working directory of `doctor` is `same_repo`, another repository is `other_repo`, the home directory itself is `home`, a path under `/tmp`, `/private/tmp`, `/var/folders`, `/private/var/folders` or the temp directory of the user is `temp`, and anything else, including a directory that cannot be read, is `other`.

The terminal block for a finding is fixed:

```
p4242-1a2b  owned-ended  pid 4242  node  age 2h 05m  cwd other_repo
  evidence: owned.tag, owned.agent_gone
  command: node server.js --port 3000
  directory: /Users/dev/app
```

`(unlisted)` stands in for a hidden executable name, `(unreadable)` for a command or directory that cannot be read, and `none` for empty evidence. The command is the arguments joined by spaces after redaction, cut to 120 characters (the last three are `...`). The directory keeps its tail within 80 characters, with `...` in front. Both are read from the process at display time, and only after its identity was checked again, so they never come from the journal and never belong to a recycled PID.

Redaction is defence in depth. The model never receives the command, and the terminal shows it. It masks the value of an argument or flag whose name contains `token`, `secret`, `password`, `passwd`, `pwd`, `pass`, `key`, `auth`, `credential`, `cookie` or `bearer`, the value that follows a flag with such a name, the credential after `Authorization:` and `Bearer`, the user and password of a URL, a token with a known prefix (`sk-`, `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`, `xoxb-`, `xoxa-`, `xoxp-`, `xoxr-`, `xoxs-`, `AKIA`, `ASIA`, `eyJ`, `glpat-`, `npm_`) and an unbroken run of 32 or more letters and digits with at least one of each. Hyphens, underscores and dots separate words, so a long hyphenated directory name is shown. It cannot recognise a secret that has no such shape, and a false positive hides a harmless value.

Escaping writes a control as `\xNN` or `\u{NNNN}` and a backslash as `\\`, so it can be undone exactly and text cannot forge an escape. It covers C0 and C1 controls, DEL, the bidirectional controls, the line and paragraph separators and the zero width characters.

## The report

The text report:

```
agentdust doctor
processes: 1126 seen
journal: 3 records, 2 sessions (active 1, ended 1, unknown 0), 0 without a known agent
launchd: PID list read
classes: managed 1065, owned-live 1, owned-ended 1, likely-owned 0, suspect 0, unknown 59
tags without a usable session: 1
findings: 2

<one terminal block for each finding, separated by blank lines>
```

The last line of the journal section reads `, N lines skipped` when the reader skipped lines. The `tags` line appears only when it is not zero. Without findings the last line is `findings: none`. Only owned-ended, suspect, likely-owned and owned-live processes are listed, in that order and by PID inside a class. The other classes are counted: a list of every managed process would not help and would cost the model its context.

The JSON object has fixed keys in this order:

```
{"version":1,
 "owned_classes":{"available":true,"reason":null},
 "launchd":{"available":true},
 "sessions":{"records":3,"skipped_lines":0,"active":1,"ended":1,"unknown":0,"degraded":0},
 "unexplained_tags":1,
 "counts":{"managed":4,"owned-live":1,"owned-ended":1,"likely-owned":0,"suspect":1,"unknown":2},
 "findings":[<ModelFinding>, ...]}
```

(The real output is one line.) `unexplained_tags` counts the unknown processes whose evidence says the tag was unmatched, unverifiable or degraded.

When owned classes are unavailable, `owned_classes.available` is false and `reason` is one of these codes, and the text report prints `journal: owned classes are unavailable: <description>`:

| Code | Description | Cause |
| --- | --- | --- |
| `unsupported_version` | the journal has a schema version this build does not support | One line, even among good ones, has a newer version |
| `unsupported_filesystem` | the data directory is on a volume that is not local APFS (name) | The volume gate of the journal |
| `secret_unavailable` | the install secret is missing or unusable | Missing, wrong size, symlink, hard link or loose mode, or a loose data directory |
| `journal_refused` | the data directory or the journal failed a safety check | A symlink, a hard link, a foreign owner or a loose mode |
| `journal_unreadable` | the journal could not be read | Any other read error |

The reasons hold no path. A corrupt line is skipped, counted and reported, and does not make owned classes unavailable: it only lowers what is known.

## Safety requirements and where they are tested

| Requirement | What the tests pin |
| --- | --- |
| S1 | A property test over random snapshots, scopes and launchd lists: PID 1, another user, the tool and its ancestors, agents, system paths, launchd jobs and agents by executable or script are always managed whatever their tag, age, idleness or parent, and every other process gets exactly the class an independent oracle computes. Nothing is actionable without the launchctl list. Tables for each rule and its boundaries. The 22 fixtures, started for real |
| S15 | `ModelFinding` is destructured so that a new field fails to compile until the privacy review is redone. A planted path, a hostile executable name, a command and a directory never reach the JSON, in the library and through the command. Hostile text cannot add a line to the terminal block or the report |
| S19 | A lost journal, an unsupported journal, sessions without an agent identity, missing tag keys, unreadable liveness and a lost secret are compared with full provenance, as a table and as a property: nothing becomes owned-ended or newly suspect. A tag that no session explains is never a suspect |
| S14 | The raw tag is hashed at once, appears in no `Debug` output and in no output of `doctor`, and is in no file of the data directory |

The live tests start tagged and untagged trees with the harness, orphan some of them under launchd and keep one session alive. Findings are checked for the PIDs the harness registered only, never over the whole machine. The clock is moved 31 minutes ahead, so no test waits for the age threshold, and the 2 second gap is shortened to 150 ms. Another user, PID 1 and the system paths are added through a scripted source. The harness signal log holds the kills of the stand-in agents and nothing else.

## The fixture corpus

`crates/agentdust-testkit/tests/classify_fixtures.rs` classifies all 22 fixtures of `fixtures/m1`. For each one it starts the plan through the harness, turns the journal events into records with the identity of the stand-in agent and a tag key for each `session_start`, and gives the tag to the process group whose children a `sample` event names. Every sample in the corpus names all the live children of its group, so none is skipped. The k-th `session_start` belongs to the k-th stand-in agent and later ones to the last, which is how the resumed fixture gets two scopes. The five record-only fixtures get recorded attributes (PID 1, another user's daemon, a launchd job with a helper, a Homebrew service, an app and its helper), because a test cannot start them.

All observed processes get their labelled class. The three claims of success criterion 5 of the roadmap hold, and the evidence of each protected case is the one that protects it: the launchd job and the service by `managed.launchd`, the job's helper by `managed.launchd_child`, the app helper by its system path and the other user's daemon by its user.

## Where this differs from the spec

- Release 0.1 has no event driven sampling, so the tag is the only ownership evidence. Spec 3.4 also lets "sampled as a descendant" make owned-ended.
- Spec 3.4 says "the launcher chain is dead". On macOS the kernel reparents the children of an exited process to launchd at once, so the chain is dead exactly when the parent is PID 1. A parent that is missing from the snapshot is not evidence: the listing is not atomic, and a false suspect can be signalled.
- A direct child of a launchd job is managed. The spec says a PID that matches a loaded job. `launchctl list` names only the main PID, and the fixture `launchd_job_lookalike` labels the helper managed. The rule stops at one parent. A walk through every ancestor would make everything under an application such as a terminal managed, so the result would depend on where the tool was started from. Only a process whose parent is PID 1 can be a suspect, so a longer rule would protect no more suspects than this one.
- `/usr` covers `/usr/local`, so a tool installed there on an Intel Mac is managed and never listed. Homebrew on Apple silicon is in `/opt/homebrew` and is not covered. Over-protection is the safe error.
- The agent executable rule has a third form: the native installer layout `.../claude/versions/<version>`. The hook walk and the deny list share it (`ancestry::agent_exe`). Without it a Claude Code installed natively has no agent identity, so every one of its sessions is degraded.
- A tag in any state but `Absent` blocks suspect, and an unreadable environment (a parse or read error, as opposed to a hidden one) is not `Absent`. The spec only says "no tag".
- A degraded session is no ownership evidence, as the provenance page says, and the tag is still not "no tag". The process is unknown, not owned-live.
- `exe_base` is shown to the model only when it is plain. The spec says `exe_base` without a rule. `age` is `age_secs`.
- `doctor` lists owned and suspect findings and counts the rest. `--json` is an object with the findings, the counts and the provenance state, because the output has to say when owned classes are unavailable.
- The doctor refuses unsafe trusted state and reports it. It does not correct a wrong directory mode as spec 7.3 says for commands that write.
- Missing launchctl list or executable path: the spec is silent. Both keep a finding out of every actionable class.

## Not built here

- The `likely-owned` class, listening ports, event driven sampling, `agentdust status`, `health.json`, the support bundle and the MCP tool `agentdust_doctor`. The report and its JSON view are what the tool will return.
- Plan and apply.
- A live test with a busy process. The harness fixture only sleeps, so the idleness boundary has table tests and no live case. A flag that burns CPU would add one.
- The thresholds are not bounded from below by the corpus (see above).
- An agent that only a session scope identifies, and that no rule recognises by name or script, is managed while the journal is readable and can become a suspect when it is not. The hook finds an agent by the same rules, so this needs an `exec` after the hook ran.
- A busy machine: the idle sample is 2 seconds long, so a process that wakes once a minute looks idle.

## Tests

| Subject | Files |
| --- | --- |
| Escaping, truncation, redaction | `crates/agentdust-core/tests/sanitize.rs`, `crates/agentdust-core/tests/sanitize_corpus.rs`, `fuzz/fuzz_targets/sanitize.rs` |
| Snapshot, tag, idle sample, launchctl list | `crates/agentdust-core/tests/inventory.rs`, `crates/agentdust-core/tests/launchctl.rs` |
| Live scan, CPU time, command and directory | `crates/agentdust-testkit/tests/inventory_live.rs` |
| Classes, deny list, thresholds | `crates/agentdust-core/tests/classifier.rs`, `crates/agentdust-core/tests/classifier_launchd.rs` |
| S1 property and S19 comparison | `crates/agentdust-core/tests/classifier_props.rs`, `crates/agentdust-core/tests/classifier_provenance.rs` |
| Live classification, provenance loss, no signal | `crates/agentdust-testkit/tests/classify_live.rs` |
| The 22 fixtures and the age grid | `crates/agentdust-testkit/tests/classify_fixtures.rs` |
| Model view, templates, working directory relation | `crates/agentdust-core/tests/finding.rs`, `crates/agentdust-core/tests/finding_display.rs`, `crates/agentdust-core/tests/cwd_relation.rs` |
| Sessions, secret load, report, run order | `crates/agentdust-core/tests/doctor_sessions.rs`, `crates/agentdust-core/tests/secret_load.rs`, `crates/agentdust-core/tests/doctor_report.rs`, `crates/agentdust-core/tests/doctor_run.rs` |
| The command on live processes and a journal | `crates/agentdust/tests/doctor.rs`, `crates/agentdust/tests/common/sleeper.rs` |
| The native installer layout | `crates/agentdust-core/tests/ancestry_native.rs` |

The command tests run copies of the test executable as the agent and as a tagged leftover, with a real journal and install secret in a temporary data directory, and they check that the leftover is alive after `doctor` ran.
