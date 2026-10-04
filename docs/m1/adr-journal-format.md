# ADR-1: The journal is one append-only file, appended without a lock and rechecked after the write

**Status:** accepted. The spec sections 3.2, 4.4, 7.2, 7.3, 9.1, 9.2 and 10 and the ROADMAP are edited to match. "Spec edits" lists them.
**Date:** 2026-10-04 · **Deciders:** the repository owner
**Evidence:** [journal-benchmark.md](journal-benchmark.md)

## Decision

We will append each journal record as one frame, the byte 0x1E, the record as compact JSON and the byte 0x0A, with one `write(2)` on a descriptor opened with `O_APPEND` and `O_NOFOLLOW`. Right after the write we compare `fstat` of the descriptor with `lstat` of `journal.jsonl`. When the path now names another file, or none, we write the frame again, up to 3 attempts in all. Appenders and readers take no lock. Rotation renames the active file to a generation and retention replaces or deletes closed generations, under a lock that only those two take. The journal directory must be on a local APFS volume, and on any other volume the writer writes nothing and says why. A frame is at most 65,536 bytes. Appends are not synced.

This is candidate C2 of the benchmark. Section "Why C2" says why it beats the other three.

## Context

The M0 design of spec section 3.2 kept the journal in `journal.jsonl` behind a `journal.lock` that every reader, writer and rotator takes, "until the M1 contention benchmark decides otherwise". Hooks have p50 under 10 ms and p95 under 20 ms (ROADMAP v1.0 criterion 2, spec 4.4). The M0 hook measured p50 2.50 to 2.68 ms and p95 2.85 to 3.36 ms ([M0 report](../m0/report.md)), which leaves about 16 ms of p95 for the journal.

Rotation and retention have to work while hooks write. That is the hard part: an appender that has opened the active file and is then paused writes into whatever that file has become by the time it resumes.

## What the benchmark found

Four candidates, real writer, reader and rotator processes, 3 runs, 144,000 appends per candidate. The tables, the method and the raw data are in the [benchmark page](journal-benchmark.md). C is `O_APPEND` and nothing else, and D takes a shared `flock` around the append.

| Measure | A: exclusive flock | C: O_APPEND | C2: C plus recheck | D: shared flock |
| --- | --- | --- | --- | --- |
| Loses a record when a writer pauses between its open and its write across a rotation and a compaction | no | **yes** | no | no |
| Acknowledged records lost, 3 runs | 0 | **115** | 0 | 0 |
| Records dropped, 3 runs | **9,744** (6.8%) | 0 | 0 | 0 |
| Records written twice, 3 runs | 0 | 0 | 1,619 (1.1%) | 0 |
| Rotation attempts that gave up at the 50 ms budget, 3 runs | 4 of 2,657 | 0 of 637 | 0 of 660 | **39 of 382** |
| Longest wait for a rotation, ms | 77.6 | 24.2 | 31.4 | 59.8 |
| p50 of the append, ms, four cells of run 1 | 0.053 to 0.212 | 0.050 to 0.115 | 0.054 to 0.100 | 0.081 to 0.123 |
| p99 of the append, ms, four cells of run 1 | 19.5 to 21.8 | 4.1 to 7.2 | 3.9 to 6.6 | 1.6 to 5.2 |
| Locks an appender takes | exclusive | none | none | shared |
| Lock files | `journal.lock` | `journal.maint` | `journal.maint` | `journal.lock`, `journal.maint` |

- **Loses a record.** The first row is not a measurement. It is asserted by [interleavings.rs](../../crates/agentdust-bench/tests/interleavings.rs), which pauses a writer at an exact point of the append and runs the rotator, a read and retention around it. C loses the record when the pause outlasts the grace window and a compaction or a deletion has replaced or unlinked the generation the writer holds. The measured losses are the same event with real processes.
- **Torn and interleaved lines.** None in 576,000 appends on APFS, for all four candidates.

## Why C2

**A drops records.** Under a rotator and a reader it drops 6.3% to 7.6% of its appends, and its p99 sits on the 20 ms lock budget. Its lock also covers every read and every rotation and retention pass, and the run does not apportion the drops between those holders.

**C loses records.** It is the fastest candidate and the only one whose acknowledged records disappear: 115 in 144,000, 0.12% in each stress run, and 1 in the run with the syncs on. A grace window of the length of the hook timeout would protect a writer paused for less than that and no longer, and the interleaving tests show the loss beyond it. C needs a clock to be safe, and the clock has no right answer.

**D is correct and loses nothing, but it puts a lock back in the hook path.**

- **Rotation starves.** The rotator needs a moment in which no appender holds the shared lock. Over the three runs it gave up at its 50 ms budget on 39 of 382 attempts (10%), and on 5 of 15 in the cell of 16 writers and 4,000 bytes in both stress runs. Its median wait reached 20 ms in a cell. Appenders never waited for it, so what starves is rotation, and a journal that cannot rotate while hooks fire grows.
- **A stopped rotator stops the hooks.** It holds the exclusive lock for a rename, and an appender that finds the lock held for 20 ms drops its record, which is A's failure at a smaller scale. This was not measured.
- **A stopped appender stops rotation.** It holds the shared lock until it resumes or dies. [candidates.rs](../../crates/agentdust-bench/tests/candidates.rs) and [interleavings.rs](../../crates/agentdust-bench/tests/interleavings.rs) show the rotator being refused while one is paused.
- **It needs two lock files and one more `open` and `flock` per append.** Its p50 is 0.008 to 0.047 ms above C's in the four cells of run 1.

**C2 loses nothing and waits for nobody.** In the interleaving tests the record is present exactly once in every scenario, and in 144,000 attempted appends in three runs under a rotator it lost none. No append failed with `Stale`, so no append used up its 3 attempts. Rotation never skipped and its median wait was 0.22 to 0.61 ms per cell in the stress runs. The recheck is two system calls, and C2's p50 is within 0.03 ms of C's in all four cells of run 1.

Its price is that a record can exist twice. When a rotation lands between the write and the check, the first copy sits in a generation and the second in the new active file. 1.1% of C2's appends were written twice. Readers collapse records that are equal in every field, and retention drops an exact duplicate of a record it kept earlier in the same run, so no consumer sees a duplicate and none remained after the final retention pass of any run. A consumer that reads the files directly and not through the reader would see both copies.

## The append

1. Encode the record as compact JSON and frame it. A frame longer than 65,536 bytes is refused with `TooLarge`, and nothing is opened or created for it. The hook drops the record silently, and the hook health counters are planned for M2.
2. Check the volume of the journal directory (section "Local APFS only"). When it is not supported, write nothing and stop. The hook drops the record silently.
3. Open `journal.jsonl` with `O_APPEND`, `O_CREAT`, `O_NOFOLLOW` and mode 0600, in a directory of mode 0700.
4. Write the frame with one `write(2)`. A call that returns `EINTR` before it transferred anything is restarted. A call that returns fewer bytes than the frame, zero included, is an error and is never completed by a second call, because a second call would not be atomic with respect to other appenders.
5. Compare `fstat` of the descriptor with `lstat` of the path. They match when `st_dev` and `st_ino` are equal and `st_nlink` is not 0. If they do not match, close the descriptor and go to step 3. After 3 attempts the append fails with `Stale`.
6. Return. There is no `fsync`.

A record is acknowledged when `Journal::append` returns `Ok(Appended)`: one `write(2)` transferred the whole frame and the recheck of step 5 then found the path still naming the file that was written. A write that succeeded and whose recheck failed is tentative and is not an acknowledgement, because the file it reached is no longer the active file. The append writes the frame again, and each tentative copy is one of the copies that readers and retention collapse. When all 3 attempts end in a failed recheck the append returns `Stale` and the record is not acknowledged. Its tentative copies may or may not remain: one written into a file that rotation sealed stays readable until retention drops it, and one written into a file that was unlinked is gone. The tests pin both: an unlinked active file under every attempt leaves no copy at all (`journal_append_recheck.rs`), and a rotation after every write leaves the copies in generations. A missing record whose append returned `Stale` is therefore not a lost record.

The guarantees below speak of acknowledged records only. A record that was not acknowledged can be present zero, one or several times before the reader collapses equal records.

The atomicity of step 4 is a reliance on POSIX append semantics for local regular files, a write on an `O_APPEND` descriptor lands whole at the end of the file while other processes append. It is not an APFS guarantee and it does not hold on network file systems. It was stress tested on APFS with 3 and 16 writer processes and no torn or interleaved line appeared. [tests/frame.rs](../../crates/agentdust-bench/tests/frame.rs) pins the byte layout, and `sixteen_writer_processes_with_4000_byte_records_tear_nothing` in [tests/harness.rs](../../crates/agentdust-bench/tests/harness.rs) pins the concurrency.

## Framing

A frame starts with the byte 0x1E (record separator) and ends with 0x0A. Compact JSON cannot contain a raw 0x1E or a raw 0x0A, because the encoder writes both as escapes (`tests/frame.rs`, `json_text_never_holds_a_raw_separator...`). The decoder splits on both bytes and applies three rules:

- A segment that ends with 0x0A is a candidate record.
- A segment that ends with 0x1E or at the end of the file has no closing 0x0A. It is torn: counted and never parsed, even when it happens to be complete JSON.
- An empty segment is ignored.

This makes a damaged write cost one record. If a write is cut anywhere, the next frame starts at its own 0x1E and is read whole. Without the 0x1E the cut text and the next record form one line that parses as neither, and both are lost. `tests/frame.rs` cuts a frame after 0, 1, 2, half, all but 2 and all but 1 bytes and checks that the next record is read in every case, and that without framing it is lost.

The short write cases of the write itself (`EINTR` before any transfer, a zero byte write, a short write of 1 byte and of N minus 1 bytes, `ENOSPC`, `EIO`) are injected into the real append by `crates/agentdust-core/tests/journal_append_faults.rs`. The injected writer keeps the bytes that reached the file, the test appends record N plus 1 for real, and both are read back. [journal-schema.md](journal-schema.md#what-survives-a-failed-write) lists what survives each case.

A line of a newer schema version is counted apart from a malformed line, and that includes a line longer than the cap. The decoder never buffers more than 65,536 bytes of one segment, and it recognises the version of a longer segment from its start, `{"v":` and the digits. Writers therefore write `v` as the first key, and every future schema keeps it first. Retention never shrinks a generation that holds such a line.

## Local APFS only

The writer supports one place for the journal: a local APFS volume. `statfs(2)` on the journal directory must report `f_fstypename` equal to `apfs` and `MNT_LOCAL` set in `f_flags`. On any other volume the writer opens nothing, writes nothing and returns `UnsupportedFilesystem` with the facts. The hook drops the record silently, and a reason in hook health is planned for M2. The refusal has three grounds: a network volume breaks the premise of an atomic append, SMB is unproven, and a local volume that is not APFS was not tested.

What `journal-bench fs` reported on the benchmark machine:

| Path | Result |
| --- | --- |
| `$TMPDIR`, `$HOME`, `/private/tmp`, `/System/Volumes/VM` | apfs, local, supported |
| `/dev` | devfs, local, not supported |
| `/System/Volumes/Data/home` | autofs, not local, not supported |

No NFS or SMB volume was available, so the refusal of a network volume rests on the flag and was not observed.

- **Injectable.** The check is the `VolumeProbe` trait of [volume.rs](../../crates/agentdust-core/src/journal/volume.rs), a function from a path to the file system name and the flags, so a test supplies them and platform independent code and the Linux job exercise the whole decision. `classify(name, flags)` is the decision and `SystemVolume` is the one `statfs` call. `locate` asks about the nearest existing ancestor of a directory that does not exist yet, so a refused volume gets no directory either.
- **Status.** `Journal::status()` returns the file system name, whether it is local and whether it is supported, and a later `agentdust status` prints them in the form `apfs, local, supported`.
- **Reads.** A read cannot tear anything, so readers do not refuse an unsupported volume. `ReadReport.filesystem` carries the facts and `status` reports them. Nothing is written there, so a new record never arrives.

## Durability contract

A successful append means the bytes reached the kernel's file cache. They survive the death of the hook process. They can be lost on an OS crash or a power failure, and the contract promises nothing about either. Lost evidence only lowers confidence (spec S19), and no class is made more certain by a missing record.

`journal-bench durability` measured one writer appending 150 byte records with each of three sync choices (200 samples each, load average 36 to 45):

| Mode | p50 ms | p95 ms | p99 ms | max ms |
| --- | ---: | ---: | ---: | ---: |
| none | 0.002 | 0.004 | 0.009 | 0.029 |
| `fsync(2)` | 0.044 | 0.093 | 0.123 | 0.162 |
| `File::sync_all` | 4.000 | 5.918 | 7.228 | 19.954 |

`File::sync_all` costs 4.0 ms at p50 for every record, 90 times `fsync(2)`, which fits a flush that reaches the drive, and a tool call writes at least two records. `fsync(2)` costs 0.044 ms, and on macOS it does not ask the drive to flush its cache, so it would survive an OS crash and still not a power failure. Neither is adopted, and `fsync(2)` could be added later without changing the format.

Rotation and retention do sync, because they run off the hook path and a replacement must not reach the directory before its data does: the replacement file is written, `File::sync_all` is called on it, it is renamed over the generation, and the directory is synced. With the syncs on, a median rotation of A, C or C2 takes 4.4 to 6.0 ms in a cell. A crash before the rename leaves `journal.compact.tmp`, which the next run removes, and the old generation is intact.

## Retention

One algorithm, stated here once and implemented once, in [retain.rs](../../crates/agentdust-core/src/journal/retain.rs). The benchmark keeps a frozen copy of its own ([maintenance.rs](../../crates/agentdust-bench/src/maintenance.rs)), so a later change to the shipped code cannot change what was measured. [journal_retain_adr.rs](../../crates/agentdust-core/tests/journal_retain_adr.rs) runs every row of the table below against the code, so a change to one without the other fails a test.

A run takes the exclusive lock on `journal.maint` without blocking and reports `Busy` when another run holds it. Only rotation and retention take that lock. Then it:

1. Opens the active file and every generation with the safe open rules. When any of them is refused, the run changes nothing, except that `journal.maint` may have been created.
2. Removes a stale `journal.compact.tmp`.
3. Reads every file into memory and decodes it. The records of all files are judged together by the rules of spec 3.2, which say for each record whether it is kept. A record that cannot be dropped, because it sits in the active file or in a generation that holds a newer schema version, still counts toward the size limit.
4. Visits each closed generation `journal.<stamp>.jsonl`, oldest stamp first. It never visits the active file. The table gives the outcome.

| Id | The generation holds | Outcome | Copied aside first |
| --- | --- | --- | --- |
| G1 | a line of a newer schema version, one over 65,536 bytes included, and any other lines | left as it is | no |
| G2 | records that the rules keep, and nothing else | left as it is | no |
| G3 | an unknown-kind line and records that the rules keep | left as it is | no |
| G4 | a torn frame and records that the rules keep | left as it is | no |
| G5 | a line that does not parse and records that the rules keep | left as it is | no |
| G6 | records that the rules drop and records that they keep | replaced | no |
| G7 | only records that the rules drop | deleted | no |
| G8 | a record that repeats one of an older generation byte for byte, and records that stay | replaced | no |
| G9 | only records that repeat those of older generations | deleted | no |
| G10 | an unknown-kind line and records that the rules drop | replaced, the unknown-kind line kept as it was | no |
| G11 | a torn frame and records that the rules drop | replaced, the torn frame dropped | no |
| G12 | a line that does not parse, and records that the rules drop and keep | replaced, the line dropped | yes |
| G13 | a line that does not parse and only records that the rules drop | deleted | yes |
| G14 | no bytes | left as it is | no |
| G15 | a symlink, a FIFO, a directory, a hard link, a loose mode or, on macOS, an extended ACL with an allow entry where a regular private file belongs | the run is refused and nothing changes | no |

- **Replaced** means the lines that stay are written to `journal.compact.tmp` (created exclusively, mode 0600), in their original order and with their original bytes, each framed as 0x1E, the bytes and 0x0A. The file is synced and renamed over the generation, and the directory is synced at the end of the run. A bare line without the 0x1E gains its frame.
- **A duplicate** is a record in a closed generation whose bytes equal those of a record read earlier in the run. Files are read in stamp order and the active file last, so the copy in the older file stays. A duplicate in the active file is not touched and readers collapse it. Duplicates are removed before the rules run, so the size limit counts a record once.
- **A copy** is the whole generation, byte for byte, in `journal.jsonl.corrupt-<n>` (created exclusively, mode 0600, synced before the rewrite). `<n>` is the millisecond of the run, moved up to the next free number. A torn frame is dropped without a copy, because it is the trace of a write that failed and was never acknowledged. Retention never deletes a copy.
- **A line of a newer schema version** holds its generation, whatever else the generation holds. The line is recognised from its `{"v":` prefix, so a line over the frame cap is held as well.
- **A generation that cannot be read** is never deleted. A read error before the run changes anything fails the run, and the generation stays.

The retention rules stay in spec 3.2: earlier boots first, ended sessions after 14 days or past 20 MB, active sessions pinned. The algorithm only fixes how a rule is carried out. A pinned record is simply a kept record. It stays in its generation, in its place.

- **No record moves.** A record is never moved to another file and never appended again. A reader holds a set of open files, and a record that moved between them could be in none of the files that reader holds. Replacing a generation under its own name keeps every record in a file that a reader either has or will list.
- **No grace window.** No step depends on elapsed time. C2 needs none, because the recheck, and not a delay, protects a late writer. A grace window protects only a writer paused for less than its length, and the interleaving tests show C losing the record beyond it. The shipped code has no grace parameter. The one use of the clock is the age rule of spec 3.2 and the millisecond of a stamp, both passed in by the caller.
- **Generations stay separate.** Merging them would shorten the list but rewrite every older generation on each pass. A run rotates whenever the active file holds a byte, so the caller sets the cadence, and the list grows by at most one file per run.
- **Crash points.** A crash after the copy leaves the copy and the generation, and the next run makes another copy. A crash after the temporary file is written leaves a `journal.compact.tmp` that no reader reads and the next run removes. A crash after the rename leaves the replacement.

## Reading

A reader takes no lock. It opens `journal.jsonl` first, then lists `journal.<stamp>.jsonl`, then opens each generation, and skips one that no longer exists. A name counts as a generation only when it is `journal.` and a canonical decimal number without leading zeros and `.jsonl`. An active file or a directory that is unsafe fails the read with `Refused`. A generation that is a symlink, a FIFO, a directory, a hard link, owned by another user or has a loose mode is not followed and is counted in `unsafe_files`, so one planted file does not hide the rest of the evidence.

- **A coherent set.** Every record acknowledged before the read began, and not dropped by a retention rule, is in a file the reader holds. If a rotation moved the active file after the reader opened it, the listing that follows shows it as a generation, and the reader recognises that it is the same inode and reads it once. If retention replaced a listed generation, the reader opens the replacement or the original, and each holds the record. If retention deleted a listed generation, every record in it was dropped by a rule.
- **Records acknowledged during the read** may or may not appear.
- **Under a lock.** The reader of A holds the shared lock for the whole read and the reader of D holds it while it opens files, so the rotator cannot change the set in that time. C and C2 do without, for the reasons above, and the tests pause a reader after its open and after its listing to check all four.

## Ordering

The order of a read is a presentation order. It has three levels:

1. **Boot.** Boots are ranked by the earliest `wall_ts` among the records read for each, then by the boot identifier.
2. **Time within a boot.** `mono_ts`, which is read before the append, so a writer that is descheduled in between lands behind a later stamp. Timestamp order is not causal order, and no consumer may infer from it that one event caused another. Pairs are matched by `tool_use_id` and not by position.
3. **A fixed final key.** Records equal in boot rank and `mono_ts` are ordered by `wall_ts` and then by every field of the record in the order the struct declares them: `v`, `kind`, `agent`, `session_id`, `subagent_id`, `tool_use_id`, `wall_ts`, `mono_ts`, `boot`, `cwd_key`, `exe_base`, with `kind` and `agent` in the order of their variants. The key is the record itself, so records that are not equal never compare equal, the order does not depend on which file holds a record or in which order the reader opened the files, and no hash is needed. A hash would add collisions and an algorithm to keep stable across builds. [journal_read.rs](../../crates/agentdust-core/tests/journal_read.rs) checks that the same records in different file positions read in the same order.

The order across boots is weak. A boot is ranked by wall time, which can go backwards (a clock step, a manual change), so two boots can swap. Retention does not use the rank: it compares a record's boot with the current one.

## The 64 KiB frame cap

A frame is at most 65,536 bytes including its delimiters. The cap is chosen from latency. The benchmark that preceded this one found no torn line up to 4 MiB with 16 writers, and p99 of 3.2 ms at 64 KiB, 5.0 ms at 256 KiB and 34.0 ms at 4 MiB ([carried over](journal-benchmark.md#measurements-carried-over), not repeated). This run measured only padded records of 150 and 4,000 bytes, the second standing for a `sample` record.

The cap says nothing about a real `sample` record. The `procs` list of spec 3.2 has no length limit. The M2 sampler needs a byte budget below the cap and a truncation marker, `procs_total` and `procs_recorded`, so a truncated list is visible as one. Until it exists, a `sample` record over the cap is refused with `TooLarge`, and the hook drops it silently until the M2 health counters exist.

## Alternatives considered

**A: one file behind an exclusive lock.** The M0 design. Rejected for the drops above. Its lock keeps appenders apart, which `O_APPEND` already does.

**C: `O_APPEND` and no lock.** Rejected for the lost records above.

**D: a shared lock for appenders and readers, an exclusive one for the rotator.** Correct, and rejected for rotation starvation, for the lock in the hook path and for the second lock file.

**B: one segment file per writer handle.** Not measured again. Its numbers come from the benchmark that preceded this one and were not repeated: 1,601 to 1,655 files per 2,400 hook appends, and 215 to 231 ms to read 10,000 records against 2.7 to 17.4 ms for A and C. A hook is a new process, so every hook opens a new segment, and retention over thousands of files is a different design from the one measured here.

**A grace window on C.** Rejected: it protects only a writer paused for less than its length.

**Delete whole generations and append the pinned records again.** Rejected because a record that moves can be missed by a reader (section "Retention") and because it writes records that are already stored.

**Complete a short write with a second call.** Rejected because the second call is not atomic with respect to other appenders.

**Sync every append.** Rejected on cost: 4.0 ms at p50 per record against a hook budget of p50 10 ms.

**SQLite.** Not measured. Rejected on three grounds:

- **A new C dependency.** ROADMAP decides on Rust and one binary, and `libsqlite3` is C code compiled or linked into it.
- **Hook cold start.** Every hook is a new process, so opening the database and checking its schema is paid per event. C2 pays one `open`, one `write` and two `stat` calls.
- **Supply chain surface.** A C library enters the SBOM, the byte identical release build and the attestation, and `cargo deny` has to allow it.

## Consequences

### Positive

- **Hooks wait for nobody.** There is no lock and no retry loop in the hook path, so there is no drop path from contention.
- **No acknowledged record was lost** in these tests, including a rotator running up to 419 rotations per cell.
- **Readers cost the same for every candidate.** Reading 10,000 records takes 4.3 to 5.1 ms at 150 bytes and 36 to 40 ms at 4,000 bytes.
- **Rotation never waits.** Its median wait is 0.22 to 0.61 ms without the syncs and 4.4 to 6.0 ms with them.

### Negative

- **A record can exist twice** between a rotation and the next retention pass. Every reader has to collapse equal records, and anything that reads the files directly would see both.
- **An acknowledged record is not durable** against an OS crash or a power failure.
- **The atomicity is measured and assumed, not guaranteed.** The 16 writer test has to stay in CI, and a different file system breaks the premise. The volume check is what stops the premise from being used where it was not tested.

### Neutral

- **One `statfs` call per append** costs 0.0015 to 0.0038 ms at p50 in three runs, and `append` with the system probe is 0.0422 to 0.0430 ms at p50 ([cost](journal-schema.md#cost)).
- **`journal.maint` is empty.** Only rotation and retention take it.

## What this does not establish

- One machine, one macOS version and one local APFS volume, under a 1 minute load average of 25 to 45.
- Unpaced writers. A real hook fires far less often than 2,400 times in 100 ms.
- No write was killed in the middle, no disk was full and no power was pulled. The error cases are injected into the real append through the `FrameWriter` trait. The kernel did not produce them.
- The rotation stress in `journal_append_rotation.rs` has no retention in it, so it does not fail when the recheck is removed. The interleaving tests in `journal_rotate_interleave.rs` and `journal_retain_interleave.rs` do.
- Retention was run on journals of a few thousand records. A journal at the 20 MB limit is read into memory whole, and that was not measured.
- No network volume was available.
- Records are padded in `session_id`. A real `sample` record is not measured.
- A stopped rotator and a stopped appender were tested for refusal and not timed.
- The benchmark and the `agentdust-core` tests have not run on Linux. The crates compile for `x86_64-unknown-linux-gnu` with `cargo check --all-targets`.
- "No observed losses" means none in 144,000 attempted appends per candidate in three runs. It is not a proof, and the rotation stress used a 1 ms period that no hook workload reaches.

## Spec edits

Applied to [the spec](../superpowers/specs/2026-10-03-agentdust-design.md):

- Section 3.2, the storage paragraph and the retention paragraph. They state the frame, the append and its recheck, the acknowledgement, `journal.maint`, the reader and the retention steps.
- Section 9.1, the rows S22 (frame and atomic write), S23 (an acknowledged record is present exactly once), S24 (local APFS only) and S25 (the order of a read), each mapped to named tests, and the additions to the tests of S13, S16 and S18.
- The ROADMAP: open decision 1 is removed, the decisions table has a Journal row, and the M1 bullet and the latency risk name the 3 and 16 writer benchmark.

Applied: the remaining edits below are in the spec too, so the spec, this record and the code agree.

Section 4.4, journal write budget. Replaced:

```text
- Journal write budget: the lock is taken without blocking, with retries for at most 20 ms. On failure the record is dropped and hook health is updated. The health update follows the same budget and is best effort.
```

with:

```text
- Journal write budget: an append is a few system calls and takes no lock, so it waits for no other process. The record is dropped when the frame is longer than 65,536 bytes, when the volume is not a local APFS volume, when a write returns fewer bytes than the frame, or when the active file was replaced in 3 attempts in a row. The M1 hook drops it silently, and the health update, which is best effort, is planned for M2.
```

Section 4.4, health record. Replace "updated under the journal lock" with "kept in `health.json`, which is replaced by writing a temporary file and renaming it over the old one. The last writer wins, so a count can miss an update".

Section 7.2, table row. Replace:

```text
| `journal.jsonl`, `journal.lock` | section 3.2 fields only | section 3.2 |
```

with:

```text
| `journal.jsonl`, `journal.<stamp>.jsonl`, `journal.compact.tmp`, `journal.jsonl.corrupt-<n>`, `journal.maint` | section 3.2 fields only. `journal.maint` is empty and only rotation and retention take it. A `corrupt` file is a copy of a generation that held a line that did not parse | section 3.2 |
```

Section 7.3, after the paragraph on the data directory. Add:

```text
The journal directory must be on a local APFS volume: `statfs` reports `f_fstypename` equal to `apfs` and `MNT_LOCAL` in `f_flags`. On any other volume the hook writes nothing and drops the record silently. Hook health, which records `unsupported_filesystem` and the file system name, and `agentdust status`, which prints the file system name, whether it is local and whether it is supported, are planned for M2. `Journal::status()` returns the same facts.
```

Section 9.2, stress row. Replace "three concurrent journal writers, rotation under load, PID churn" with "3 and 16 concurrent journal writer processes with a rotator, a reader and retention running, PID churn".

Section 10, journal format row. Replace "M1 contention benchmark" with "M1 contention benchmark: one `O_APPEND` file, no lock for appenders and readers, an append recheck (docs/m1/adr-journal-format.md)".

## What the remaining journal tasks must change

Grouped by concern.

**Journal store** (`agentdust_core::journal`) is built: the append of steps 1 to 6, the fault injection writer, the volume check and the status value, the reader of "Reading" and "Ordering", and the framing with its decoder. One item is left:

- Point the benchmark's C2 at `Journal::append`, so that the 16 writer test (S22) runs against the shipped append. The benchmark keeps its own frozen copy of the append until then.

**Rotation, retention and recovery** is built: `Journal::rotate`, `retain` and `prune` in `agentdust_core::journal` follow "Retention" as stated, with the spec's keep rules in `journal::retention` and `journal.maint` for exclusion. [journal-retention.md](journal-retention.md) describes it. Two items are left:

- Persist or carry `RetainReport.degraded`, the sessions whose pinned evidence was dropped. It is returned and not stored.
- Call `prune` from the CLI and the server. Nothing calls it, and the hook never does.

**Hook write path and health** (spec 4.4, planned for M2):

- Count each kind of drop in hook health: too large, a short write, `Stale`, an unsupported volume, any other error.
- Write health to a temporary file and rename it, one file per agent, with no lock.
- Keep the 16 writer test (S22) and the interleaving tests (S23) in the Linux and the macOS job. They are `journal_append.rs`, `journal_append_recheck.rs` and `journal_snapshot.rs` in `agentdust-core`, and they are platform independent.

## Open questions

- **Is 64 KiB the right cap for a real `sample`?** It rests on latency. The M2 sampler decides the byte budget.
- **Should `fsync(2)` be added?** It costs 0.044 ms at p50 and promises survival of an OS crash only. The contract promises neither today.
- **Does the benchmark pass on Linux?** The tests are platform independent and have not run there.
