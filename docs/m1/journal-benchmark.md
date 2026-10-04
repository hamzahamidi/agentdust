# Journal contention benchmark

This page records the M1 benchmark behind [ADR-1](adr-journal-format.md) (ROADMAP open decision 1, spec sections 3.2 and 10, safety requirement S13, section 9.2 Stress). Four candidates were measured with real writer processes, a reader process and a rotator process. A, C2 and D lost no acknowledged record in these tests. C lost 115 acknowledged records in the 144,000 appends of the three runs, and each run had a rotator running.

- **Decision and spec edits:** [adr-journal-format.md](adr-journal-format.md)
- **Code:** `crates/agentdust-bench`, measured at `ae7ee1b`
- **Raw results:** [journal-benchmark-data/](journal-benchmark-data/). `journal-bench render <file>` prints the tables of a run from its JSON, and the data files together hold 653 lines.

## Candidates

| Label | Files | Append | Read | Rotation and retention |
| --- | --- | --- | --- | --- |
| A | `journal.jsonl`, `journal.lock` | The M0 design. Exclusive `flock` on `journal.lock`, non-blocking and retried for 20 ms, then the open and one `write` of the line. A busy lock drops the record. The line has no RS prefix | Shared `flock` on `journal.lock` for the whole read | Exclusive `flock` on `journal.lock` for the whole run |
| B | `segments/<pid>-<ns>-<n>.jsonl` | Not in this tree. See [Measurements carried over](#measurements-carried-over) | | |
| C | `journal.jsonl`, `journal.maint` | `O_APPEND`, one `write(2)` of RS, compact JSON and LF, no lock | No lock | Exclusive `flock` on `journal.maint` per run, nothing else |
| C2 | `journal.jsonl`, `journal.maint` | As C, then `fstat` of the descriptor against `lstat` of the path. When the path names another inode or none, the frame is written again, at most 3 attempts in all | No lock | As C |
| D | `journal.jsonl`, `journal.lock`, `journal.maint` | Shared `flock` on `journal.lock`, non-blocking and retried for 20 ms, held from before the open to after the write, then as C | Shared `flock` on `journal.lock` while the files are listed and opened | As C, plus an exclusive `flock` on `journal.lock` for each rename or unlink that changes the set of generations |

Every file is created with mode 0600 under a 0700 directory and opened with `O_NOFOLLOW`, so no candidate pays for a safeguard another skips. All four candidates share one reader (open the active file, list the generations, open each, skip one that vanished, collapse exact duplicates, order by boot and `mono_ts`), one rotation (rename of `journal.jsonl` to `journal.<stamp>.jsonl`) and one retention algorithm (ADR-1, section Retention). They differ in the append and in which locks the reader and the maintenance take. A refuses a frame of more than 65,536 bytes like the others, which the M0 code did not.

SQLite was not measured. The ADR gives the reasons.

## Deterministic interleavings

`crates/agentdust-bench/tests/interleavings.rs` runs the real implementation with a pause point after the open, before the write and after the write, plus two pause points inside the reader. A test pauses a writer thread at one point, runs the rotator, a read or a retention pass on the test thread, then resumes the writer and checks the result. Time is passed in as an argument, so a grace window is a number and no test sleeps or asserts a duration. Where a candidate cannot do a step (a rotator refused by a lock a paused appender holds), the test asserts that refusal.

Rows are scenarios, and each cell is what the test asserts about the paused writer's record after the writer resumes.

| Scenario | A | C | C2 | D |
| --- | --- | --- | --- | --- |
| Pause after the open, rotate, resume, read, rotate again, retain | Rotation refused (busy) while paused. Present once | Rotation done. Present once | Rotation done. Written again, 2 attempts, one duplicate collapsed on read. Present once, and once on disk after retention | Rotation refused (busy) while paused. Present once |
| Pause after the open, rotate and compact the old generation, resume | Rotation and retention refused. Present once | **Lost.** The append returned success | Present once, 2 attempts | Rotation refused. Present once |
| The same, and the old generation holds only records retention drops, so it is deleted | Present once | **Lost.** The append returned success | Present once | Present once |
| The same, but retention runs one millisecond inside the grace window | Present once | Present once | Present once | Present once |
| Pause after the write, rotate, compact, resume | Present once | Present once | Written again, one duplicate collapsed. Present once | Present once |
| A probe unlinks the active file after the write | not run | **Lost.** The append returned success | Present once, 2 attempts | not run |
| A probe rotates after every write | not run | not run | The third attempt fails with `Stale`. The record sits in three generations and is collapsed to one on read | not run |
| A reader paused after opening the active file, or after listing, while a rotation and a compaction run | Rotation refused while the reader holds the lock. Every acknowledged record once | Every acknowledged record once | Every acknowledged record once | Rotation refused while the reader holds the lock. Every acknowledged record once |
| A generation retention deletes between the listing and the open | Retention refused. No error, every record once | Skipped, no error, every record once | Skipped, no error, every record once | Retention refused. No error, every record once |

The read during the pause is skipped for A in the first row, because its reader waits for the lock that the paused appender holds.

C is the one candidate that loses a record, and only when the writer's pause outlasts the grace window and a compaction or deletion has already replaced or unlinked the generation the writer holds a descriptor to. The test asserts the loss, so a change in that behaviour fails it. The measured runs below show the same loss with real processes.

## Method

- **Real processes.** `journal-bench` spawns itself. Each writer, the reader and the rotator print `ready`, wait for one `go` line and then run. A process whose stdin closes before `go` exits without touching the journal.
- **Writers.** 3 and 16 processes append back to back. Every append opens the files again, as a hook does. A record is a journal record whose line is 150 or 4,000 bytes including its newline. A writes that line, and C, C2 and D write it with an RS in front, one byte more. The writer and the sequence number are in `session_id` (`w<writer>-<seq>-<padding>`), with a padding derived from both. The 4,000 byte size is chosen to stand for a `sample` record that carries a `procs` list.
- **No pacing.** A run is 2,400 appends (800 per writer at 3 writers, 150 at 16) and lasts 0.03 to 2.5 s. Every cell is a saturation burst.
- **A rotator process.** Every cycle it appends a marker record (`expired-<n>`) through the candidate, rotates, and runs retention with a keep rule that drops markers. Every generation therefore holds a droppable record, so every retention pass rewrites or deletes. When the writers have finished, the coordinator creates a stop file and the rotator runs one last cycle with grace 0. That cycle compacts every generation, so the final counts do not depend on how many cycles the machine had time for, and a surviving marker would show as a defect.
- **Rotator timing.** The period between cycles is 10 ms (1 ms in the stress runs). The wait for a rotation is capped at 50 ms (a rotation that gives up is counted as skipped), and the final cycle waits up to 10 s. Retention runs with grace 0, the worst case for every candidate.
- **One reader.** A separate process reads the whole journal every 20 ms until the writers finish.
- **Latency.** The wall time of the append call, read with `Instant` inside the writer. Process start-up is not included. A hook's total is its start-up (M0: p50 2.50 to 2.68 ms, p95 2.85 to 3.36 ms) plus this.
- **Checks.** After the final cycle one process reads the journal and regenerates each record from its writer and sequence number.
- **Repeats.** Each cell runs 5 times, round robin across cells, so a change in machine load reaches every candidate. Percentile columns are the median of the 5 per-run values with the range in parentheses. Count columns are totals over the 5 runs (12,000 appends per cell, 48,000 per candidate).
- **Reader cost.** One process writes 10,000 records and rotates after every quarter, so the store is three generations and an active file. Then `read_all` is timed 5 times with a warm page cache.

Column definitions:

- **Dropped:** the append returned an error and the writer knows it. For A and D, a lock was still busy after 20 ms. For C2, 3 attempts found a replaced file.
- **Lost:** the append returned success and the record is missing from the final read.
- **Torn:** a line that does not parse as a record, a frame without its newline, a line of a newer version or a line of an unknown kind.
- **Interleaved:** a line that parses but is not the record the writer wrote.
- **Out of order:** one writer's sequence number goes backwards in read order.
- **Duplicates:** records the reader or a writer repeated. The final cycle's retention removes the copies a re-append leaves, so this column is 0 after the run.
- **Unacknowledged but present:** a record that is in the journal although its append returned an error.
- **Timestamp inversions:** adjacent records in read order where `mono_ts` goes backwards. Every candidate reads through the same sort, so the column is 0 by construction and only checks the sort.
- **Re-appended records:** appends that needed a second attempt (C2).
- **Rotations skipped, compactions skipped:** the rotator could not take a lock within its 50 ms budget.
- **Rotation wait:** the time of the rotation call, from the first lock attempt to the return. It includes the rename and, with syncs on, the directory sync.

## Machine

- **Hardware:** MacBook Pro `MacBookPro18,4`, Apple M1 Max, 10 cores, 32 GB.
- **Software:** macOS 26.6.2 (Darwin 25.6.0), a release build with rustc 1.99.0.
- **Disk:** the data directory is under `$TMPDIR`, which `journal-bench fs` reports as `apfs, local, supported`.
- **Load:** the Mac is shared, and the 1 minute load average was 25 to 45 during every run below, which inflates every absolute latency. Each result carries its load averages (1, 5 and 15 minutes). The candidates are compared inside one run, round robin, so they share that load.

## Results

Run 1 has the maintenance syncs on, as the shipped code would run them: rotation and retention call `File::sync_all` on the replacement file and on the directory.

- Command: `journal-bench run --json docs/m1/journal-benchmark-data/run-1.json`
- Load averages before the run: 24.66 42.82 46.57
- Load averages after the run: 27.74 40.89 45.65
- Run time: 43.8 s. Repeats per cell: 5.

### Append latency under contention and rotation

| Candidate | Writers | Record bytes | p50 ms | p95 ms | p99 ms | max ms | Dropped | Lost | Torn or interleaved | Files |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A | 3 | 150 | 0.053 (0.044 to 0.069) | 0.368 (0.094 to 5.002) | 19.482 (7.623 to 21.591) | 21.614 (21.248 to 47.734) | 101 | 0 | 0 | 14 |
| C | 3 | 150 | 0.050 (0.049 to 0.054) | 0.607 (0.118 to 0.824) | 4.062 (2.529 to 4.335) | 6.937 (4.244 to 13.787) | 0 | 0 | 0 | 8 |
| C2 | 3 | 150 | 0.054 (0.049 to 0.076) | 0.611 (0.110 to 0.834) | 3.856 (2.644 to 4.019) | 5.082 (4.160 to 16.947) | 0 | 0 | 0 | 8 |
| D | 3 | 150 | 0.081 (0.062 to 0.097) | 0.520 (0.118 to 0.723) | 3.686 (0.454 to 4.709) | 5.826 (4.462 to 13.714) | 0 | 0 | 0 | 7 |
| A | 16 | 150 | 0.072 (0.047 to 0.078) | 20.366 (8.886 to 20.755) | 21.233 (21.188 to 21.953) | 23.066 (21.911 to 25.147) | 602 | 0 | 0 | 15 |
| C | 16 | 150 | 0.051 (0.045 to 0.063) | 2.084 (1.234 to 2.921) | 5.659 (3.842 to 6.351) | 9.809 (5.762 to 16.844) | 0 | 1 | 0 | 4 |
| C2 | 16 | 150 | 0.058 (0.052 to 0.071) | 2.367 (1.516 to 3.046) | 6.567 (4.735 to 7.748) | 16.218 (8.985 to 30.975) | 0 | 0 | 0 | 4 |
| D | 16 | 150 | 0.098 (0.073 to 0.122) | 1.428 (1.144 to 1.674) | 5.213 (2.498 to 7.318) | 18.508 (5.442 to 22.995) | 0 | 0 | 0 | 3 |
| A | 3 | 4000 | 0.105 (0.073 to 0.137) | 20.493 (1.417 to 20.810) | 21.750 (20.213 to 22.365) | 39.972 (21.275 to 53.426) | 655 | 0 | 0 | 31 |
| C | 3 | 4000 | 0.115 (0.057 to 0.151) | 0.835 (0.411 to 1.312) | 4.341 (3.679 to 5.022) | 6.793 (4.749 to 23.699) | 0 | 0 | 0 | 8 |
| C2 | 3 | 4000 | 0.097 (0.058 to 0.157) | 0.729 (0.385 to 1.427) | 4.479 (3.581 to 5.167) | 14.230 (4.620 to 75.369) | 0 | 0 | 0 | 9 |
| D | 3 | 4000 | 0.123 (0.102 to 0.149) | 0.502 (0.215 to 0.872) | 1.600 (0.306 to 5.242) | 38.925 (0.924 to 109.800) | 0 | 0 | 0 | 5 |
| A | 16 | 4000 | 0.212 (0.084 to 0.513) | 21.241 (20.968 to 21.422) | 21.595 (21.269 to 23.869) | 24.810 (22.727 to 30.916) | 2309 | 0 | 0 | 21 |
| C | 16 | 4000 | 0.072 (0.070 to 0.198) | 3.371 (1.381 to 3.988) | 7.204 (3.990 to 9.258) | 15.807 (8.064 to 31.575) | 0 | 0 | 0 | 5 |
| C2 | 16 | 4000 | 0.100 (0.095 to 0.216) | 2.288 (1.233 to 4.119) | 6.445 (4.389 to 8.828) | 12.850 (5.895 to 23.895) | 0 | 0 | 0 | 4 |
| D | 16 | 4000 | 0.111 (0.086 to 0.161) | 1.438 (0.980 to 1.885) | 4.657 (1.558 to 13.301) | 12.569 (5.352 to 36.635) | 0 | 0 | 0 | 3 |

### Consistency and throughput

| Candidate | Writers | Record bytes | Appends per second | Out of order | Duplicates | Unacknowledged but present | Timestamp inversions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A | 3 | 150 | 6270 | 0 | 0 | 0 | 0 |
| C | 3 | 150 | 14150 | 0 | 0 | 0 | 0 |
| C2 | 3 | 150 | 13802 | 0 | 0 | 0 | 0 |
| D | 3 | 150 | 12710 | 0 | 0 | 0 | 0 |
| A | 16 | 150 | 4947 | 0 | 0 | 0 | 0 |
| C | 16 | 150 | 34960 | 0 | 0 | 0 | 0 |
| C2 | 16 | 150 | 34336 | 0 | 0 | 0 | 0 |
| D | 16 | 150 | 37976 | 0 | 0 | 0 | 0 |
| A | 3 | 4000 | 1303 | 0 | 0 | 0 | 0 |
| C | 3 | 4000 | 8755 | 0 | 0 | 0 | 0 |
| C2 | 3 | 4000 | 8966 | 0 | 0 | 0 | 0 |
| D | 3 | 4000 | 8720 | 0 | 0 | 0 | 0 |
| A | 16 | 4000 | 1633 | 0 | 0 | 0 | 0 |
| C | 16 | 4000 | 23927 | 0 | 0 | 0 | 0 |
| C2 | 16 | 4000 | 22537 | 0 | 0 | 0 | 0 |
| D | 16 | 4000 | 30801 | 0 | 0 | 0 | 0 |

### Rotator running during the appends

| Candidate | Writers | Record bytes | Rotations | Rotations skipped | Median rotation wait ms | Max rotation wait ms | Compactions | Compactions skipped | Re-appended records |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A | 3 | 150 | 76 | 0 | 4.420 | 21.447 | 73 | 0 | 0 |
| C | 3 | 150 | 34 | 0 | 4.781 | 9.010 | 34 | 0 | 0 |
| C2 | 3 | 150 | 34 | 0 | 4.739 | 6.927 | 34 | 0 | 47 |
| D | 3 | 150 | 24 | 0 | 9.442 | 42.154 | 24 | 0 | 0 |
| A | 16 | 150 | 65 | 0 | 4.594 | 10.316 | 57 | 0 | 0 |
| C | 16 | 150 | 17 | 0 | 4.728 | 7.501 | 17 | 0 | 0 |
| C2 | 16 | 150 | 17 | 0 | 5.086 | 8.256 | 17 | 0 | 117 |
| D | 16 | 150 | 8 | 3 | 5.431 | 50.579 | 8 | 0 | 0 |
| A | 3 | 4000 | 148 | 1 | 4.985 | 50.329 | 140 | 0 | 0 |
| C | 3 | 4000 | 35 | 0 | 6.039 | 15.264 | 35 | 0 | 0 |
| C2 | 3 | 4000 | 38 | 0 | 5.450 | 31.350 | 38 | 0 | 55 |
| D | 3 | 4000 | 15 | 6 | 16.081 | 53.872 | 15 | 2 | 0 |
| A | 16 | 4000 | 106 | 0 | 4.744 | 24.200 | 94 | 0 | 0 |
| C | 16 | 4000 | 19 | 0 | 5.022 | 10.291 | 19 | 0 | 0 |
| C2 | 16 | 4000 | 16 | 0 | 4.829 | 7.418 | 16 | 0 | 113 |
| D | 16 | 4000 | 9 | 5 | 12.176 | 54.648 | 9 | 0 | 0 |

### Reader running during the appends

| Candidate | Writers | Record bytes | Reads | Read ms | Most torn tail lines seen |
| --- | ---: | ---: | ---: | ---: | ---: |
| A | 3 | 150 | 15 | 3.686 | 0 |
| C | 3 | 150 | 8 | 1.484 | 0 |
| C2 | 3 | 150 | 8 | 1.054 | 0 |
| D | 3 | 150 | 9 | 1.344 | 0 |
| A | 16 | 150 | 18 | 2.892 | 0 |
| C | 16 | 150 | 4 | 1.188 | 0 |
| C2 | 16 | 150 | 4 | 1.193 | 0 |
| D | 16 | 150 | 4 | 1.242 | 0 |
| A | 3 | 4000 | 30 | 16.597 | 0 |
| C | 3 | 4000 | 8 | 5.532 | 0 |
| C2 | 3 | 4000 | 8 | 5.669 | 0 |
| D | 3 | 4000 | 7 | 7.187 | 0 |
| A | 16 | 4000 | 23 | 10.888 | 0 |
| C | 16 | 4000 | 3 | 14.330 | 0 |
| C2 | 16 | 4000 | 4 | 9.501 | 0 |
| D | 16 | 4000 | 3 | 7.934 | 0 |

### Reader cost on a prepared store

| Candidate | Record bytes | Records | Files | Bytes on disk | Read ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| A | 150 | 10000 | 5 | 1500000 | 4.341 (4.244 to 5.100) |
| C | 150 | 10000 | 5 | 1510000 | 5.145 (4.707 to 7.389) |
| C2 | 150 | 10000 | 5 | 1510000 | 4.470 (4.232 to 4.770) |
| D | 150 | 10000 | 6 | 1510000 | 4.839 (4.723 to 5.249) |
| A | 4000 | 10000 | 5 | 40000000 | 40.026 (35.831 to 58.432) |
| C | 4000 | 10000 | 5 | 40010000 | 36.121 (35.003 to 36.588) |
| C2 | 4000 | 10000 | 5 | 40010000 | 38.002 (37.657 to 42.445) |
| D | 4000 | 10000 | 6 | 40010000 | 40.366 (34.717 to 46.932) |

Run 1 ends with these counts, per candidate, over 48,000 appends each.

- **A** dropped 3,667 records (7.6%). Its lock is held by every appender, for the whole of every read and for the whole of every rotation and retention pass, and a writer that finds it held for 20 ms drops its record. The run does not apportion the drops between those holders.
- **C** lost 1 acknowledged record and dropped none.
- **C2** lost none, dropped none and wrote 332 records a second time.
- **D** lost none and dropped none. 14 of its 70 rotation attempts gave up at the 50 ms budget.

## Rotation stress

With the syncs on, the median rotation of A, C and C2 takes 4.4 to 6.0 ms in a cell, and a short run rotates only a few times. The stress runs turn the syncs off (`--sync off`), which brings that median to 0.19 to 0.61 ms, and set the period between cycles to 1 ms. That gives 10 to 419 rotations per cell and 137 to 1,134 per candidate and run. The reader cost phase is skipped (`--reader-records 0`).

- Command: `journal-bench run --sync off --rotate-period-ms 1 --reader-records 0 --json <file>`
- Run 1: [stress-sync-off.json](journal-benchmark-data/stress-sync-off.json), 35.1 s, load before 28.02 40.52 45.46 and after 35.25 40.91 45.40
- Run 2: [stress-sync-off-run-2.json](journal-benchmark-data/stress-sync-off-run-2.json), 34.2 s, load before 37.66 40.80 45.13 and after 44.98 42.19 45.46

Counts per cell, 12,000 appends each:

| Candidate | Writers | Record bytes | Dropped, run 1 | Dropped, run 2 | Lost, run 1 | Lost, run 2 | Re-appended, run 1 | Re-appended, run 2 | Rotations, run 1 | Rotations, run 2 | Skipped, run 1 | Skipped, run 2 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A | 3 | 150 | 37 | 51 | 0 | 0 | 0 | 0 | 287 | 322 | 0 | 0 |
| C | 3 | 150 | 0 | 0 | 9 | 5 | 0 | 0 | 119 | 131 | 0 | 0 |
| C2 | 3 | 150 | 0 | 0 | 0 | 0 | 188 | 176 | 129 | 136 | 0 | 0 |
| D | 3 | 150 | 0 | 0 | 0 | 0 | 0 | 0 | 86 | 103 | 1 | 0 |
| A | 16 | 150 | 383 | 355 | 0 | 0 | 0 | 0 | 217 | 207 | 0 | 0 |
| C | 16 | 150 | 0 | 0 | 36 | 46 | 0 | 0 | 46 | 41 | 0 | 0 |
| C2 | 16 | 150 | 0 | 0 | 0 | 0 | 190 | 245 | 50 | 49 | 0 | 0 |
| D | 16 | 150 | 0 | 0 | 0 | 0 | 0 | 0 | 14 | 12 | 4 | 4 |
| A | 3 | 4000 | 632 | 750 | 0 | 0 | 0 | 0 | 402 | 419 | 0 | 1 |
| C | 3 | 4000 | 0 | 0 | 4 | 1 | 0 | 0 | 75 | 73 | 0 | 0 |
| C2 | 3 | 4000 | 0 | 0 | 0 | 0 | 139 | 108 | 77 | 69 | 0 | 0 |
| D | 3 | 4000 | 0 | 0 | 0 | 0 | 0 | 0 | 27 | 25 | 2 | 4 |
| A | 16 | 4000 | 2019 | 1850 | 0 | 0 | 0 | 0 | 215 | 186 | 1 | 1 |
| C | 16 | 4000 | 0 | 0 | 9 | 4 | 0 | 0 | 24 | 23 | 0 | 0 |
| C2 | 16 | 4000 | 0 | 0 | 0 | 0 | 119 | 122 | 22 | 23 | 0 | 0 |
| D | 16 | 4000 | 0 | 0 | 0 | 0 | 0 | 0 | 10 | 10 | 5 | 5 |

Totals per candidate for all three runs, 48,000 appends per candidate and run:

| Run | Candidate | Appends | Dropped | Lost | Re-appended | Rotations | Rotations skipped |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Run 1, syncs on | A | 48000 | 3667 | 0 | 0 | 395 | 1 |
| Run 1, syncs on | C | 48000 | 0 | 1 | 0 | 105 | 0 |
| Run 1, syncs on | C2 | 48000 | 0 | 0 | 332 | 105 | 0 |
| Run 1, syncs on | D | 48000 | 0 | 0 | 0 | 56 | 14 |
| Stress run 1, syncs off | A | 48000 | 3071 | 0 | 0 | 1121 | 1 |
| Stress run 1, syncs off | C | 48000 | 0 | 58 | 0 | 264 | 0 |
| Stress run 1, syncs off | C2 | 48000 | 0 | 0 | 636 | 278 | 0 |
| Stress run 1, syncs off | D | 48000 | 0 | 0 | 0 | 137 | 12 |
| Stress run 2, syncs off | A | 48000 | 3006 | 0 | 0 | 1134 | 2 |
| Stress run 2, syncs off | C | 48000 | 0 | 56 | 0 | 268 | 0 |
| Stress run 2, syncs off | C2 | 48000 | 0 | 0 | 651 | 277 | 0 |
| Stress run 2, syncs off | D | 48000 | 0 | 0 | 0 | 150 | 13 |

- **Lost records.** C lost 58 and 56 acknowledged records in the two stress runs, 0.12% each, and 1 in run 1. A, C2 and D lost none in any run. The largest loss is in the cell of 16 writers and 150 bytes, 36 and 46 records.
- **Dropped records.** A drops 3,006 to 3,667 records per run (6.3% to 7.6%). C, C2 and D dropped none. No append failed with `Stale`, so C2 never ran out of its 3 attempts.
- **Re-appends.** C2 wrote a record a second time 332, 636 and 651 times, 0.7% to 1.4% of its appends. The final read collapsed no duplicate, because retention in the final cycle had removed them.
- **Rotation under D.** A rotation attempt gave up at the 50 ms budget 14 of 70 times in run 1 and 12 of 149 and 13 of 163 times in the stress runs. The median rotation wait of D is 0.58 to 20 ms per cell in the stress runs and 5.4 to 16.1 ms in run 1. C and C2 never skipped a rotation, and their median wait is 0.22 to 0.61 ms per cell in the stress runs. The longest single rotation wait was 59.8 ms for D, 31.4 ms for C2, 24.2 ms for C and 77.6 ms for A.
- **p50 of the append.** C and C2 differ by at most 0.03 ms in the four cells of run 1 (0.050 against 0.054, 0.051 against 0.058, 0.115 against 0.097, 0.072 against 0.100 ms). D is 0.008 to 0.047 ms above C in all four.

## Cost of syncing an append

`journal-bench durability` writes one 150 byte record at a time and times the write together with its sync. `fsync` is `fsync(2)`, and `full` is `File::sync_all`. Its cost is 90 times that of `fsync(2)`, which fits the standard library using `F_FULLFSYNC` on macOS, the only call that asks the drive to flush its cache. That was not read from the library source.

- Command: `journal-bench durability --samples 200 --json docs/m1/journal-benchmark-data/durability.json`
- Load averages before and after the run: 36.48 40.80 45.24
- Run time: 0.9 s

| Mode | Samples | Record bytes | p50 ms | p95 ms | p99 ms | max ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| none | 200 | 150 | 0.002 | 0.004 | 0.009 | 0.029 |
| fsync | 200 | 150 | 0.044 | 0.093 | 0.123 | 0.162 |
| full | 200 | 150 | 4.000 | 5.918 | 7.228 | 19.954 |

The hook has p50 under 10 ms and p95 under 20 ms to keep (spec 4.4), and a tool call writes at least two records, one before and one after. The full flush alone is 4.0 ms at p50 for each record.

## File systems

`journal-bench fs <path>` prints the result of `statfs(2)` for a path: the file system name (`f_fstypename`) and whether `MNT_LOCAL` is set in `f_flags`. A journal directory is supported when the name is `apfs` and the flag is set.

| Path | Result |
| --- | --- |
| `$TMPDIR` | apfs, local, supported |
| `$HOME` | apfs, local, supported |
| `/private/tmp` | apfs, local, supported |
| `/System/Volumes/VM` | apfs, local, supported |
| `/dev` | devfs, local, not supported |
| `/System/Volumes/Data/home` | autofs, not local, not supported |

No NFS, SMB or other network volume was available, so none was measured. The policy treats them as unsupported because their `f_flags` lack `MNT_LOCAL`, and that is a statement about the flag and not about a run.

## Measurements carried over

B (one segment file per writer handle, merged on read by boot and `mono_ts`) is not in this tree, and its numbers are not repeated here. Adding rotation and retention to a design with one file per hook record was not cheap, and the measurement that decided against it does not depend on rotation. Its numbers come from the benchmark that preceded this one (branch `m1-proto`, commit `5e95b28`, 5 repeats, no rotator, one reader, the same hardware and writers, load averages 6.8 to 10.3). They were not run again here.

| Measure | A (flock) | B (segment files) | C (O_APPEND) |
| --- | ---: | ---: | ---: |
| Files after 2,400 hook appends | 2 | 1,601 to 1,655 | 1 |
| p99 append ms, 16 writers, 150 and 4000 bytes | 20.690 and 21.136 | 2.273 and 2.233 | 1.165 and 1.081 |
| Read of 10,000 records, 150 and 4000 bytes, ms | 2.751 and 17.427 | 215.382 and 230.615 | 2.692 and 17.418 |
| Lost, torn or interleaved lines | 0 | 0 | 0 |

The same earlier benchmark appended records of 16 KiB to 4 MiB with 16 writers to C alone, 5 runs per size, and found no lost, torn or interleaved record at any size. The p99 of the append was 3.166 ms at 64 KiB, 5.041 ms at 256 KiB and 33.954 ms at 4 MiB. The 64 KiB cap in the ADR rests on that latency and on nothing measured here: this run only appends 150 and 4,000 byte records.

## Limits

What the benchmark does not do:

- It does not pace writers. A real hook fires far less often than 2,400 times in 100 ms, so contention in use is lower than in any cell here.
- It times the append, not the process start-up around it.
- It reads with a warm page cache. A cold cache needs privileges the run does not have.
- It runs on one machine, one local APFS volume and one macOS version, under a 1 minute load average of 25 to 45.
- It does not kill a writer in the middle of a write, fill the disk or pull the power. The short write cases are tested on bytes (`tests/frame.rs`) and through files with a hand cut frame (`tests/candidates.rs`), not with an injected `write` error.
- It does not measure a reader that has to wait on a stopped writer or a stopped rotator.
- It uses records padded in `session_id`. A `sample` record with a real `procs` list is not measured.
- It was not run on Linux. The crate compiles for `x86_64-unknown-linux-gnu`, and its tests have not run there.
- A, C2 and D lost no acknowledged record in 144,000 attempted appends each, in these three runs. That is an observation and not a proof.

What the platform decides, and the benchmark only observes:

- Whether concurrent `O_APPEND` writes of one frame each stay whole is a property of the local file system. No torn or interleaved line appeared in any of the 576,000 appends of the three runs on APFS. A network file system is not covered.
- `flock` is advisory. A process that does not call it is not excluded.
- A rename and an unlink are atomic in the namespace of one local volume. C2 depends on both.

## Reproduce

```sh
cargo build --release -p agentdust-bench
./target/release/journal-bench run --json run-1.json
./target/release/journal-bench run --sync off --rotate-period-ms 1 --reader-records 0 --json stress.json
./target/release/journal-bench durability --samples 200 --json durability.json
./target/release/journal-bench render run-1.json
./target/release/journal-bench fs "$TMPDIR"
```

`run` exits with status 1 and saves nothing when it passes `--budget-secs` (60 by default). Run it when the Mac is quiet and read the load averages it prints.
