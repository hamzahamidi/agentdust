# ADR-1: The journal is one append-only file, appended without a lock and rechecked after the write

**Status:** proposed. The spec and the ROADMAP are not edited here. A person applies the edits in the last sections.
**Date:** 2026-10-04 · **Deciders:** the repository owner
**Evidence:** [journal-benchmark.md](journal-benchmark.md)

## Decision

We will append each journal record as one frame, the byte 0x1E, the record as compact JSON and the byte 0x0A, with one `write(2)` on a descriptor opened with `O_APPEND` and `O_NOFOLLOW`. Right after the write we compare `fstat` of the descriptor with `lstat` of `journal.jsonl`. When the path now names another file, or none, we write the frame again, up to 3 attempts in all. Appenders and readers take no lock. Rotation renames the active file to a generation and retention replaces or deletes closed generations, under a lock that only those two take. The journal directory must be on a local APFS volume, and on any other volume the writer writes nothing and says why. A frame is at most 65,536 bytes. Appends are not synced.

This is candidate C2 of the benchmark. Section "Why C2" says why it beats the other three.

## Context

Spec section 3.2 stores the journal in `journal.jsonl` behind a `journal.lock` that every reader, writer and rotator takes, "until the M1 contention benchmark decides otherwise". Hooks have p50 under 10 ms and p95 under 20 ms (ROADMAP v1.0 criterion 2, spec 4.4). The M0 hook measured p50 2.50 to 2.68 ms and p95 2.85 to 3.36 ms ([M0 report](../m0/report.md)), which leaves about 16 ms of p95 for the journal.

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

1. Encode the record as compact JSON and frame it. A frame longer than 65,536 bytes is dropped and counted, and nothing is opened or created for it.
2. Check the volume of the journal directory (section "Local APFS only"). When it is not supported, write nothing, count it and stop.
3. Open `journal.jsonl` with `O_APPEND`, `O_CREAT`, `O_NOFOLLOW` and mode 0600, in a directory of mode 0700.
4. Write the frame with one `write(2)`. A call that returns `EINTR` before it transferred anything is restarted. A call that returns fewer bytes than the frame, zero included, is an error and is never completed by a second call, because a second call would not be atomic with respect to other appenders.
5. Compare `fstat` of the descriptor with `lstat` of the path. They match when `st_dev` and `st_ino` are equal and `st_nlink` is not 0. If they do not match, close the descriptor and go to step 3. After 3 attempts the append fails with `Stale`.
6. Return. There is no `fsync`.

A record is acknowledged by its last successful data-bearing `write(2)`. The earlier ones, if any, wrote into files that are no longer the active file, and those copies are what the readers and retention collapse. An append that fails with `Stale` is counted as dropped, yet its copies sit in generations and the record may still be read. That only adds evidence.

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

The writer supports one place for the journal: a local APFS volume. `statfs(2)` on the journal directory must report `f_fstypename` equal to `apfs` and `MNT_LOCAL` set in `f_flags`. On any other volume the writer opens nothing, writes nothing and records the reason in hook health: a network volume breaks the premise of an atomic append, SMB is unproven, and a local volume that is not APFS was not tested.

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

One algorithm, stated here once. [maintenance.rs](../../crates/agentdust-bench/src/maintenance.rs) implements it for the benchmark, and the retention task ports it into `agentdust-core`.

Retention takes the exclusive lock on `journal.maint`, which only rotation and retention take. Then, for each closed generation `journal.<stamp>.jsonl`, oldest stamp first, and never for the active file:

1. Decode it. When any line is of a newer schema version, leave the generation as it is.
2. Go through its lines in file order. A record the retention rules drop is dropped. A record whose bytes equal those of a record kept earlier in this run is dropped as a duplicate. A line of an unknown kind is kept as it is. A line that is torn or does not parse is dropped, and by itself never causes a rewrite.
3. When nothing was dropped, leave the generation as it is.
4. When nothing is kept, delete the generation.
5. Otherwise write the kept lines, in their original order and with their original bytes, to `journal.compact.tmp` (created exclusively, mode 0600), sync it, rename it over the generation and sync the directory.

The retention rules themselves stay in spec 3.2: earlier boots first, ended sessions after 14 days or past 20 MB, active sessions pinned. The algorithm only fixes how a rule is carried out. A pinned record is simply a kept record. It stays in its generation, in its place.

- **No record moves.** A record is never moved to another file and never appended again. A reader holds a set of open files, and a record that moved between them could be in none of the files that reader holds. Replacing a generation under its own name keeps every record in a file that a reader either has or will list.
- **No grace window.** No step depends on elapsed time. C2 needs none, because the recheck, and not a delay, protects a late writer. A grace window protects only a writer paused for less than its length, and the interleaving tests show C losing the record beyond it. The benchmark's `grace_ms` is an argument that tests set to a number, and the value to ship is 0.
- **Generations stay separate.** Merging them would shorten the list but rewrite every older generation on each pass. Rotation is rare, so the list stays short.

Open: whether a damaged generation is copied aside before it is rewritten. That is the corrupt-state recovery task's decision, and this algorithm drops what it cannot parse only when it rewrites a generation for another reason.

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

The cap says nothing about a real `sample` record. The `procs` list of spec 3.2 has no length limit. The M2 sampler needs a byte budget below the cap and a truncation marker, `procs_total` and `procs_recorded`, so a truncated list is visible as one. Until it exists, a `sample` record over the cap is dropped and counted in hook health.

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
- The rotation stress in `journal_append_rotation.rs` has no retention in it, so it does not fail when the recheck is removed. The interleaving tests do.
- No network volume was available.
- Records are padded in `session_id`. A real `sample` record is not measured.
- A stopped rotator and a stopped appender were tested for refusal and not timed.
- The benchmark and the `agentdust-core` tests have not run on Linux. The crates compile for `x86_64-unknown-linux-gnu` with `cargo check --all-targets`.
- "No observed losses" means none in 144,000 attempted appends per candidate in three runs. It is not a proof, and the rotation stress used a 1 ms period that no hook workload reaches.

## Spec edits to propose

Section 3.2, storage paragraph. Replace:

```text
Storage until the M1 contention benchmark decides otherwise: `journal.jsonl` plus a separate `journal.lock` taken with `flock` by every reader, writer and rotator before opening the data file. Rotation never truncates the active file. A truncated last line is ignored and counted in hook health.
```

with:

```text
Storage: `journal.jsonl` and closed generations `journal.<stamp>.jsonl`, in a directory on a local APFS volume (section 7.3). A record is one frame: the byte 0x1E, the record as compact JSON, the byte 0x0A. An append opens `journal.jsonl` with `O_APPEND` and `O_NOFOLLOW`, writes the frame with one `write(2)` and compares `fstat` of the descriptor with `lstat` of the path. When the path names another file, or none, the frame is written again, up to 3 attempts in all. An appender takes no lock. A call interrupted by a signal before it transferred anything may be restarted. A write that returns fewer bytes than the frame is an error and is never completed by a second call. A frame longer than 65,536 bytes is dropped and counted in hook health. Appends are not synced: an acknowledged record can be lost on an OS crash or a power failure, and lost evidence only lowers confidence (S19). `journal.maint`, taken with `flock` by rotation and retention only, keeps two maintenance runs apart. Rotation renames `journal.jsonl` to `journal.<stamp>.jsonl`, where `<stamp>` is the wall clock in milliseconds moved up to the next free number, and never truncates or rewrites the active file. Readers take no lock. A reader opens `journal.jsonl`, lists the generations, opens each one that still exists, collapses records that are equal in every field, and orders the rest by the wall time at which their boot first appears, then `mono_ts`, then the remaining fields in a fixed order. This order is for presentation. It does not show which event caused which, and the order of boots is weak because wall time can move backwards. A frame without its closing 0x0A, a line that does not parse and a line of an unknown kind are skipped and counted. A line of a newer schema version is counted apart, including one over the size limit, which is recognised by its leading `{"v":`.
```

Section 3.2, retention paragraph. After "records of active sessions are pinned." add:

```text
Retention visits each closed generation, oldest first, and never the active file. A generation that holds a line of a newer schema version is left as it is. Otherwise the records the rules above keep are written, as they were, to `journal.compact.tmp`, which is synced and renamed over the generation, and the directory is synced. A generation with no kept record is deleted, and one with nothing to drop is left as it is. A record is never moved to another file and never appended again. An exact duplicate of a record kept earlier in the same run is dropped. No step depends on elapsed time.
```

Section 4.4, journal write budget. Replace:

```text
- Journal write budget: the lock is taken without blocking, with retries for at most 20 ms. On failure the record is dropped and hook health is updated. The health update follows the same budget and is best effort.
```

with:

```text
- Journal write budget: an append is a few system calls and takes no lock, so it waits for no other process. The record is dropped and hook health is updated when the frame is longer than 65,536 bytes, when the volume is not a local APFS volume, when a write returns fewer bytes than the frame, or when the active file was replaced in 3 attempts in a row. The health update is best effort.
```

Section 4.4, health record. Replace "updated under the journal lock" with "kept in `health.json`, which is replaced by writing a temporary file and renaming it over the old one. The last writer wins, so a count can miss an update".

Section 7.2, table row. Replace:

```text
| `journal.jsonl`, `journal.lock` | section 3.2 fields only | section 3.2 |
```

with:

```text
| `journal.jsonl`, `journal.<stamp>.jsonl`, `journal.compact.tmp`, `journal.maint` | section 3.2 fields only. `journal.maint` is empty and only rotation and retention take it | section 3.2 |
```

Section 7.3, after the paragraph on the data directory. Add:

```text
The journal directory must be on a local APFS volume: `statfs` reports `f_fstypename` equal to `apfs` and `MNT_LOCAL` in `f_flags`. On any other volume the hook writes nothing and records `unsupported_filesystem` and the file system name in hook health. `agentdust status` prints the file system name, whether it is local and whether it is supported.
```

Section 9.1. Add after S21, and extend S18:

```text
| S22 | A journal record is written by one write call per attempt and a frame is never longer than 65,536 bytes. Concurrent appenders never tear or interleave a line, and a cut write costs one record | performance: 16 writer processes with 4,000 byte records, zero torn or interleaved lines. Byte level cuts after 0, 1, 2, half, all but 2 and all but 1 bytes. An oversized record is dropped and counted |
| S23 | An acknowledged record is present exactly once after any interleaving of an append with rotation, a read and retention | deterministic interleavings with pause points after the open, before the write and after the write, a reader paused after its open and after its listing, retention deleting and compacting. Rotation stress with a rotator, a reader and 16 writers: zero lost acknowledged records |
| S24 | The journal writes nothing on a volume that is not a local APFS volume | injected `statfs` results for apfs local, apfs not local, hfs, devfs, nfs, smbfs and autofs. The status value is printed |
```

S18, tests column: append "A journal line of a future version longer than 65,536 bytes is counted as a newer version, and retention leaves its generation as it is".

Section 9.2, stress row. Replace "three concurrent journal writers, rotation under load, PID churn" with "3 and 16 concurrent journal writer processes with a rotator, a reader and retention running, PID churn".

Section 10, journal format row. Replace "M1 contention benchmark" with "M1 contention benchmark: one `O_APPEND` file, no lock for appenders and readers, an append recheck (docs/m1/adr-journal-format.md)".

ROADMAP: remove open decision 1, and add a row to the decisions table: `| Journal | One O_APPEND file framed with RS, no lock for appenders and readers, an append recheck, 64 KiB frame cap, local APFS only |`. In the M1 bullet "Locking and the three-writer benchmark that picks the ledger format", replace "Locking and the three-writer benchmark" with "The 3 and 16 writer benchmark".

## What the remaining journal tasks must change

Grouped by concern.

**Journal store** (`agentdust_core::journal`) is built: the append of steps 1 to 6, the fault injection writer, the volume check and the status value, the reader of "Reading" and "Ordering", and the framing with its decoder. One item is left:

- Point the benchmark's C2 at `Journal::append`, so that the 16 writer test (S22) runs against the shipped append. The benchmark keeps its own frozen copy of the append until then.

**Rotation, retention and recovery:**

- Implement "Retention" as stated, with the spec's keep rules as the predicate and `journal.maint` for exclusion.
- Decide whether a damaged generation is copied aside before it is rewritten.
- Carry `degraded` for a session whose pinned evidence was dropped.

**Hook write path and health** (spec 4.4):

- Count each kind of drop in hook health: too large, a short write, `Stale`, an unsupported volume, any other error.
- Write health to a temporary file and rename it, one file per agent, with no lock.
- Keep the 16 writer test (S22) and the interleaving tests (S23) in the Linux and the macOS job. They are `journal_append.rs`, `journal_append_recheck.rs` and `journal_snapshot.rs` in `agentdust-core`, and they are platform independent.

## Open questions

- **Is 64 KiB the right cap for a real `sample`?** It rests on latency. The M2 sampler decides the byte budget.
- **Should `fsync(2)` be added?** It costs 0.044 ms at p50 and promises survival of an OS crash only. The contract promises neither today.
- **Does the benchmark pass on Linux?** The tests are platform independent and have not run there.
