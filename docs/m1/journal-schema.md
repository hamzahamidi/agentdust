# Journal schema version 1 and its store

This page describes `agentdust_core::journal` as it is built. [ADR-1](adr-journal-format.md) gives the reasons for the storage choice and lists the spec edits. Spec section 3.2 states the same storage, and the last section lists where the code differs from the spec.

## Record

One JSON object per frame. Fields are written in this order, and an absent optional field is omitted. `v` is always the first key, because a reader recognises a line of a newer schema from its first bytes.

| Field | Type | Rule |
| --- | --- | --- |
| `v` | number | 1 on write. A record with another value is refused by `encode` |
| `kind` | string | `session_start`, `session_end`, `shell_start`, `shell_end`, `sample`, `server_start` |
| `agent` | string | `claude`, `codex`, `cursor` |
| `session_id` | string | |
| `subagent_id` | string, optional | |
| `tool_use_id` | string, optional | |
| `wall_ts` | unsigned 64-bit | Milliseconds since the Unix epoch, for display |
| `mono_ts` | unsigned 64-bit | Nanoseconds of `CLOCK_MONOTONIC`. It orders events within one boot only |
| `boot` | string | Boot session UUID |
| `cwd_key` | string, optional | Lowercase hex, 1 to 64 characters |
| `exe_base` | string, optional | At most 64 bytes, no C0, C1 or bidi control characters |

The keys are listed once, in `journal::RECORD_KEYS`. The [privacy test](privacy-test.md) fails on a key in a journal file that is not in the list, so adding a field means editing the list.

`agent_identity`, `session_tag_key` and `procs` are not part of the record. A `null` optional field reads as absent, and a field this build does not know is ignored on a version 1 line. `ExeBase` and `CwdKey` check their rule in the constructor and in `Deserialize`, so a line that breaks it is malformed on read and no writer can produce it. The hook writes `cwd_key`, an HMAC under the install secret ([install-secret.md](install-secret.md)), and leaves `exe_base` empty.

## Frame

A frame is the byte `0x1E`, the record as compact JSON and the byte `0x0A`. It is at most 65,536 bytes including both delimiters, so the JSON is at most 65,534 bytes. The JSON encoder writes `0x1E` and `0x0A` as escapes, so a frame holds exactly one of each.

`decode` and `scan` split a file on both bytes. Every non-empty segment lands in exactly one class:

| Class | Segment |
| --- | --- |
| record | Ends with `0x0A`, is a JSON object, has `v` 1 and passes the field rules |
| malformed | Ends with `0x0A` and is not valid UTF-8, is not a JSON object, repeats a field, lacks a field, has a mistyped field, has `v` of 0 or a `v` that is not an integer, breaks the `exe_base` or `cwd_key` rule, or is over the 65,534 byte limit without a newer version |
| newer version | Ends with `0x0A` and has an integer `v` above 1. It is never interpreted, even when shaped like version 1 |
| unknown kind | Ends with `0x0A`, has `v` 1 and a `kind` this build does not know |
| torn | Ends at `0x1E` or at the end of the file. It is never parsed, even when it happens to be complete JSON |

An empty segment is ignored. A line without the leading `0x1E` still decodes, so a file of bare lines reads as records.

A segment over the limit is held only up to 65,535 bytes. Its version comes from its first bytes: `{"v":`, a number without leading zeros of up to 20 digits, then `,` or `}`. A line over the cap that starts that way with a value above 1 is a newer-version line, and any other line over the cap is malformed. Every future schema therefore keeps `v` as its first key.

## Writing

`Journal::append(record)` takes these steps:

1. Encode the frame. `WrongVersion` when `v` is not 1, `TooLarge` when the frame is over 65,536 bytes. Nothing is opened or created before this check.
2. Ask the volume probe about the directory, or about its nearest existing ancestor when it does not exist yet. A volume that is not local APFS returns `UnsupportedFilesystem` with the facts, and nothing is created.
3. Create the final component of the data directory with mode 0700 when it is missing, then check it: a real directory, owned by the current user, with no bit outside 0700. The parent must exist. A missing parent fails with `Io(NotFound)`, nothing is created and the hook stays silent.
4. Open `journal.jsonl` with `O_APPEND` and `O_NOFOLLOW`. A missing file is created exclusively with mode 0600 and then reopened, and is never written through the creating descriptor. When a rotation takes the file between the creation and the reopen, the open is repeated, up to 3 rounds, and then fails with `NotFound`.
5. Write the whole frame with one `write(2)`. A call that fails with `EINTR` has transferred nothing and is restarted. Fewer bytes than the frame, zero included, is `ShortWrite` and is never completed by a second call. Any other error is returned.
6. Compare `fstat` of the descriptor with `lstat` of the path. They match when `st_dev` and `st_ino` are equal and `st_nlink` is not 0. Otherwise close and go to step 4. After 3 attempts the result is `Stale`.

No lock is taken and no lock file is created. Appends are not synced, so an acknowledged record can be lost on an OS crash or a power failure, and lost evidence only lowers confidence. The one write call relies on POSIX append semantics for a local regular file. It is stress tested on APFS and is not an APFS guarantee.

`Ok(Appended { attempts })` is the acknowledgement: the whole frame was written and the recheck of step 6 then found the path still naming the file written. `attempts` says how many frames were written, and a value above 1 means the earlier frames went into files that were no longer the active one. Those are tentative copies. They are not acknowledgements, and readers collapse them with the acknowledged one. An append that ends in `Stale` is not acknowledged and its tentative copies may or may not be readable: one in a file that rotation sealed stays until retention drops it, and one in an unlinked file is gone.

The hook ignores the result of `append`, exits 0 and prints nothing. A record that fails with `TooLarge`, `UnsupportedFilesystem`, `ShortWrite`, `Stale`, a refusal or an I/O error is dropped silently. The hook health counters of spec 4.4 are planned for M2.

The write goes through the `FrameWriter` trait, which the tests implement to inject faults and to pause an append between its write and its recheck.

### What survives a failed write

Each case is a test in `crates/agentdust-core/tests/journal_append_faults.rs`. The cases write record N between a good record and a good record N+1.

| Write | Result | On disk after N | Read |
| --- | --- | --- | --- |
| `EINTR` twice, then whole | `Ok`, 3 calls | the frame once | N and N+1 |
| Zero bytes | `ShortWrite`, 1 call | nothing | N+1 |
| 1 byte | `ShortWrite`, 1 call | a lone `0x1E` | N+1, nothing counted |
| Half of the frame | `ShortWrite`, 1 call | a torn frame | N+1, 1 torn frame |
| All but the newline | `ShortWrite`, 1 call | a torn frame holding complete JSON | N+1, 1 torn frame, N never parsed |
| `ENOSPC` or `EIO`, nothing transferred | `Io`, 1 call | nothing | N+1 |
| `EIO` after N minus 1 bytes landed | `Io`, 1 call | a torn frame | N+1, 1 torn frame |

A sweep over every cut length from 0 to N minus 1 reads the record before and the record after in all of them, with no malformed line.

## Reading

`Journal::read()` returns a `ReadReport` and takes no lock. A missing directory is an empty report and is not created.

1. Check the directory with the same rules as an append. A symlink, a file, a foreign owner or a loose mode is `Refused`.
2. Open `journal.jsonl` for reading. A missing file is fine. A symlink, a FIFO, a directory, a hard link, a foreign owner or a loose mode is `Refused`.
3. List the files named `journal.<stamp>.jsonl`, where `<stamp>` is a decimal `u64` without leading zeros.
4. Open each generation. One that vanished is skipped. One that is a symlink, a FIFO, a directory, a hard link, has a foreign owner or a loose mode is not followed and is counted in `unsafe_files`. Any other open error fails the read.
5. A file that is the same inode as one already held is read once.
6. Decode every file, add the counters, order the records and collapse exact duplicates.

Opening the active file before the listing makes the set coherent. A rotation that lands after the open moves the file under a generation name, and the listing then names the inode the reader already holds. A compaction that lands after the listing leaves the reader the original or the replacement, and each holds the record. A deletion after the listing removes only records that a retention rule dropped. Records acknowledged while the read runs may or may not appear. `crates/agentdust-core/tests/journal_snapshot.rs` pauses a reader after its open and after its listing and runs a rotation, a compaction and a deletion in the gap, and `journal_rotate_interleave.rs` and `journal_retain_interleave.rs` do the same with the real rotation and retention of [journal-retention.md](journal-retention.md).

| `ReadReport` field | Meaning |
| --- | --- |
| `records` | Records in presentation order |
| `malformed_lines` | Malformed segments, including non-UTF-8 ones |
| `torn_frames` | Torn segments |
| `truncated_last_line` | The last segment of some file is torn. An append that is still in flight looks the same |
| `newer_version_lines` | Newer-version segments, over the cap or not |
| `unknown_kind_lines` | Unknown-kind segments |
| `unsupported_version` | `newer_version_lines` is above 0. Consumers treat owned classes as unavailable |
| `duplicates_removed` | Records dropped because an equal record was already present |
| `unsafe_files` | Generations that were not followed |
| `filesystem` | The volume of the directory, or `None` when the probe fails or the directory is missing |

`skipped_lines()` adds the four counters for segments that did not become a record. `on_unsupported_filesystem()` is true when the volume is known and not supported. A read does not refuse an unsupported volume. It cannot tear anything, and the report says where the files are.

Nothing in the reader or the writer rewrites a line. A newer-version line stays byte for byte in its file, and a frame appended after it starts with its own `0x1E`.

### Order

Records are ordered by these keys in turn:

1. The rank of the boot, which is the earliest `wall_ts` among the records of that boot, then the boot string.
2. `mono_ts`.
3. `wall_ts`.
4. Every field of the record, in the order the struct declares them, with `kind` and `agent` in the order of their variants.

The last key makes the order total, so it does not depend on which file holds a record or in which order the reader opened them, and two different records are never tied. It compares the records themselves, so it needs no hash. Records equal under it are equal in every field, and the reader keeps one.

This is a presentation order and not a causal one. `mono_ts` is read before the append, so a writer that is descheduled in between lands behind a later stamp, and a consumer pairs events by `tool_use_id` and never by position. The order across boots is weak, because boots are ranked by wall time and wall time can move backwards.

## Volume policy

The journal supports a local APFS volume: `statfs` reports `f_fstypename` equal to `apfs` and `MNT_LOCAL` set in `f_flags`. On any other volume `append` returns `UnsupportedFilesystem` and writes nothing. A network volume breaks the premise of an atomic append, SMB is unproven, and a local volume that is not APFS was not tested.

`Journal::status()` returns the facts for a later `agentdust status`, and `describe()` prints them as `apfs, local, supported` or `nfs, not local, not supported`. The probe is the `VolumeProbe` trait. `SystemVolume` calls `statfs` on macOS and answers `unknown, not local, not supported` elsewhere, and `FixedVolume` answers a fixed value, which is how the tests and the Linux job write a journal. `crates/agentdust-core/tests/journal_volume.rs` runs the decision on the volumes of S24 and, on macOS, on the real temporary directory and on `/dev`.

On this machine `$TMPDIR` is `apfs, local, supported` and `/dev` is `devfs, local, not supported`. No network volume was available, so the refusal of NFS and SMB rests on the flag and was not observed.

## Safe opens

`safe_open::open_file(path, Read | Append | Create)` opens with `O_NOFOLLOW` and `O_NONBLOCK` and then judges the descriptor with `fstat` and, on macOS, with its extended ACL. `check_dir`, `open_dir` and `ensure_dir` apply the same rules to a directory. The data directory, the journal, the generations, `journal.maint`, `journal.compact.tmp`, the copies and `install.secret` with its temporary file all go through them.

| Refusal | Error |
| --- | --- |
| The path is a symlink, dangling or not | `Symlink` |
| A file that is not regular (FIFO, directory, device) | `NotRegular` |
| A directory path that is not a directory | `NotDirectory` |
| A file with more than one hard link | `HardLinked` |
| An owner other than the effective uid | `ForeignOwner` |
| Any mode bit outside 0600 for a file or 0700 for a directory | `LooseMode` |
| On macOS, an extended ACL with an allow entry | `ExtendedAcl` |

`Append` and `Read` never create. `Create` is exclusive with mode 0600. A FIFO is refused at once and never blocks. A refused open reads and writes nothing.

The ACL is read from the opened descriptor with `acl_get_fd_np` and `acl_to_text`, declared in `crates/agentdust-core/src/acl.rs` because the `libc` crate does not have them. A file or directory with no ACL passes, and so does one whose entries are all `deny`, such as the `group:everyone deny delete` entry that macOS puts on `~/Library` and `~/Library/Application Support`. Any other entry refuses, and so does a line that is not a recognised entry. The check matters because a parent directory with an inheritable allow entry gives a new file mode 0600 and an ACL that lets another account read it. The mode bits do not show that. The check runs on the descriptor, so it sees an ACL that was inherited when the file was created. A file or directory that `Create` or `ensure_dir` has just made and then refuses is removed, so a refusal leaves no `install.secret.<hex>.tmp` and no empty journal behind. Other platforms have no ACL check.

## Cost

`cargo test --release -p agentdust-core --test journal_cost -- --ignored --nocapture` measures 2,000 samples after 200 warm-up calls, with a 150 byte record. Two sessions of three runs each, on a shared machine. Each cell is the range of the three runs, in milliseconds.

| Call | p50, load 4.0 to 4.5 | p95, load 4.0 to 4.5 | p50, load 13.3 | p95, load 13.3 |
| --- | ---: | ---: | ---: | ---: |
| `statfs` of the directory | 0.0015 to 0.0038 | 0.0016 to 0.0040 | 0.0016 to 0.0017 | 0.0019 to 0.0023 |
| `lstat` and `statfs` (`locate`) | 0.0031 to 0.0058 | 0.0032 to 0.0067 | 0.0033 to 0.0035 | 0.0042 to 0.0053 |
| `append` with the system probe | 0.0422 to 0.0430 | 0.0523 to 0.0593 | 0.0486 to 0.0511 | 0.0694 to 0.0732 |
| `append` with a fixed probe | 0.0394 to 0.0416 | 0.0456 to 0.0500 | 0.0457 to 0.0496 | 0.0630 to 0.0732 |

The system probe adds 0.001 to 0.005 ms to the p50 of an append, under 0.2% of the 2.2 ms of a hook.

The whole hook is measured with `cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture` after 20 warm-up runs, against a budget of p50 10 ms and p95 20 ms. The first session is commit 9f904e6 and the second is commit 62046c0.

| Session | Load, 1 minute | p50 | p95 |
| --- | ---: | ---: | ---: |
| First, 3 runs | 6.2 to 6.7 | 2.20 to 2.21 ms | 2.42 to 2.77 ms |
| Last commit, 3 runs | 13.3 | 2.66 to 2.73 ms | 3.03 to 3.81 ms |

## Where this differs from the spec

- Section 7.3 says a wrong directory mode "is corrected". The journal functions refuse a data directory looser than 0700 and do not change it. The startup validation that corrects the mode is not part of the journal.
- Section 3.2 lists `cwd_key` and `exe_base` without limits. The limits above are new.
- A file mode is refused when it has any bit outside 0600, which includes an owner execute bit.

## Not built yet

The hook health counters for each kind of drop. The `exe_base` value in the hook. A command or a server call that runs `prune`: rotation, retention and the recovery of a damaged generation are in `agentdust-core` and [journal-retention.md](journal-retention.md) describes them.
