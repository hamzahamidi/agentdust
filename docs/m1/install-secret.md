# Install secret and keyed working directory digests

The journal never holds a working directory. It holds `cwd_key`, an HMAC of the canonical path under a random per-install secret.

A reader without the secret cannot recover the path or test a guess against the key. Spec 3.2 and 7.2 define the contract. This page describes the code in `agentdust_core::{secret, digest, cwd}` and the Claude Code hook. The record fields and the store are in [journal-schema.md](journal-schema.md).

## The secret

`install.secret` in the data directory holds 32 bytes from `getentropy`. The file has mode 0600 and one link, and the data directory has mode 0700.

`secret::load_or_create(dir)`:

1. Asks the volume probe about the directory, or about its nearest existing ancestor when it does not exist yet. A volume that is not local APFS returns `UnsupportedFilesystem` with the facts. Nothing is read or created, and the directory is not made.
2. Checks the data directory with `safe_open::ensure_dir`, creating it with mode 0700 when it is missing.
3. Reads `install.secret` through `safe_open` and requires exactly 32 bytes. A found secret is returned and the file is not touched.
4. Only when the file is missing: draws 40 bytes, writes the first 32 to a temporary file in the same directory (created exclusively, mode 0600, named `install.secret.<16 hex characters>.tmp` from the other 8 bytes), syncs it, and renames it into place with an exclusive rename (`renamex_np` with `RENAME_EXCL` on macOS, `renameat2` with `RENAME_NOREPLACE` on Linux).
5. A process that loses the rename removes its temporary file. Every caller then reads the installed file through step 3, so two hooks that start together end with one secret.

The volume policy is the journal's ([journal-schema.md](journal-schema.md#volume-policy)), because the secret lives in the directory the journal lives in. Without the check a hook on an unsupported volume would create the directory and the secret and only then have its record refused. `load_or_create_with(dir, volume, probe)` takes the volume probe, so tests and Linux use `FixedVolume`.

The exclusive rename is used instead of a create followed by a write so that `install.secret` never exists with fewer than 32 bytes. With a create and a write, a second hook could read an empty file, find a wrong size and record nothing, and a hook killed between the two calls would leave an empty secret that every later hook refuses. A hard link followed by an unlink was not used on macOS because the secret would briefly have two links, which `safe_open` refuses.

A secret that is present and unusable is an error, and the file is left as it was. It is never replaced, because a new secret orphans every digest already in the journal.

| State of `install.secret` | Error |
| --- | --- |
| Not 32 bytes | `WrongSize { found }` |
| Any mode bit outside 0600 | `Refused(LooseMode)` |
| Symlink, dangling or not | `Refused(Symlink)` |
| More than one hard link | `Refused(HardLinked)` |
| FIFO or directory | `Refused(NotRegular)` |
| Owned by another user | `Refused(ForeignOwner)` |
| Data directory a symlink, a file, or looser than 0700 | `Refused(..)`, and no secret is created |
| Volume not local APFS | `UnsupportedFilesystem(facts)`, and nothing is created |

A process killed between the creation of its temporary file and the rename leaves `install.secret.<16 hex characters>.tmp` behind. It holds 32 random bytes that were never the secret, no code reads it as the secret, and no code removes it.

### Durability

The temporary file is synced before the rename (`File::sync_all`). The directory is not synced after it. After an operating system crash or a power failure the secret can be missing, and the next hook draws a new one. Keys made under the old secret then no longer match, which is the "lost install secret" row of spec 6.5: the evidence lowers confidence and nothing else.

### Interleavings

`load_or_create_with` calls an `InstallProbe` at two points where another caller can get in: `Absent`, after a read found no secret, and `TempSynced`, after the temporary file is synced and before the rename. The tests in `crates/agentdust-core/tests/secret_install.rs` use them to reach the losing path by construction:

| Test | What it fixes |
| --- | --- |
| A file planted at `TempSynced` | It is returned and not replaced. An unusable one (5 bytes, 40 bytes, mode 0644) is reported and left as planted. In every case the temporary file is gone |
| A caller paused at `TempSynced` while another finishes | The paused caller returns the secret of the one that finished |
| 16 callers held at `TempSynced` until all 16 temporary files exist | All 16 return the one secret, 15 of them through the losing path, and only the secret remains |
| 16 callers held at `Absent` until all 16 have found no secret | The same |
| The probe at `TempSynced` looks at the directory | `install.secret` does not exist yet, and the one temporary file is 32 bytes of mode 0600 |

The callers wait for each other with a 60 second timeout, so a failing run stops and does not hang.

## The digest

`digest::keyed_digest(key, domain, data)` is HMAC-SHA256 with the secret as key over the domain label, one NUL byte and the data, written as 64 lowercase hex characters.

| Domain | Label |
| --- | --- |
| `Domain::Session` | `AGENTDUST-SESSION-v1` |
| `Domain::Cwd` | `AGENTDUST-CWD-v1` |

`digest::hmac_sha256` is the plain HMAC. The tests check it against the RFC 4231 vectors (keys of 4, 20, 25 and 131 bytes), and check `keyed_digest` against known answers computed with Python's `hmac` module, including the variant without the NUL byte. The hook test plants a secret of the bytes 0 to 31 and compares the journaled key with a literal computed the same way.

HMAC pads a key shorter than its 64 byte block with zero bytes, so two keys that differ only in trailing zero bytes are one key. The secret is always 32 bytes, so the property that two secrets give two digests is tested with 32 byte keys.

`Domain::Session` is used through `tag::key_of`, by the hook for the key of a new session tag and by the process inventory for the key of the tag it reads from an environment.

## Canonical working directory

`cwd::canonical_cwd(path)` works in three steps.

1. An empty, relative or NUL-holding string returns `None`.
2. Normalise lexically: drop `.` segments, repeated separators and a trailing slash, and resolve `..` (`/..` stays `/`).
3. When the normalised path exists, replace it by its real path with `realpath`.

A path that does not exist is returned normalised and its existing parents are not resolved. A symlink loop also returns the normalised path.

- Idempotent: `canonical_cwd(canonical_cwd(p))` equals `canonical_cwd(p)`.
- The normalised form is never longer than its input, so the length constant is 0. A resolved path can be longer, because a symlink target can be, and its length is bounded by the operating system.
- `..` is removed before symlinks are resolved: `link/../x` is read as `x` beside `link`, not as the parent of the link's target.
- On this machine's APFS volume, which ignores case, `realpath` returned the on-disk spelling for a path typed in another case: `/users/hhamidi` came back as `/Users/hhamidi`. Spellings that differ only in case therefore give one key there. On a volume that is case sensitive they are different directories and give different keys.
- `realpath` runs with no time limit. A working directory on a stalled network mount blocks the hook until the host gives up on it.

`cwd::cwd_key(secret, cwd)` is `keyed_digest` of the canonical path under `Domain::Cwd`, as a `CwdKey`, or `None` when there is no canonical path.

## The hook

`HookEvent.cwd` is read with its own limit of 4096 decoded bytes, enforced while reading like the 256 byte identifier limit. An oversized `cwd` refuses the whole event, so nothing is recorded and no file is created.

For `SessionStart`, `SessionEnd`, `SubagentStart`, `SubagentStop`, and Bash `PreToolUse` and `PostToolUse`, the hook loads the secret when it needs to write `session_tag_key`, `subagent_id`, or `cwd_key`. `session_id` remains as supplied. New `subagent_id` values and the working directory are keyed digests; the working directory is dropped. Older version 1 records may contain raw subagent IDs. Events that are not journaled touch nothing. The secret is also touched when a journaled event has an `agent_id`, even when `cwd` is absent.

If the secret cannot be used for an event with `agent_id`, the hook writes `subagent_attribution_unknown` without the raw ID. This makes ownership for that session non-actionable. A `SubagentStart` or `SubagentStop` without an ID also makes the session non-actionable. Other records are written without the `cwd_key`, `session_tag_key` or keyed `subagent_id` that depended on the secret. On a volume that is not local APFS neither the secret nor the record is written. The hook exits 0 and prints nothing in every case.

## Checks

- `crates/agentdust/tests/hook_cwd.rs` runs the binary. A sentinel path, also placed in the command text and reached through a symlink, a sentinel in the tool output and a sentinel session tag in the hook's environment appear in no file under the data directory, and the raw secret appears in no file but `install.secret`. Sixteen processes started together on an empty directory all key with the one secret, and the same holds against an existing secret. Ten unusable-secret setups leave the file and the record count as expected, and the FIFO case has a 60 second hang guard.
- `crates/agentdust-core/tests/secret_visibility.rs` polls the secret while it is removed and recreated, until the poller has seen the file 20 times and at least 150 cycles have run. Every sighting must be 32 bytes.
- `crates/agentdust-core/tests/secret_volume.rs` runs the volume policy with injected facts (nfs, smbfs, apfs without the local flag, hfs, devfs, unknown), and on macOS with the real temporary directory and `/dev`.

## Hook latency

Release binary, 200 runs after 20 warm-up runs (30 runs for the empty directory case), Apple M1 Max, `cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture --test-threads=1`. The payload carries a `cwd` that exists and has 9 path components. Each cell is the range of three runs, in milliseconds, and the load is the 1 minute load average of `uptime` around the session.

| Payload | Session | Load | p50 | p95 |
| --- | --- | ---: | ---: | ---: |
| With `cwd` | first | 4.9 to 5.1 | 2.27 to 2.30 | 2.58 to 2.73 |
| With `cwd` | second | 3.2 to 3.3 | 2.26 to 2.29 | 2.56 to 2.76 |
| Without `cwd`, no secret read | first | 4.9 to 5.1 | 2.20 to 2.22 | 2.54 to 2.71 |
| Without `cwd`, no secret read | second | 3.2 to 3.3 | 2.20 to 2.51 | 2.67 to 2.86 |
| First hook in an empty data directory | first | 4.9 to 5.1 | 7.77 to 7.80 | 8.73 to 8.99 |
| First hook in an empty data directory | second | 3.2 to 3.3 | 7.70 to 7.82 | 8.78 to 8.92 |

The budget is p50 10 ms and p95 20 ms. The first hook after an install also makes the directory, creates and syncs the secret and creates the journal, once per install.

## Where this differs from the spec

- Section 7.3 asks that the secret has no group or world access. The code refuses any mode bit outside 0600, which includes an owner execute bit.
- Section 3.2 does not define the canonical form. The rules above are new.
- Section 7.2 asks for exclusive, no-follow creation of the secret. The code creates a temporary file that way and publishes it with an exclusive rename, so the secret path is never created directly.
- The secret follows the journal's volume policy, which the spec does not mention.
- `canonical_cwd` returns an `Option`, so a relative path never becomes a key.
