# Journal rotation and retention

`agentdust_core::journal` rotates the journal and trims it. These are library functions for the future CLI and server. The hook never calls them and the binary has no maintenance subcommand.

[ADR-1](adr-journal-format.md) explains why appenders take no lock and states the retention algorithm once, with a table of what happens to each kind of generation. [journal-schema.md](journal-schema.md) describes the records and the reader. Spec section 3.2 states the same algorithm, and the last section lists where the code differs from the spec.

```rust
Journal::rotate(&self, now_ms: u64) -> Result<Rotation, MaintenanceError>
Journal::retain(&self, policy: &Policy, now_ms: u64, current_boot: &str) -> Result<RetainReport, MaintenanceError>
Journal::prune(&self, policy: &Policy, now_ms: u64, current_boot: &str) -> Result<PruneReport, MaintenanceError>
```

`prune` is `rotate` followed by `retain` under one lock. The free functions `journal::rotate`, `retain` and `prune` use the system volume. The clock is the `now_ms` argument: it names a generation, it is the age of the age rule, and it names a copy. Nothing else reads time. `Journal::with_probe` installs a `MaintenanceProbe`, which the tests use to hold a run at a named point.

## Files in the data directory

| Name | Written by | Content |
| --- | --- | --- |
| `journal.jsonl` | appenders | The active file. Maintenance renames it and never opens it for writing |
| `journal.<stamp>.jsonl` | rotation, retention | A closed generation. `<stamp>` is a canonical decimal number, the millisecond of the rotation moved up to the next free number |
| `journal.maint` | rotation, retention | Empty. Taken with a non-blocking exclusive `flock`. Appenders and readers never open it |
| `journal.compact.tmp` | retention | The replacement of one generation, created exclusively with mode 0600 |
| `journal.jsonl.corrupt-<n>` | retention | A byte for byte copy of a generation that held a line that did not parse, mode 0600 |

A reader reads `journal.jsonl` and every file whose name is exactly `journal.<canonical decimal>.jsonl`. `journal.007.jsonl`, `journal.5.jsonl.tmp` and `journal.jsonl.corrupt-5` are not journal files and are ignored.

## One rotation

1. Opens the data directory with the safe open rules. A missing directory is left missing and the result is `Rotation::Empty`.
2. Asks the volume probe about the directory. A volume that is not local APFS returns `UnsupportedFilesystem` and creates nothing.
3. Creates `journal.maint` exclusively when it is missing, opens it with the safe open rules and takes the lock without blocking. A held lock is `Busy`.
4. Opens every existing generation `journal.<stamp>.jsonl` with the safe open rules, oldest stamp first, and closes it again. The first one that is a symlink, a FIFO, a directory, hard linked, owned by another user or too loose fails the run with `Refused` and its path, before anything is renamed. The check is the one retention makes, shared in the code. A FIFO is refused without blocking.
5. Opens `journal.jsonl` for reading with the safe open rules. A missing or empty file is `Rotation::Empty`.
6. Renames it to `journal.<stamp>.jsonl`, taking the next free stamp when the name is taken, and syncs the directory. Nothing is truncated and the inode is kept.

The active file is renamed and never rewritten. An appender that opened the old inode and writes later lands in a file that readers still list, and its recheck sees that the path no longer names that file and writes the frame again. That recheck is the protection of a late writer, so no step of rotation or retention waits for time to pass.

## One retention run

[ADR-1](adr-journal-format.md#retention) gives the steps and the outcome of each generation. In short: it opens every file before it changes any, reads them all, judges every record together by the rules below, and then leaves each closed generation as it is, replaces it under its own name or deletes it. It never visits the active file and never moves a record.

`prune` opens every file once to refuse before it renames anything, rotates, and then runs the same retention on a fresh set of opens, so the generation it has just sealed is a file like the others and is retained in the same run. An active file that appenders have created since the rotation is counted and never changed.

### The rules

| Rule | Condition | Effect |
| --- | --- | --- |
| Earlier boots | The record's boot is not `current_boot` | Dropped on every run |
| Age | The session has ended and `now_ms` minus the newest `wall_ts` of the session is more than `ended_max_age` (14 days) | Every record of the session is dropped |
| Size, ended sessions | The kept records total more than `max_bytes` (20,000,000) | Ended sessions are dropped whole, the least recently active first, until the total fits |
| Size, pinned sessions | Still over `max_bytes` | Records of sessions that have not ended are dropped, the least recently active session first, oldest record first, `session_start` last. Each such session is listed in `RetainReport::degraded` with the number of records lost |

A session is an agent, a session id and a boot. A subagent record carries its parent's session id and belongs to the parent's session. The same session id under two agents, or in two boots, is two sessions.

A session has ended when it has a `session_end` record and every `session_start` of the session has a smaller `mono_ts` than its earliest `session_end`. Only `mono_ts` within the boot is compared. The wall clock and the order in which a run read the records never decide, because neither shows which event came first. A `session_start` that is not earlier than the earliest `session_end` keeps the session open. That covers a resume, a start with the same `mono_ts` as an end, and a `session_end` that arrives after a resumed start: M1 records carry no agent identity, so a delayed end of the old run cannot be told from the end of the resumed one. The session's records are pinned, and they are dropped for the size limit only as pinned evidence, with a `degraded` entry. The cost is that a resumed session that did end stays pinned until the next boot, when every record of the earlier boot is dropped. Spec 3.3 opens a resumed scope for a `session_start` with a different agent identity and ignores a duplicate one with the same identity. The M1 scope is an agent, a session id and a boot, so retention pins more evidence than the spec does and never less. A finer scope arrives with the agent identity that M2 records. A late event of any kind keeps the whole session because the age is judged from the newest record.

The size of a record is its whole frame: the JSON bytes plus the 0x1E and the 0x0A. Only records that parse count. Lines that do not parse, lines of an unknown kind and lines of a newer schema version are not counted. A record that cannot be dropped, because it sits in the active file or in a generation that holds a newer schema version, still counts, so a journal can stay above the limit until those files change. The planner is `journal::retention::plan`, a pure function of the records, the sizes, the policy, `now_ms` and the current boot.

### Reports

| Field | Meaning |
| --- | --- |
| `generations` | Closed generations read. It equals `held_newer_version + untouched + rewritten + deleted` |
| `held_newer_version` | Generations left as they are because a line is of a newer schema version |
| `untouched`, `rewritten`, `deleted` | What happened to the others |
| `kept_records` | Records that remain in every file read, the active file included |
| `dropped_earlier_boot`, `dropped_aged`, `dropped_over_cap`, `dropped_pinned` | Records dropped by each rule |
| `duplicates_removed` | Records dropped because an older generation holds the same bytes |
| `dropped_damaged_lines` | Malformed and torn lines removed by a rewrite or a delete |
| `malformed_lines`, `torn_frames`, `unknown_kind_lines`, `newer_version_lines` | Lines of each class in every file read, whether or not anything changed |
| `corrupt_copies` | Paths of the copies made |
| `degraded` | Sessions whose pinned evidence was dropped, with the count |

`PruneReport` holds the `Rotation` and the `RetainReport`.

### Errors

| Error | When | What changed |
| --- | --- | --- |
| `Busy` | Another run holds `journal.maint` | Nothing |
| `EmptyBoot` | `current_boot` is empty, which would make every record look like an earlier boot | Nothing, and the check precedes any file access |
| `UnsupportedFilesystem(facts)` | The volume is not local APFS | Nothing is created |
| `Refused { path, source }` | The data directory, the lock file, the active file or a generation is a symlink, a FIFO, a directory, hard linked, has a loose mode or another owner, or on macOS has an extended ACL with an allow entry | Nothing, except that `journal.maint` may have been created. A FIFO is refused without blocking |
| `Io` | Any other failure, such as a permission error on a generation or a taken `journal.compact.tmp` directory | Generations already visited stay as they were left. The one that failed is intact |

A run that fails on a generation never deletes it. A reader reports an unsafe generation and goes on, while a maintenance run refuses, because it must not act on a directory it cannot account for.

## What the tests pin

| File | Pins |
| --- | --- |
| `journal_retention.rs`, `journal_retention_props.rs` | The planner: 34 cases and 8 properties at 1,024 cases each, including a session resumed after its end, a delayed end of the old run and ties in `mono_ts` |
| `journal_rotate.rs` | Rotation, the stamp, the lock, the volume gate, every refusal, the probe points |
| `journal_rotate_interleave.rs` | A writer paused before and after its write across rotations, a reader paused after its open and after its listing |
| `journal_retain.rs` | The algorithm file by file: replacement under its own name, deletion, duplicates, the active file, the lock, the gate, unsafe files, torn and garbage lines, unknown kinds |
| `journal_retain_adr.rs` | Every row of the ADR outcome table against the code, and the sentences of the ADR that the code can check |
| `journal_retain_recovery.rs` | Copies, crash states, a failed rewrite, and S18: a newer-version line, one over 65,536 bytes included, holds its generation |
| `journal_retain_rules.rs` | The rules of spec 3.2 through generation files |
| `journal_retain_interleave.rs` | Real rotation and retention around a paused writer, around a writer that resumes between the scan and the replacement, around a run that holds the lock, and around a paused reader |
| `journal_retain_stress.rs` | Three writer threads, a straggler and a rotator, with a reader that checks every record acknowledged before it began |
| `crates/agentdust/tests/hook_never_prunes.rs` | The hook rotates, compacts and deletes nothing, takes no lock and has no maintenance command |
| `crates/agentdust-testkit/tests/journal_stress.rs` | The same with real processes (macOS only) |

The writer interleavings run the append through the `FrameWriter` seam, which pauses it after its open or after its write. A pause is a barrier in the test and not a sleep: the test thread waits until the writer is parked, runs rotation and retention, and then lets the writer go.

No test asserts a wall-clock bound below 60 seconds. The only bounds are hang guards of 60 seconds on channels and condition variables, and the one ignored latency test elsewhere in the repository. No liveness assertion depends on throughput. The two stress tests do a fixed amount of work and end with a forced final run, so their counts do not depend on how fast the machine is.

## The stress tests

The rotator appends a record of an earlier boot at the start of each cycle and then runs `prune`, so retention always has something to drop. After the stop file appears, or after every writer has finished, it runs one more cycle. That cycle rotates what the writers left, drops its marker and so replaces or deletes a generation. The assertions that follow do not depend on timing: every record that was acknowledged is in the read and on disk exactly once, the markers dropped equal the markers appended, nothing else was dropped, and no duplicate, torn frame, temporary file, copy or active file remains.

The straggler appends through a `FrameWriter` that holds its descriptor until the rotator has completed a whole run that began after the descriptor was opened. Each of its appends therefore writes into a file that has been rotated, and needs a second attempt. The test asserts that the fewest attempts of any of its appends is 2.

| Run | Work | Load average, 1 minute | Result |
| --- | --- | --- | --- |
| Threads, `journal_retain_stress.rs`, 7 runs | 3 writers of 400 records, a straggler of 20 appends, a rotator | 35 to 39 | 0.89 to 1.02 s each |
| Threads, 5 runs | the same | 2.6 to 2.7 | 0.87 to 0.94 s each |
| Processes, `journal_stress.rs`, 5 runs | 3 writers of 4,000 records, a straggler of 12 appends, a rotator | 2.2 to 2.5 | 1.63 to 1.78 s each |
| Processes, 3 runs | the same | 2.6 to 2.9 | 1.70 to 1.89 s each |
| Processes, 1 run beside 10 busy loops | the same | 8.8 | 3.42 s, 42 rotations, 65 reads |
| Threads, 1 run beside 10 busy loops | the same as the first row | 13.5 | 1.14 s |

Every run found every acknowledged record once. The 8 runs with processes at a load below 3 had 24 to 25 rotations, each followed by a replacement, 0 failed appends, 36 to 41 reads and 2 as the fewest attempts of the straggler. The multi-process test runs 12,000 appends and 24 to 25 rotations per run, which is a small rotation count: the run time divided by the rotations is about 70 ms, and each cycle syncs the files it changes and the directory.

The mutation checks below were run once each, and each makes the named tests fail:

| Change | Failing tests |
| --- | --- |
| The append recheck always answers yes | 6 of 11 in `journal_retain_interleave.rs`, 3 of 7 in `journal_rotate_interleave.rs`, both stress tests |
| Retention does not hold a generation with a newer-version line | 3 in `journal_retain_recovery.rs` and the ADR table test |
| Retention appends the kept lines to the active file and deletes the generation | 14 in `journal_retain.rs`, 3 in `journal_retain_adr.rs`, 8 in `journal_retain_interleave.rs`, 7 in `journal_retain_recovery.rs` |
| No copy before a rewrite | 8 in `journal_retain_recovery.rs` and the ADR table test |
| The lock is shared and not exclusive | 1 in `journal_rotate.rs` (the one that holds a run at the probe) |
| The hook calls `prune` | 1 in `hook_never_prunes.rs` |
| A session with one `session_end` after all its starts is ended | 2 in `journal_retain_rules.rs`, 4 in `journal_retention.rs`, 2 of the 8 properties in `journal_retention_props.rs` |
| A resumed session, a delayed end of the old run and an equal `mono_ts` keep the session open | 5 in `journal_retention.rs`, 1 in `journal_retain_rules.rs` |
| The wall clock and the read order never decide the state | 2 in `journal_retention.rs`, 3 of the 8 properties in `journal_retention_props.rs` |
| A standalone rotation skips the check of the existing generations | 7 in `journal_rotate.rs` |

## Limits

- Nothing calls `prune` yet. The CLI and the server decide the cadence, and each run that finds bytes in the active file adds one generation.
- The whole journal is read into memory. At the default limit that is about 20 MB of frames and the same again for their records. A journal of that size was not measured.
- `degraded` is returned and not stored. The M2 doctor has to carry it.
- Copies are never deleted by retention.
- A session ends for retention only with a `session_end` record. A session whose agent died without one stays pinned until the size limit drops it. Spec 3.3 also ends a session when its agent identity is gone, and retention has no view of processes.
- A line of an unknown kind is kept for ever, because retention cannot tell its session or its age.
- A symlink, a hard link or a loose mode on one generation stops every run until a person removes it. That is the refusal of spec 7.3 and it is a standing denial of retention for whoever can plant a file, which is out of scope under spec 7.4.
- Rotation and retention sync the replacement and the directory. A crash was simulated by planting the files each crash point leaves, and no power was pulled.
- The tests that need a real APFS volume are macOS only. The Linux job compiles and lints `agentdust-core` and `agentdust-testkit` for `x86_64-unknown-linux-gnu`, and the platform independent tests use `FixedVolume`. They were not run on Linux here.
- No network volume was available.

## Where this differs from the spec

- Spec 3.2 says records from earlier boots are pruned first. They are dropped on every run, because no process of an earlier boot can be signalled.
- The limit of "20 MB" is 20,000,000 bytes of frames of records that parse.
- Spec 3.2 says provenance is marked degraded. The mark is a list in the report.
- Spec 3.3 opens a resumed scope for a `session_start` with a different agent identity and ignores a duplicate one. Retention keeps a session open when a `session_start` is not earlier than its first `session_end`, because M1 records carry no agent identity (see "The rules").
