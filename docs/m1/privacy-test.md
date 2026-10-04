# Privacy invariant test (S14)

S14 says raw tags, commands, output and environments are never persisted. The test is `crates/agentdust/tests/privacy.rs`, and its scanner is `crates/agentdust/tests/privacy_support/mod.rs`. It runs the real hook binary, reads back every byte the data directory holds, and looks at what the run left in the system temp directory.

## What it runs

The hook runs four times with `AGENTDUST_DATA_DIR` set: `SessionStart`, `PreToolUse` and `PostToolUse` for `Bash`, and `SessionEnd`. Every payload carries:

| Field | Value |
| --- | --- |
| `cwd` | `/Users/<user sentinel>/work/<repo sentinel>` |
| `transcript_path` | a path under the same user and repository names |
| an unknown field | a sentinel name and a sentinel value |
| `tool_input.command` (tool events) | a command with a sentinel token and the repository name |
| `tool_response.stdout` (`PostToolUse`) | 5 MB with a sentinel at the start, the middle and the end |
| `session_id` | a sentinel the schema allows |

The hook process also has a sentinel in `AGENTDUST_SESSION` (the one variable the product reads from other processes) and in a generic variable, and its working directory is a directory with a sentinel name.

Every sentinel ends in the process id of the test run, for example `command-sentinel-vQ4x-41873`. A second run on the same machine cannot be mistaken for this one in a shared temp directory.

## What it scans in the data directory

The scan runs at three stages:

1. After the four hook runs.
2. After `journal::rotate`. Before it, the test appends one record from an earlier boot with `journal::append`, so the active file holds five records.
3. After `journal::retain` with the default policy. It drops the earlier-boot record and rewrites the generation with the four hook records. The test requires the report to say one generation rewritten, none deleted and one record dropped for its boot.

Across the stages that covers `install.secret`, `journal.jsonl`, `journal.maint`, the rotated generation and the rewritten generation. At each stage:

- None of 13 values appears in any file: the command token, the three output sentinels, the user and repository names, the full cwd, the transcript name, the unknown field name and value, the environment value, the tag, and the name of the hook's own working directory.
- The session id appears only in journal files.
- Every file name is one of `install.secret`, `journal.maint`, `journal.jsonl` or `journal.<digits>.jsonl`, so a new kind of file fails the test until someone decides what it may hold. A `journal.compact.tmp`, a `journal.jsonl.corrupt-<n>` copy and an `install.secret.<hex>.tmp` are unknown names, because a run that finishes leaves none of them. Every file is a regular file with one link and mode 0600, and the directory is 0700.
- Every segment of every journal file is a record, and every key of it is in `journal::RECORD_KEYS`.
- The hook wrote all ten keys it is meant to write, the journal reads back with the expected number of records (4, 5, 4), and the session id is visible to the scan, so a scan of the wrong directory cannot pass.

Each hook run exits 0 and prints nothing on stdout or stderr. The hook's working directory stays empty.

## What it scans outside the data directory

Before the first hook runs, the test notes the time. After the four runs it walks the system temp directories: `std::env::temp_dir()`, `/tmp` and `/var/tmp`, the ones that exist, counted once when two of them are the same directory. It goes three levels deep, skips the test's own directories by inode, and does not follow symlinks. Every entry whose ctime is not earlier than the noted time is checked. A value from the list of 13, or the session id, in the path of an entry fails the test. For a regular file the first 32 MiB are read, and the same values in the bytes fail it. Only entries changed during the run are read, because the temp directory of a development machine holds thousands of older entries.

A second test starts the hook with `TMPDIR`, `HOME` and the working directory each set to a new empty directory, runs the four events, and requires all three to be empty afterwards. A temp file written through the standard library, whatever it holds, fails this test.

Eleven more tests run the scanners on planted input and require them to refuse exactly what they should:

- A forbidden value in each kind of file, and the session id in a journal file (allowed) and in every other file (refused).
- A key outside the list, a journal segment that is not a record (garbage, a torn frame, a newer version), and a file name the scan does not know.
- In the temp scan, a value in the bytes and in the name of a new file, a file older than the start, an excluded directory, a file at the third and at the fourth level, a symlink and a FIFO that are not followed or read, and the presence of `std::env::temp_dir()` among the directories walked.

## The key list

`journal::RECORD_KEYS` is an array of 11 names, defined next to `Record`. `crates/agentdust-core/tests/journal_keys.rs` requires it to equal the keys of a record with every field set, in written order, with `v` first, and destructures `Record` so that a new field does not compile until the test is edited. To add a journal field:

1. Add it to `Record`.
2. Add its name to `RECORD_KEYS`. The array length is part of the type, so the compiler asks for this.
3. Update `journal_keys.rs`.
4. If the hook writes it, add the name to `WRITTEN_BY_THE_HOOK` in `privacy.rs`.

## Limits

- A value that is hashed before it is stored, such as `cwd_key`, cannot be found by a byte scan. The test shows the path is absent, and `hook_cwd.rs` checks the digest against the secret.
- `health.json`, `audit.log`, `inspection/` and `manifest.json` do not exist yet. Their files fail the name check until they are added to this test.
- The support bundle and `ModelFinding` output (S15) are not covered. They arrive with the code that produces them.
- The temp scan reaches three levels and the three directories above. A file that the hook creates and deletes inside one run, or one written under another path, is not seen. The empty `TMPDIR`, `HOME` and working directory cover the paths a hook reaches through the environment.
