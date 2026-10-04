# Fuzz targets and seeds

Spec 9.2 lists the journal decoder and the hook payload parsers as fuzz targets, requires every pull request to compile all fuzz targets and replay their regression corpora, and asks for at least 10 minutes of fuzzing each before a release. This page describes the targets in `fuzz/`, a separate cargo workspace that is never shipped.

## Targets

| Target | Input | What runs |
| --- | --- | --- |
| `procargs` | `KERN_PROCARGS2` bytes | the parser, one variable read, and the script argument parser, which must fail with the error `parse` gives and return one of the arguments after the program name |
| `journal_decode` | journal bytes | `journal::decode` |
| `journal_read` | up to four files, split at byte `0x1C` | `journal::read` over a 0700 directory of 0600 files |
| `claude_payload` | hook stdin | `claude::parse_event` |
| `sanitize` | any bytes | `sanitize::escape`, `redact_command`, `terminal_command` and `terminal_path`, and `inventory::parse_launchctl_list` |

`journal_read` writes the first three parts as `journal.1000.jsonl`, `journal.1001.jsonl` and `journal.1002.jsonl` and the last part as `journal.jsonl`. A part may be empty, and separators after the third stay inside the last part. `0x1E` is the frame delimiter, so the file separator is `0x1C`. JSON text cannot hold either byte raw. Copying a frame across a separator gives the reader duplicates to remove.

For one input in 16, `claude_payload` also parses a copy in which the first, second, third or fourth quoted string is repeated until it is up to 1 MiB long (the stretcher in `fuzz/src/bounds.rs`). It makes very long strings reachable without a large `-max_len`.

## What each target asserts

Every target fails on a panic and on memory use above its bound.

| Target | Invariants |
| --- | --- |
| `journal_decode` | every non-empty segment between `0x1E` and `0x0A` lands in exactly one counter, `unsupported_version` equals "a newer line was seen", `torn_frames` is at least 1 when the last segment is torn, the report is the same when the bytes arrive in chunks of 1 to 70,000 bytes, and the first 32 decoded records are encoded and decode back unchanged |
| `journal_decode`, resynchronisation | a known frame appended to the input is the last record read, after exactly the records the input gave, with every counter unchanged and no truncated tail. A known frame placed before the input is the first record read |
| `journal_read` | records plus duplicates equal the records of each part decoded alone, every counter is the sum over the parts, no file is reported unsafe, the records are exactly the distinct records of the parts, and the order is strictly increasing under the key of [journal-schema.md](journal-schema.md), computed again inside the target |
| `claude_payload` | no field is longer than its limit, `journal_kind` does not panic, and an event written back as JSON parses to the same event |
| `sanitize` | the escaped text holds no control, bidirectional or invisible character and is undone exactly by reading the escapes back, the terminal views of any argument bytes and any path bytes hold none either, and the launchctl list parser never panics |

The resynchronisation check is the framing contract: whatever precedes a frame, a cut write included, costs only the bytes of that write.

## Memory bound

`fuzz/src/meter.rs` is a global allocator that counts live bytes. Each target takes the peak live bytes of one call.

The journal targets allow `FIXED + 8 * input bytes` and, separately, `FIXED + 6 * retained bytes`. `FIXED` is 393,216 bytes (6 frame limits). Retained bytes are the size of every record the call kept: `size_of::<Record>()` plus its string lengths. For `journal_read` they are summed over the parts decoded alone, because a read holds every record until it removes duplicates.

A bound that is only a multiple of the input cannot hold for an empty input, because the decoder allocates a 64 KiB buffer first, so the bound has a fixed part. The second bound is the one that catches a missing segment limit: a 3 MB segment with no terminator keeps nothing, so it must stay under 393,216 bytes, and it peaks at 131,071.

`claude_payload` allows 131,072 bytes whatever the input size, because the budget in the parser limits what is held. The budget is armed before the object is entered, so a top-level string or a run of whitespace before the opening brace is cut off after 1,600 bytes.

Peaks measured with the same allocator on release builds, `uptime` load 6.36 to 7.62:

| Input | Input bytes | Peak bytes |
| --- | --- | --- |
| `journal::decode`, empty | 0 | 65,536 |
| `journal::decode`, 3 MB segment with no terminator | 3,000,000 | 131,071 |
| `journal::decode`, one 3 MB frame | 3,000,002 | 131,071 |
| `journal::decode`, one frame of 65,536 bytes | 65,536 | 197,169 |
| `journal::decode`, 10,000 minimal records | 1,207,780 | 4,267,157 |
| `journal::read`, the same 10,000 records | 1,207,780 | 4,521,494 |
| `journal::read`, 100,000 records in each of two files, all duplicates | 24,555,560 | 77,789,042 |
| `parse_event`, 50 MB `session_id` | 50,000,000 | 3,157 |
| `parse_event`, 50 MB top-level string | 50,000,000 | 3,074 |
| `parse_event`, 5 MB `cwd` | 5,000,000 | 49,225 |
| `parse_event`, `cwd` at its limit of 4,096 bytes | 4,096 | 8,265 |
| `parse_event`, 50 MB unknown field value | 50,000,000 | 91 |
| `parse_event`, 1,599 bytes of whitespace then an event | 1,650 | 3,278 |

## Seeds

`fuzz/seeds/<target>/` holds one file per behaviour class: 9 files for `procargs`, 37 for `journal_decode`, 18 for `journal_read`, 24 for `claude_payload` and 13 for `sanitize`. The largest file is 70,358 bytes, a `journal_read` seed. A reproducer that a fuzz run finds is added to the seed directory under a name that says what it is.

The journal seeds are in the frame format and group like this:

| Group | `journal_decode` seeds |
| --- | --- |
| Valid input | `one-frame`, `full-frame`, `session-lifecycle`, `two-boots`, `escapes`, `json-escaped-separator`, `crlf`, `bare-lines-without-rs` |
| Cut writes | `cut-after-zero-bytes`, `cut-after-one-byte`, `cut-then-next-frame`, `cut-all-but-the-newline`, `lone-separators`, `torn-tail`, `torn-tail-complete-json` |
| Glued records | `frames-glued-without-newline`, `cut-glued-bare-lines` |
| Frame cap | `exactly-the-cap`, `one-over-the-cap`, `unterminated-over-cap` |
| Newer versions | `newer-version`, `newer-version-shaped-like-one`, `newer-version-over-cap`, `version-one-over-cap`, `version-not-first-over-cap`, `version-numbers` |
| Damage | `bad-fields`, `repeated-field`, `non-utf8`, `bom`, `blank-and-garbage`, `not-objects`, `numbers`, `unknown-kind`, `separators-only`, `newlines-only`, `empty` |

The 18 `journal_read` seeds split at `0x1C` into files: one file, two boots over three files, a record copied into two files, a late writer's copy, a torn tail in a generation, a cut in the middle of the active file, a newer version line (also over the cap) in a generation, an empty generation, equal stamps with different sessions and with different kinds, more separators than files, bare lines in a generation, damaged lines between valid ones, boots out of wall order, an unknown kind, and the same record in four files.

The seeds are replayed twice:

- The normal suite. `journal_corpus` decodes every `journal_decode` seed, compares it with chunked reads at six sizes, re-encodes every accepted record and reads every `journal_read` seed as files. Named seeds pin the claims: a cut costs one record, a glued pair of bare lines costs two, a newer version line over the cap is counted as newer and hides nothing. `claude_corpus` parses every `claude_payload` seed, and `procargs_corpus` parses every `procargs` seed, and `sanitize_corpus` runs every `sanitize` seed through the same checks and requires that some seeds change under escaping and under redaction and that the list parser both accepts and refuses seeds. Each also checks that the seeds still reach every outcome (a record, a malformed segment, a torn frame, a newer version, an unknown kind, a duplicate, several boots, each journal kind, both parse errors), so an emptied or renamed directory fails. `fuzz_layout` requires a seed directory for every target in `fuzz/Cargo.toml` and a target for every seed directory, and reads `.github/workflows/ci.yml` to require the replay loop.
- The Linux CI job. For each target listed by `cargo fuzz list` it checks that `fuzz/seeds/<target>` exists and runs `cargo fuzz run <target> fuzz/seeds/<target> -- -runs=0` with the pinned nightly, which replays every seed through the instrumented target and its memory assertions. libFuzzer also runs the empty input once, so it reports one run more than there are non-empty seeds: 37 runs for the 36 non-empty `journal_decode` seeds.

## Running a target

```
cargo +nightly-2026-10-02 fuzz run journal_decode fuzz/corpus/journal_decode fuzz/seeds/journal_decode -- -max_total_time=15 -max_len=1048576
```

The working corpus goes first because libFuzzer writes new inputs to the first directory it is given. Passing `fuzz/seeds/<target>` first adds generated files to the seeds. `fuzz/corpus` and `fuzz/artifacts` are ignored by git.

## Results

Runs on an Apple silicon Mac, one per target, 16 seconds each (libFuzzer reports 17), fresh corpus, seeds as the initial inputs. The first three rows were one session of 51 seconds. The `sanitize` row is a later run of the same kind:

| Target | `-max_len` | Runs | Runs per second | Load before and after | Crashes |
| --- | --- | --- | --- | --- | --- |
| `journal_decode` | 1,048,576 | 41,806 | 2,459 | 6.46 and 5.68 | 0 |
| `journal_read` | 262,144 | 23,069 | 1,357 | 5.68 and 11.28 | 0 |
| `claude_payload` | 1,000,000 | 136,168 | 8,009 | 11.28 and 9.21 | 0 |
| `sanitize` | 4,096 | 137,804 | 8,106 | 7.41 and 7.24 | 0 |

## Limits

- Seventeen seconds per target is a smoke run. The 10 minute run that spec 9.2 asks for before a release is not part of M1.
- libFuzzer raises its input length slowly. In these runs the length limit stayed at the size of the largest seed (70,162 bytes for `journal_decode`, 70,358 for `journal_read`, 4,157 for `claude_payload`), so inputs above it are reached through the seeds and, for the payload, through the stretcher.
- The memory bound for the journal targets is a multiple of what the call keeps. It does not detect a leak that is proportional to the kept records.
- The meter counts bytes requested from the allocator. It does not count memory that the kernel holds for the process (the stack, file buffers).
- No seed holds a `sample` record with a `procs` list. The cap says nothing about a real sample, and [ADR-1](adr-journal-format.md) leaves the byte budget with `procs_total` and `procs_recorded` to the M2 sampler, so no seed or invariant here covers one.
