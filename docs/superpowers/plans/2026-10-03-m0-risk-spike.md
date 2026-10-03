# AgentDust M0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the M0 risk spike: a Rust workspace that reads process identity and one environment variable on macOS, a Claude Code hook that appends journal events, an MCP approval probe for both protocol generations, CI, a reproducible release dry run that installs through Homebrew, and recorded client experiments.

**Architecture:** One Rust workspace with three product crates (`agentdust-core`, `agentdust-agents`, `agentdust-mcp`), the `agentdust` binary crate, and a test-only `agentdust-testkit` crate. Platform calls live in `agentdust_core::darwin`, so every crate except `agentdust` builds and tests on Linux. The MCP probe changes nothing on the machine; it measures how each client renders and answers a typed-code form.

**Tech Stack:** Rust 1.99.0 (edition 2024), `libc` 0.2.190, `serde` 1.0.229, `serde_json` 1.0.151, `thiserror` 2.0.21, `rmcp` 3.5.0, `tokio` 1.53.2, `cargo-fuzz` with `libfuzzer-sys` 0.4, the Python 3 standard library for release scripts, GitHub Actions.

**Spec:** [docs/superpowers/specs/2026-10-03-agentdust-design.md](../specs/2026-10-03-agentdust-design.md), milestone M0 in [ROADMAP.md](../../../ROADMAP.md).

## Scope

This plan covers roadmap milestone M0 only. M1 to M9 each get their own plan when they start, because M0's results (the client matrix, the Cursor anchor, the journal inputs) change their details.

Every code block in Tasks 1 to 6 was compiled and tested on 2026-10-04 with Rust 1.99.0 on macOS 26.6.2 (Apple silicon) before this plan was written. Expected test counts below are the counts from that run.

Not in this plan, by spec section: the session state machine (3.3), classes and the classifier (3.4), keyed digests and the install secret (3.2), Codex and Cursor adapters (4.2), plan and apply (6), single-link safe opens for every file (7.3), `setup` (8.2), release gating and the tap pull request (8.4, S20).

## Global Constraints

- Rust toolchain `1.99.0` pinned in `rust-toolchain.toml`, edition 2024, workspace resolver 3, `rust-version = "1.99"`.
- The binary targets macOS on Apple silicon. `agentdust_core::darwin` and the `agentdust` crate are macOS only; every other crate builds and tests on Linux.
- On this Mac the default Command Line Tools SDK (27.0) breaks Rust linking. Run `export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` in the shell before any `cargo` command. An executor that opens a new shell per command prefixes each `cargo` command with that assignment. CI needs nothing.
- Data directory `~/Library/Application Support/agentdust`, mode 0700. Every file in it is 0600 and opened with `O_NOFOLLOW`. `AGENTDUST_DATA_DIR` overrides the location; tests use it.
- Hooks always exit 0 and print nothing. Release-binary hook latency: p50 under 10 ms, p95 under 20 ms.
- The journal never stores raw commands, command output or process environments.
- A hook identifier (`session_id`, `tool_use_id`, `agent_id`) longer than 256 bytes makes the event invalid.
- Approval codes: 4 characters from `ACDEFGHJKMNPQRTUVWXY34679`, single use, valid for 120 seconds, exact match.
- `requestState` is a random 128-bit nonce mapped to server memory. An unknown or reused nonce is an error.
- No network code in the binary. Every direct dependency has a line in `docs/dependencies.md`.
- CI runs on `pull_request` and `workflow_dispatch` only. No scheduled workflows. Every action is pinned to a full commit SHA. Artifact retention is 3 days.
- Commits use the global git identity (`22576950+hamzahamidi@users.noreply.github.com`), conventional subjects, a body that explains why, and no `Co-Authored-By` or agent trailer.
- Markdown prose follows the writing rules in `~/projects/CLAUDE.md`: no em or en dashes and none of the banned words listed there.
- Code comments: none, except one-line `// SAFETY:` notes on `unsafe` blocks.
- Creating the public repository, pushing to it, merging pull requests and editing any agent configuration file require the user's explicit yes at the moment they happen.

## Review Focus

1. Empty stdin: a host that closes stdin at once gets exit 0 and no record. Pinned by `empty_stdin_exits_zero_and_appends_nothing` in Task 5.
2. Unwritable data directory: the hook exits 0 without printing or panicking. Pinned by `an_unwritable_data_dir_exits_zero_silently` in Task 5.
3. Oversized identifiers: a 10,000-byte session ID never reaches the journal. Pinned by `rejects_identifiers_longer_than_the_limit` and `an_oversized_session_id_is_not_recorded` in Task 5.
4. Concurrent hooks from several agents: every record lands complete, with no interleaved lines. Pinned by `concurrent_hooks_append_complete_records` in Task 5.
5. Malformed MCP calls: an unknown tool name and a retry without the approval response are errors, never an approval. Pinned by `an_unknown_tool_is_an_error` and `a_retry_without_the_approval_response_is_rejected` in Task 6.

## File structure

| Path | Responsibility |
| --- | --- |
| `Cargo.toml` | Workspace members, shared dependency versions, release profile |
| `rust-toolchain.toml`, `rustfmt.toml` | Pinned toolchain and formatting width |
| `deny.toml`, `docs/dependencies.md` | Dependency policy and the allowlist of direct dependencies |
| `crates/agentdust-core/src/identity.rs` | `KernelIdentity` and `ProcessInfo` types |
| `crates/agentdust-core/src/darwin.rs` | macOS calls: boot session UUID, process info, executable path, `KERN_PROCARGS2`, one environment variable |
| `crates/agentdust-core/src/procargs.rs` | Platform-independent `KERN_PROCARGS2` parser |
| `crates/agentdust-core/src/clock.rs`, `paths.rs` | Wall and monotonic clocks, data directory location |
| `crates/agentdust-core/src/journal.rs` | Locked, append-only JSON Lines journal |
| `crates/agentdust-agents/src/claude.rs` | Claude Code hook payload parsing and event mapping |
| `crates/agentdust-mcp/src/probe.rs` | MCP server with the `agentdust_probe_approval` tool |
| `crates/agentdust/src/main.rs`, `hook.rs` | Subcommand dispatch and the hook entry point |
| `crates/agentdust-testkit` | `fixture-sleeper` binary and live platform tests |
| `fuzz/` | `cargo-fuzz` target and seed inputs for the parser |
| `scripts/*.py` | Release packaging, build comparison, toolchain lock, formula, Homebrew smoke test |
| `scripts/m0/cursor_probe.py` | Cursor experiment hook (not shipped) |
| `.github/workflows/` | CI and the release dry run |
| `docs/m0/` | Client matrix, Cursor experiments and the M0 report |

---

### Task 1: Workspace skeleton and repository basics

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `.gitignore`, `LICENSE`, `README.md`, `SECURITY.md`, `deny.toml`, `docs/dependencies.md`
- Create: `crates/agentdust/Cargo.toml`, `crates/agentdust/src/main.rs`
- Test: `crates/agentdust/tests/version.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: the `agentdust` binary. `agentdust version` prints `agentdust <crate version>` and exits 0. Any other argument list prints a line starting with `usage: agentdust` on stderr and exits 2.

- [ ] **Step 1: Start the work branch**

Skip this step when the executor already created an isolated worktree branch.

```bash
git switch main && git switch -c m0/spike
```

- [ ] **Step 2: Write the workspace manifest, toolchain pin and binary crate manifest**

`Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["crates/agentdust"]

[workspace.package]
version = "0.0.0"
edition = "2024"
rust-version = "1.99"
license = "MIT"
publish = false

[workspace.dependencies]
libc = "0.2.190"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
thiserror = "2.0.21"
tokio = { version = "1.53.2", features = ["macros", "rt", "io-std", "io-util", "sync", "time"] }
rmcp = { version = "3.5.0", default-features = false, features = ["server", "macros", "transport-io", "elicitation"] }

[profile.release]
codegen-units = 1
strip = true
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.99.0"
components = ["clippy", "rustfmt"]
profile = "minimal"
```

`rustfmt.toml`:

```toml
max_width = 110
```

`crates/agentdust/Cargo.toml`:

```toml
[package]
name = "agentdust"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true
```

- [ ] **Step 3: Write the failing test**

```rust
use std::process::Command;

#[test]
fn version_prints_the_crate_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .arg("version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("agentdust {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn an_unknown_command_exits_with_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .arg("nope")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("usage: agentdust")
    );
}
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p agentdust --test version`
Expected: FAIL. Cargo reports that the package has no targets, because `src/main.rs` does not exist yet.

- [ ] **Step 5: Write the binary**

`crates/agentdust/src/main.rs`:

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["version"] => {
            println!("agentdust {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: agentdust version");
            ExitCode::from(2)
        }
    }
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p agentdust --test version`
Expected: `test result: ok. 2 passed`

- [ ] **Step 7: Add the repository files**

`.gitignore`:

```text
/target
/dist
/fuzz/target
/fuzz/corpus
/fuzz/artifacts
.DS_Store
```

`LICENSE`:

```text
MIT License

Copyright (c) 2026 Hamza Hamidi

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

`README.md`:

````markdown
# AgentDust

Finds the processes that AI coding agents (Claude Code, Codex, Cursor) leave running after their sessions end, and stops them after you approve each one with a typed code.

Status: in development. Nothing is released yet. The plan is in [ROADMAP.md](ROADMAP.md) and the design in [docs/superpowers/specs](docs/superpowers/specs).

## Build

```bash
cargo build --release
```

The binary is `target/release/agentdust`. It runs on macOS on Apple silicon.

## Network access

None. The binary contains no network code.
````

`SECURITY.md`:

```markdown
# Security policy

## Reporting a vulnerability

Report it privately through GitHub: open the repository's Security tab and choose "Report a vulnerability". Please do not open a public issue for a security problem.

Include the AgentDust version (`agentdust version`), the macOS version, and the steps that reproduce the problem.

## Supported versions

Nothing is released yet. Reports against the `main` branch are welcome.

## What counts as a vulnerability

- AgentDust signals a process that its rules say it must never signal.
- Approval can be completed without the typed code.
- Data that the design says is never stored (commands, command output, process environments) reaches disk or a model.
```

`deny.toml`:

```toml
[graph]
targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"]

[advisories]
version = 2
yanked = "deny"

[licenses]
version = 2
allow = ["MIT", "Apache-2.0", "Unicode-3.0"]
confidence-threshold = 0.93

[bans]
multiple-versions = "warn"
wildcards = "deny"
allow-wildcard-paths = true

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

`docs/dependencies.md`:

```markdown
# Dependency allowlist

Every direct dependency has one line here. A pull request that adds a dependency adds its line.

| Crate | Used by | Why |
| --- | --- | --- |
| `libc` | agentdust-core | `proc_pidinfo`, `proc_pidpath`, `sysctl`, `flock` and `clock_gettime` |
| `serde`, `serde_json` | all crates | hook payloads, journal records, MCP results |
| `thiserror` | agentdust-core, agentdust-agents | error types |
| `rmcp` | agentdust-mcp | official MCP SDK (Tier 1), elicitation for both protocol generations |
| `tokio` | agentdust-mcp, agentdust | runtime for the `mcp` subcommand only |
| `libfuzzer-sys` | fuzz (not shipped) | fuzz targets |
```

- [ ] **Step 8: Run formatting, lint and dependency policy**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no output, exit 0.

Run: `cargo deny check` (install once with `cargo install cargo-deny --locked` if missing)
Expected: `advisories ok, bans ok, licenses ok, sources ok`

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml .gitignore LICENSE README.md SECURITY.md deny.toml docs/dependencies.md crates/agentdust
git commit -m "build: add the Rust workspace and the agentdust binary" -m "Every later task adds a crate to this workspace, so the toolchain pin, the dependency policy and the release profile have to exist first."
```

---

### Task 2: Process identity on macOS

**Files:**
- Modify: `Cargo.toml` (members and one path dependency)
- Create: `crates/agentdust-core/Cargo.toml`, `crates/agentdust-core/src/lib.rs`, `crates/agentdust-core/src/identity.rs`, `crates/agentdust-core/src/darwin.rs`
- Create: `crates/agentdust-testkit/Cargo.toml`, `crates/agentdust-testkit/src/lib.rs`, `crates/agentdust-testkit/src/bin/fixture-sleeper.rs`
- Test: `crates/agentdust-testkit/tests/platform.rs`

**Interfaces:**
- Consumes: the workspace from Task 1.
- Produces:
  - `agentdust_core::identity::KernelIdentity { boot_session_uuid: String, pid: i32, start_time_us: u64, uid: u32 }` (derives `Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize`)
  - `agentdust_core::identity::ProcessInfo { identity: KernelIdentity, ppid: i32, pgid: i32 }`
  - `agentdust_core::darwin::boot_session_uuid() -> io::Result<String>`
  - `agentdust_core::darwin::process_info(pid: i32, boot_session_uuid: &str) -> io::Result<Option<ProcessInfo>>` (`Ok(None)` when the PID does not exist)
  - `agentdust_core::darwin::exe_path(pid: i32) -> io::Result<Option<PathBuf>>`
  - Test binary `fixture-sleeper [seconds]`, default 30 seconds.

- [ ] **Step 1: Register the new crates in the workspace**

In `Cargo.toml`, replace the `members` line:

```toml
members = ["crates/agentdust-core", "crates/agentdust", "crates/agentdust-testkit"]
```

and add this as the first line under `[workspace.dependencies]`:

```toml
agentdust-core = { path = "crates/agentdust-core" }
```

- [ ] **Step 2: Create the test kit crate**

`crates/agentdust-testkit/Cargo.toml`:

```toml
[package]
name = "agentdust-testkit"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[[bin]]
name = "fixture-sleeper"
path = "src/bin/fixture-sleeper.rs"
test = false

[dev-dependencies]
agentdust-core.workspace = true
libc.workspace = true
```

`crates/agentdust-testkit/src/lib.rs` is an empty file.

`crates/agentdust-testkit/src/bin/fixture-sleeper.rs`:

```rust
use std::time::Duration;

fn main() {
    let seconds = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    std::thread::sleep(Duration::from_secs(seconds));
}
```

- [ ] **Step 3: Write the failing test**

`crates/agentdust-testkit/tests/platform.rs`:

```rust
#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use agentdust_core::darwin;

const SLEEPER: &str = env!("CARGO_BIN_EXE_fixture-sleeper");

struct Fixture(Child);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_sleeper(env: &[(&str, &str)]) -> Fixture {
    let mut command = Command::new(SLEEPER);
    command.arg("30");
    for (key, value) in env {
        command.env(key, value);
    }
    let child = command.spawn().unwrap();
    thread::sleep(Duration::from_millis(100));
    Fixture(child)
}

fn boot() -> String {
    darwin::boot_session_uuid().unwrap()
}

#[test]
fn boot_session_uuid_is_a_uuid() {
    let uuid = boot();
    assert_eq!(uuid.len(), 36, "{uuid}");
    assert_eq!(uuid.matches('-').count(), 4, "{uuid}");
}

#[test]
fn identity_of_this_process_is_stable() {
    let pid = std::process::id() as i32;
    let first = darwin::process_info(pid, &boot()).unwrap().unwrap();
    let second = darwin::process_info(pid, &boot()).unwrap().unwrap();
    assert_eq!(first.identity, second.identity);
    assert_eq!(first.identity.uid, unsafe { libc::getuid() });
    assert!(first.identity.start_time_us > 1_767_225_600_000_000);
}

#[test]
fn a_missing_pid_has_no_identity() {
    assert_eq!(darwin::process_info(i32::MAX, &boot()).unwrap(), None);
    assert_eq!(darwin::exe_path(i32::MAX).unwrap(), None);
}

#[test]
fn a_child_reports_its_parent_and_group() {
    let child = spawn_sleeper(&[]);
    let pid = child.0.id() as i32;
    let info = darwin::process_info(pid, &boot()).unwrap().unwrap();
    assert_eq!(info.ppid, std::process::id() as i32);
    assert_eq!(info.pgid, unsafe { libc::getpgrp() });
}

#[test]
fn a_child_reports_its_executable_path() {
    let child = spawn_sleeper(&[]);
    let path = darwin::exe_path(child.0.id() as i32).unwrap().unwrap();
    assert_eq!(
        path.canonicalize().unwrap(),
        Path::new(SLEEPER).canonicalize().unwrap()
    );
}
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p agentdust-testkit --test platform`
Expected: FAIL to resolve `agentdust_core`, because the crate does not exist yet.

- [ ] **Step 5: Write the core crate**

`crates/agentdust-core/Cargo.toml`:

```toml
[package]
name = "agentdust-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
libc.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
```

`crates/agentdust-core/src/lib.rs`:

```rust
pub mod identity;

#[cfg(target_os = "macos")]
pub mod darwin;
```

`crates/agentdust-core/src/identity.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KernelIdentity {
    pub boot_session_uuid: String,
    pub pid: i32,
    pub start_time_us: u64,
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub identity: KernelIdentity,
    pub ppid: i32,
    pub pgid: i32,
}
```

`crates/agentdust-core/src/darwin.rs`:

```rust
use std::ffi::{CStr, OsString};
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use crate::identity::{KernelIdentity, ProcessInfo};

pub fn boot_session_uuid() -> io::Result<String> {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: `buf` and `len` describe a writable buffer of `len` bytes.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let value = CStr::from_bytes_until_nul(&buf[..len]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "boot session UUID is not NUL terminated",
        )
    })?;
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "boot session UUID is not UTF-8"))
}

pub fn process_info(pid: i32, boot_session_uuid: &str) -> io::Result<Option<ProcessInfo>> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` points to `size` writable bytes for a proc_bsdinfo.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if written <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    if written != size {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short proc_bsdinfo"));
    }
    // SAFETY: the kernel filled all `size` bytes, checked above.
    let info = unsafe { info.assume_init() };
    Ok(Some(ProcessInfo {
        identity: KernelIdentity {
            boot_session_uuid: boot_session_uuid.to_owned(),
            pid,
            start_time_us: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            uid: info.pbi_uid,
        },
        ppid: info.pbi_ppid as i32,
        pgid: info.pbi_pgid as i32,
    }))
}

pub fn exe_path(pid: i32) -> io::Result<Option<PathBuf>> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is writable for `buf.len()` bytes.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    buf.truncate(len as usize);
    Ok(Some(PathBuf::from(OsString::from_vec(buf))))
}

fn missing_or<T>(err: io::Error) -> io::Result<Option<T>> {
    match err.raw_os_error() {
        Some(libc::ESRCH) | Some(libc::EINVAL) => Ok(None),
        _ => Err(err),
    }
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p agentdust-testkit --test platform`
Expected: `test result: ok. 5 passed`

- [ ] **Step 7: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/agentdust-core crates/agentdust-testkit
git commit -m "feat(core): read process identity on macOS" -m "Every later revalidation compares boot session, PID, start time and UID, so these reads come first and are tested against live child processes."
```

---

### Task 3: KERN_PROCARGS2 parser, environment reader and fuzz target

**Files:**
- Modify: `Cargo.toml` (exclude `fuzz`), `crates/agentdust-core/src/lib.rs`, `crates/agentdust-core/src/darwin.rs`, `crates/agentdust-testkit/tests/platform.rs`
- Create: `crates/agentdust-core/src/procargs.rs`, `fuzz/Cargo.toml`, `fuzz/fuzz_targets/procargs.rs`, `fuzz/seeds/procargs/*`
- Test: `crates/agentdust-core/tests/procargs.rs`, `crates/agentdust-core/tests/procargs_corpus.rs`

**Interfaces:**
- Consumes: `agentdust_core::darwin` from Task 2.
- Produces:
  - `agentdust_core::procargs::ProcArgs<'a> { exec_path: &'a [u8], args: Vec<&'a [u8]>, env: Vec<&'a [u8]> }`
  - `agentdust_core::procargs::ParseError { TooShort, NegativeArgc, Truncated }`
  - `agentdust_core::procargs::parse(buf: &[u8]) -> Result<ProcArgs<'_>, ParseError>`
  - `agentdust_core::procargs::env_value<'a>(parsed: &ProcArgs<'a>, name: &str) -> Option<&'a [u8]>`
  - `agentdust_core::darwin::procargs2(pid: i32) -> io::Result<Option<Vec<u8>>>`
  - `agentdust_core::darwin::env_var(pid: i32, name: &str) -> io::Result<Option<Vec<u8>>>`

- [ ] **Step 1: Write the failing parser test**

`crates/agentdust-core/tests/procargs.rs`:

```rust
use agentdust_core::procargs::{ParseError, env_value, parse};

fn buffer(argc: i32, exec_path: &[u8], padding: usize, strings: &[&[u8]]) -> Vec<u8> {
    let mut buf = argc.to_ne_bytes().to_vec();
    buf.extend_from_slice(exec_path);
    buf.push(0);
    buf.extend(std::iter::repeat_n(0, padding));
    for s in strings {
        buf.extend_from_slice(s);
        buf.push(0);
    }
    buf
}

#[test]
fn parses_exec_path_args_and_env_after_padding() {
    let buf = buffer(
        2,
        b"/usr/bin/node",
        3,
        &[b"node", b"server.js", b"HOME=/Users/a", b"AGENTDUST_SESSION=abc"],
    );
    let parsed = parse(&buf).unwrap();
    assert_eq!(parsed.exec_path, b"/usr/bin/node");
    assert_eq!(parsed.args, vec![&b"node"[..], &b"server.js"[..]]);
    assert_eq!(
        parsed.env,
        vec![&b"HOME=/Users/a"[..], &b"AGENTDUST_SESSION=abc"[..]]
    );
}

#[test]
fn env_value_matches_the_exact_name_only() {
    let buf = buffer(
        0,
        b"/bin/x",
        0,
        &[b"AGENTDUST_SESSION_OLD=no", b"AGENTDUST_SESSION=yes"],
    );
    let parsed = parse(&buf).unwrap();
    assert_eq!(env_value(&parsed, "AGENTDUST_SESSION"), Some(&b"yes"[..]));
    assert_eq!(env_value(&parsed, "AGENTDUST"), None);
}

#[test]
fn stops_at_the_first_empty_env_entry() {
    let mut buf = buffer(0, b"/bin/x", 0, &[b"A=1"]);
    buf.push(0);
    buf.extend_from_slice(b"B=2\0");
    assert_eq!(parse(&buf).unwrap().env, vec![&b"A=1"[..]]);
}

#[test]
fn ignores_a_final_entry_without_nul() {
    let mut buf = buffer(0, b"/bin/x", 0, &[b"A=1"]);
    buf.extend_from_slice(b"B=2");
    assert_eq!(parse(&buf).unwrap().env, vec![&b"A=1"[..]]);
}

#[test]
fn rejects_a_buffer_shorter_than_the_header() {
    assert_eq!(parse(&[1, 0]), Err(ParseError::TooShort));
    assert_eq!(parse(&[]), Err(ParseError::TooShort));
}

#[test]
fn rejects_negative_argc() {
    let buf = buffer(-1, b"/bin/x", 0, &[]);
    assert_eq!(parse(&buf), Err(ParseError::NegativeArgc));
}

#[test]
fn rejects_argc_larger_than_the_arguments_present() {
    let buf = buffer(5, b"/bin/x", 0, &[b"x"]);
    assert_eq!(parse(&buf), Err(ParseError::Truncated));
}

#[test]
fn rejects_an_exec_path_without_nul() {
    let mut buf = 0i32.to_ne_bytes().to_vec();
    buf.extend_from_slice(b"/bin/x");
    assert_eq!(parse(&buf), Err(ParseError::Truncated));
}

#[test]
fn huge_argc_does_not_allocate_unbounded_memory() {
    let buf = buffer(i32::MAX, b"/bin/x", 0, &[b"a"]);
    assert_eq!(parse(&buf), Err(ParseError::Truncated));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p agentdust-core --test procargs`
Expected: FAIL with `unresolved import agentdust_core::procargs`.

- [ ] **Step 3: Write the parser**

`crates/agentdust-core/src/procargs.rs`:

```rust
use thiserror::Error;

#[derive(Debug, PartialEq, Eq)]
pub struct ProcArgs<'a> {
    pub exec_path: &'a [u8],
    pub args: Vec<&'a [u8]>,
    pub env: Vec<&'a [u8]>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("buffer is shorter than the argc header")]
    TooShort,
    #[error("argc is negative")]
    NegativeArgc,
    #[error("buffer ends before all arguments were read")]
    Truncated,
}

pub fn parse(buf: &[u8]) -> Result<ProcArgs<'_>, ParseError> {
    let header: [u8; 4] = buf
        .get(..4)
        .and_then(|h| h.try_into().ok())
        .ok_or(ParseError::TooShort)?;
    let argc = usize::try_from(i32::from_ne_bytes(header)).map_err(|_| ParseError::NegativeArgc)?;
    let mut rest = &buf[4..];
    let exec_path = take_cstr(&mut rest).ok_or(ParseError::Truncated)?;
    while let [0, tail @ ..] = rest {
        rest = tail;
    }
    let mut args = Vec::with_capacity(argc.min(256));
    for _ in 0..argc {
        args.push(take_cstr(&mut rest).ok_or(ParseError::Truncated)?);
    }
    let mut env = Vec::new();
    while let Some(entry) = take_cstr(&mut rest) {
        if entry.is_empty() {
            break;
        }
        env.push(entry);
    }
    Ok(ProcArgs { exec_path, args, env })
}

pub fn env_value<'a>(parsed: &ProcArgs<'a>, name: &str) -> Option<&'a [u8]> {
    parsed
        .env
        .iter()
        .find_map(|entry| entry.strip_prefix(name.as_bytes())?.strip_prefix(b"="))
}

fn take_cstr<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let end = rest.iter().position(|&b| b == 0)?;
    let (value, tail) = rest.split_at(end);
    *rest = &tail[1..];
    Some(value)
}
```

`crates/agentdust-core/src/lib.rs`:

```rust
pub mod identity;
pub mod procargs;

#[cfg(target_os = "macos")]
pub mod darwin;
```

- [ ] **Step 4: Run the parser test to verify it passes**

Run: `cargo test -p agentdust-core --test procargs`
Expected: `test result: ok. 9 passed`

- [ ] **Step 5: Add the seed inputs and the seed regression test**

Run from the repository root:

```bash
mkdir -p fuzz/seeds/procargs && python3 - <<'EOF'
import pathlib
import struct

seeds_dir = pathlib.Path("fuzz/seeds/procargs")


def buf(argc, *parts, pad=0):
    out = struct.pack("<i", argc) + parts[0] + b"\0" + b"\0" * pad
    for part in parts[1:]:
        out += part + b"\0"
    return out


seeds = {
    "basic": buf(2, b"/usr/local/bin/node", b"node", b"server.js", b"HOME=/Users/a", b"AGENTDUST_SESSION=abc", pad=3),
    "no-env": buf(1, b"/bin/x", b"x"),
    "negative-argc": buf(-1, b"/bin/x"),
    "truncated": buf(9, b"/bin/x", b"only-one"),
    "header-only": struct.pack("<i", 0),
}
for name, data in seeds.items():
    (seeds_dir / name).write_bytes(data)
EOF
```

`crates/agentdust-core/tests/procargs_corpus.rs`:

```rust
use std::fs;
use std::path::Path;

use agentdust_core::procargs;

#[test]
fn every_corpus_entry_parses_without_panicking() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/procargs");
    let mut seen = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let data = fs::read(entry.unwrap().path()).unwrap();
        if let Ok(parsed) = procargs::parse(&data) {
            let _ = procargs::env_value(&parsed, "AGENTDUST_SESSION");
        }
        seen += 1;
    }
    assert!(seen >= 5, "expected the seeds in {}", dir.display());
}
```

Run: `cargo test -p agentdust-core --test procargs_corpus`
Expected: `test result: ok. 1 passed`

- [ ] **Step 6: Write the failing live environment tests**

Append to `crates/agentdust-testkit/tests/platform.rs`, and change its `use std::time::Duration;` line to `use std::time::{Duration, Instant};`:

```rust
#[test]
fn a_named_environment_variable_is_read_from_another_process() {
    let child = spawn_sleeper(&[("AGENTDUST_SESSION", "tag-123")]);
    let value = darwin::env_var(child.0.id() as i32, "AGENTDUST_SESSION").unwrap();
    assert_eq!(value.as_deref(), Some(&b"tag-123"[..]));
    assert_eq!(
        darwin::env_var(child.0.id() as i32, "AGENTDUST_ABSENT").unwrap(),
        None
    );
}

#[test]
fn the_tag_survives_reparenting_to_launchd() {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "AGENTDUST_SESSION=orphan-9 '{SLEEPER}' 30 </dev/null >/dev/null 2>&1 & echo $!"
        ))
        .output()
        .unwrap();
    let pid: i32 = String::from_utf8(output.stdout).unwrap().trim().parse().unwrap();
    let sleeper = Path::new(SLEEPER).canonicalize().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ppid = 0;
    while Instant::now() < deadline {
        ppid = darwin::process_info(pid, &boot()).unwrap().unwrap().ppid;
        let execed =
            darwin::exe_path(pid).unwrap().and_then(|p| p.canonicalize().ok()) == Some(sleeper.clone());
        if ppid == 1 && execed {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let value = darwin::env_var(pid, "AGENTDUST_SESSION").unwrap();
    unsafe { libc::kill(pid, libc::SIGTERM) };
    assert_eq!(ppid, 1);
    assert_eq!(value.as_deref(), Some(&b"orphan-9"[..]));
}

#[test]
fn launchd_arguments_are_not_readable() {
    assert_eq!(darwin::procargs2(1).unwrap(), None);
}
```

- [ ] **Step 7: Run them to verify they fail**

Run: `cargo test -p agentdust-testkit --test platform`
Expected: FAIL to compile with `cannot find function env_var in module darwin`.

- [ ] **Step 8: Add the environment reader**

Replace `crates/agentdust-core/src/darwin.rs` with:

```rust
use std::ffi::{CStr, OsString};
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use crate::identity::{KernelIdentity, ProcessInfo};
use crate::procargs;

pub fn boot_session_uuid() -> io::Result<String> {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: `buf` and `len` describe a writable buffer of `len` bytes.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let value = CStr::from_bytes_until_nul(&buf[..len]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "boot session UUID is not NUL terminated",
        )
    })?;
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "boot session UUID is not UTF-8"))
}

pub fn process_info(pid: i32, boot_session_uuid: &str) -> io::Result<Option<ProcessInfo>> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` points to `size` writable bytes for a proc_bsdinfo.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if written <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    if written != size {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short proc_bsdinfo"));
    }
    // SAFETY: the kernel filled all `size` bytes, checked above.
    let info = unsafe { info.assume_init() };
    Ok(Some(ProcessInfo {
        identity: KernelIdentity {
            boot_session_uuid: boot_session_uuid.to_owned(),
            pid,
            start_time_us: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            uid: info.pbi_uid,
        },
        ppid: info.pbi_ppid as i32,
        pgid: info.pbi_pgid as i32,
    }))
}

pub fn exe_path(pid: i32) -> io::Result<Option<PathBuf>> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is writable for `buf.len()` bytes.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    buf.truncate(len as usize);
    Ok(Some(PathBuf::from(OsString::from_vec(buf))))
}

pub fn procargs2(pid: i32) -> io::Result<Option<Vec<u8>>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut len: libc::size_t = 0;
    // SAFETY: a null buffer asks the kernel for the required size only.
    let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, ptr::null_mut(), &mut len, ptr::null_mut(), 0) };
    if rc != 0 {
        return missing_or(io::Error::last_os_error());
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is writable for `len` bytes.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return missing_or(io::Error::last_os_error());
    }
    buf.truncate(len);
    Ok(Some(buf))
}

pub fn env_var(pid: i32, name: &str) -> io::Result<Option<Vec<u8>>> {
    let Some(buf) = procargs2(pid)? else {
        return Ok(None);
    };
    let parsed = procargs::parse(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(procargs::env_value(&parsed, name).map(<[u8]>::to_vec))
}

fn missing_or<T>(err: io::Error) -> io::Result<Option<T>> {
    match err.raw_os_error() {
        Some(libc::ESRCH) | Some(libc::EINVAL) => Ok(None),
        _ => Err(err),
    }
}
```

- [ ] **Step 9: Run the live tests to verify they pass**

Run: `cargo test -p agentdust-testkit --test platform`
Expected: `test result: ok. 8 passed`. The reparenting test waits until the child has both PPID 1 and the sleeper as its executable, because until `exec` completes the child is still the Apple-protected `/bin/sh`, whose environment the kernel hides.

- [ ] **Step 10: Add the fuzz target**

In `Cargo.toml`, add this line under `resolver = "3"`:

```toml
exclude = ["fuzz"]
```

`fuzz/Cargo.toml`:

```toml
[package]
name = "agentdust-fuzz"
version = "0.0.0"
edition = "2024"
publish = false

[package.metadata]
cargo-fuzz = true

[dependencies]
libfuzzer-sys = "0.4"
agentdust-core = { path = "../crates/agentdust-core" }

[[bin]]
name = "procargs"
path = "fuzz_targets/procargs.rs"
test = false
doc = false
bench = false

[workspace]
members = ["."]
```

`fuzz/fuzz_targets/procargs.rs`:

```rust
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(parsed) = agentdust_core::procargs::parse(data) {
        let _ = agentdust_core::procargs::env_value(&parsed, "AGENTDUST_SESSION");
    }
});
```

Install once if missing: `rustup toolchain install nightly --profile minimal && cargo install cargo-fuzz --locked`

Run: `cargo +nightly fuzz build procargs`
Expected: `Finished` with no error.

Run: `mkdir -p fuzz/corpus/procargs && cargo +nightly fuzz run procargs fuzz/corpus/procargs fuzz/seeds/procargs -- -max_total_time=60`
Expected: a final line `Done <N> runs in 60 second(s)` and no crash file under `fuzz/artifacts`. The 2026-10-03 run reached 1.9 million runs in 21 seconds.

- [ ] **Step 11: Lint and commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

```bash
git add Cargo.toml crates/agentdust-core crates/agentdust-testkit fuzz/Cargo.toml fuzz/Cargo.lock fuzz/fuzz_targets fuzz/seeds
git commit -m "feat(core): read one environment variable from another process" -m "The Claude Code session tag survives reparenting to launchd only inside the process environment, so the parser for KERN_PROCARGS2 is the provenance primitive. It parses untrusted kernel output, so it ships with a fuzz target and seed inputs."
```

---

### Task 4: Journal

**Files:**
- Modify: `crates/agentdust-core/src/lib.rs`
- Create: `crates/agentdust-core/src/clock.rs`, `crates/agentdust-core/src/paths.rs`, `crates/agentdust-core/src/journal.rs`
- Test: `crates/agentdust-core/tests/clock.rs`, `crates/agentdust-core/tests/journal.rs`

**Interfaces:**
- Consumes: the core crate from Tasks 2 and 3.
- Produces:
  - `agentdust_core::clock::{wall_ms() -> u64, monotonic_ns() -> u64}`
  - `agentdust_core::paths::{DATA_DIR_ENV: &str = "AGENTDUST_DATA_DIR", data_dir() -> io::Result<PathBuf>}`
  - `agentdust_core::journal::{SCHEMA_VERSION: u32 = 1, LOCK_BUDGET: Duration = 20 ms}`
  - `agentdust_core::journal::Kind { SessionStart, SessionEnd, ShellStart, ShellEnd, Sample, ServerStart }` and `Agent { Claude, Codex, Cursor }`, serialized in snake case
  - `agentdust_core::journal::Record { v: u32, kind: Kind, agent: Agent, session_id: String, subagent_id: Option<String>, tool_use_id: Option<String>, wall_ts_ms: u64, mono_ns: u64, boot: String }`
  - `agentdust_core::journal::JournalError { LockBusy, Io(io::Error), Encode(serde_json::Error) }`
  - `agentdust_core::journal::ReadReport { records: Vec<Record>, skipped_lines: usize }`
  - `agentdust_core::journal::append(dir: &Path, record: &Record) -> Result<(), JournalError>`
  - `agentdust_core::journal::read(dir: &Path) -> io::Result<ReadReport>`

- [ ] **Step 1: Write the failing tests**

`crates/agentdust-core/tests/clock.rs`:

```rust
use agentdust_core::clock::{monotonic_ns, wall_ms};

#[test]
fn monotonic_clock_never_goes_backwards() {
    let first = monotonic_ns();
    let second = monotonic_ns();
    assert!(first > 0);
    assert!(second >= first);
}

#[test]
fn wall_clock_is_after_2026() {
    assert!(wall_ms() > 1_767_225_600_000);
}
```

`crates/agentdust-core/tests/journal.rs`:

```rust
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use agentdust_core::journal::{self, Agent, JournalError, Kind, Record, SCHEMA_VERSION};

fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agentdust-test-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn record(session: &str) -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::ShellStart,
        agent: Agent::Claude,
        session_id: session.to_owned(),
        subagent_id: None,
        tool_use_id: Some("toolu_1".to_owned()),
        wall_ts_ms: 1,
        mono_ns: 2,
        boot: "boot".to_owned(),
    }
}

#[test]
fn appended_records_read_back_in_order() {
    let dir = scratch_dir("roundtrip");
    journal::append(&dir, &record("a")).unwrap();
    journal::append(&dir, &record("b")).unwrap();
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records, vec![record("a"), record("b")]);
    assert_eq!(report.skipped_lines, 0);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn files_are_private_to_the_user() {
    let dir = scratch_dir("modes");
    journal::append(&dir, &record("a")).unwrap();
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.clone()), 0o700);
    assert_eq!(mode(dir.join("journal.jsonl")), 0o600);
    assert_eq!(mode(dir.join("journal.lock")), 0o600);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn malformed_and_future_lines_are_skipped() {
    let dir = scratch_dir("skip");
    journal::append(&dir, &record("a")).unwrap();
    let mut future = serde_json::to_value(record("b")).unwrap();
    future["v"] = serde_json::json!(2);
    let mut text = fs::read_to_string(dir.join("journal.jsonl")).unwrap();
    text.push_str(&format!("{future}\n{{\"truncated\": "));
    fs::write(dir.join("journal.jsonl"), text).unwrap();
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records, vec![record("a")]);
    assert_eq!(report.skipped_lines, 2);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_held_lock_fails_within_the_budget() {
    let dir = scratch_dir("busy");
    journal::append(&dir, &record("a")).unwrap();
    let holder = OpenOptions::new()
        .write(true)
        .open(dir.join("journal.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let start = Instant::now();
    let result = journal::append(&dir, &record("b"));
    assert!(matches!(result, Err(JournalError::LockBusy)));
    assert!(start.elapsed().as_millis() < 200);
    drop(holder);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_journal_is_refused() {
    let dir = scratch_dir("symlink");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("elsewhere");
    fs::write(&target, b"").unwrap();
    symlink(&target, dir.join("journal.jsonl")).unwrap();
    assert!(matches!(
        journal::append(&dir, &record("a")),
        Err(JournalError::Io(_))
    ));
    assert_eq!(fs::read(&target).unwrap(), b"");
    fs::remove_dir_all(&dir).unwrap();
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p agentdust-core --test clock --test journal`
Expected: FAIL with `unresolved import agentdust_core::clock` and `unresolved import agentdust_core::journal`.

- [ ] **Step 3: Write the implementation**

`crates/agentdust-core/src/clock.rs`:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

pub fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    (ts.tv_sec as u64) * 1_000_000_000 + ts.tv_nsec as u64
}
```

`crates/agentdust-core/src/paths.rs`:

```rust
use std::io;
use std::path::PathBuf;

pub const DATA_DIR_ENV: &str = "AGENTDUST_DATA_DIR";

pub fn data_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    let home =
        std::env::var_os("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library/Application Support/agentdust"))
}
```

`crates/agentdust-core/src/journal.rs`:

```rust
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCHEMA_VERSION: u32 = 1;
pub const LOCK_BUDGET: Duration = Duration::from_millis(20);
const JOURNAL_FILE: &str = "journal.jsonl";
const LOCK_FILE: &str = "journal.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SessionStart,
    SessionEnd,
    ShellStart,
    ShellEnd,
    Sample,
    ServerStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub v: u32,
    pub kind: Kind,
    pub agent: Agent,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub wall_ts_ms: u64,
    pub mono_ns: u64,
    pub boot: String,
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("journal lock stayed busy beyond the write budget")]
    LockBusy,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Encode(#[from] serde_json::Error),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    pub records: Vec<Record>,
    pub skipped_lines: usize,
}

pub fn append(dir: &Path, record: &Record) -> Result<(), JournalError> {
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let lock = open_private(&dir.join(LOCK_FILE), OpenMode::Lock)?;
    lock_within(&lock, LOCK_BUDGET)?;
    let mut journal = open_private(&dir.join(JOURNAL_FILE), OpenMode::Append)?;
    journal.write_all(&line)?;
    Ok(())
}

pub fn read(dir: &Path) -> io::Result<ReadReport> {
    let file = match File::open(dir.join(JOURNAL_FILE)) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ReadReport::default()),
        Err(err) => return Err(err),
    };
    let mut report = ReadReport::default();
    for line in BufReader::new(file).lines() {
        match serde_json::from_str::<Record>(&line?) {
            Ok(record) if record.v == SCHEMA_VERSION => report.records.push(record),
            _ => report.skipped_lines += 1,
        }
    }
    Ok(report)
}

enum OpenMode {
    Lock,
    Append,
}

fn open_private(path: &Path, mode: OpenMode) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
    match mode {
        OpenMode::Lock => options.write(true),
        OpenMode::Append => options.append(true),
    };
    options.open(path)
}

fn lock_within(file: &File, budget: Duration) -> Result<(), JournalError> {
    let start = Instant::now();
    loop {
        // SAFETY: the descriptor stays open for as long as `file` is borrowed.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(err.into());
        }
        if start.elapsed() >= budget {
            return Err(JournalError::LockBusy);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
```

`crates/agentdust-core/src/lib.rs`:

```rust
pub mod clock;
pub mod identity;
pub mod journal;
pub mod paths;
pub mod procargs;

#[cfg(target_os = "macos")]
pub mod darwin;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentdust-core --test clock --test journal`
Expected: `test result: ok. 2 passed` and `test result: ok. 5 passed`

- [ ] **Step 5: Lint and commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

```bash
git add crates/agentdust-core
git commit -m "feat(core): append journal records under a bounded lock" -m "Three agents can run hooks at the same moment, so writes go through flock on a separate lock file with a 20 ms budget, and a symlinked journal is refused instead of followed."
```

---

### Task 5: Claude Code hook

**Files:**
- Modify: `Cargo.toml` (members and one path dependency), `crates/agentdust/Cargo.toml`, `crates/agentdust/src/main.rs`
- Create: `crates/agentdust-agents/Cargo.toml`, `crates/agentdust-agents/src/lib.rs`, `crates/agentdust-agents/src/claude.rs`, `crates/agentdust/src/hook.rs`
- Test: `crates/agentdust-agents/tests/claude.rs`, `crates/agentdust/tests/common/mod.rs`, `crates/agentdust/tests/hook.rs`, `crates/agentdust/tests/hook_latency.rs`

**Interfaces:**
- Consumes: `agentdust_core::journal`, `clock`, `paths` (Task 4) and `darwin::boot_session_uuid` (Task 2).
- Produces:
  - `agentdust_agents::claude::MAX_ID_LEN: usize = 256`
  - `agentdust_agents::claude::HookEvent { session_id: String, hook_event_name: String, tool_use_id: Option<String>, agent_id: Option<String> }`
  - `agentdust_agents::claude::EventError { Json(serde_json::Error), FieldTooLong }`
  - `agentdust_agents::claude::parse_event(reader: impl Read) -> Result<HookEvent, EventError>`
  - `agentdust_agents::claude::journal_kind(event: &HookEvent) -> Option<Kind>`
  - Subcommand `agentdust hook claude`: reads one event from stdin, reads stdin to the end, appends at most one record, prints nothing, exits 0.

- [ ] **Step 1: Register the agents crate**

In `Cargo.toml`, replace the `members` line:

```toml
members = ["crates/agentdust-core", "crates/agentdust-agents", "crates/agentdust", "crates/agentdust-testkit"]
```

and add under `agentdust-core = ...` in `[workspace.dependencies]`:

```toml
agentdust-agents = { path = "crates/agentdust-agents" }
```

`crates/agentdust-agents/Cargo.toml`:

```toml
[package]
name = "agentdust-agents"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
agentdust-core.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
```

`crates/agentdust-agents/src/lib.rs`:

```rust
pub mod claude;
```

- [ ] **Step 2: Write the failing parser test**

`crates/agentdust-agents/tests/claude.rs`:

```rust
use agentdust_agents::claude::{EventError, HookEvent, MAX_ID_LEN, journal_kind, parse_event};
use agentdust_core::journal::Kind;

#[test]
fn reads_only_the_fields_it_needs() {
    let input = br#"{"session_id":"s1","hook_event_name":"PostToolUse","tool_use_id":"toolu_9",
        "tool_name":"Bash","tool_input":{"command":"ls"},"tool_response":{"stdout":"secret"}}"#;
    let event = parse_event(&input[..]).unwrap();
    assert_eq!(
        event,
        HookEvent {
            session_id: "s1".into(),
            hook_event_name: "PostToolUse".into(),
            tool_use_id: Some("toolu_9".into()),
            agent_id: None,
        }
    );
}

#[test]
fn maps_hook_events_to_journal_kinds() {
    let event = |name: &str| HookEvent {
        session_id: "s".into(),
        hook_event_name: name.into(),
        tool_use_id: None,
        agent_id: None,
    };
    assert_eq!(journal_kind(&event("SessionStart")), Some(Kind::SessionStart));
    assert_eq!(journal_kind(&event("SessionEnd")), Some(Kind::SessionEnd));
    assert_eq!(journal_kind(&event("PreToolUse")), Some(Kind::ShellStart));
    assert_eq!(journal_kind(&event("PostToolUse")), Some(Kind::ShellEnd));
    assert_eq!(journal_kind(&event("Stop")), None);
}

#[test]
fn rejects_identifiers_longer_than_the_limit() {
    let long = "s".repeat(MAX_ID_LEN + 1);
    let input = format!(r#"{{"session_id":"{long}","hook_event_name":"PreToolUse"}}"#);
    assert!(matches!(
        parse_event(input.as_bytes()),
        Err(EventError::FieldTooLong)
    ));
}

#[test]
fn rejects_an_event_without_session_id() {
    assert!(parse_event(&br#"{"hook_event_name":"PreToolUse"}"#[..]).is_err());
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p agentdust-agents --test claude`
Expected: FAIL to compile, because `src/claude.rs` does not exist.

- [ ] **Step 4: Write the parser**

`crates/agentdust-agents/src/claude.rs`:

```rust
use std::io::Read;

use agentdust_core::journal::Kind;
use serde::Deserialize;
use thiserror::Error;

pub const MAX_ID_LEN: usize = 256;

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct HookEvent {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum EventError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("an identifier is longer than {MAX_ID_LEN} bytes")]
    FieldTooLong,
}

pub fn parse_event(reader: impl Read) -> Result<HookEvent, EventError> {
    let event = HookEvent::deserialize(&mut serde_json::Deserializer::from_reader(reader))?;
    let ids = [
        Some(&event.session_id),
        event.tool_use_id.as_ref(),
        event.agent_id.as_ref(),
    ];
    if ids.into_iter().flatten().any(|id| id.len() > MAX_ID_LEN) {
        return Err(EventError::FieldTooLong);
    }
    Ok(event)
}

pub fn journal_kind(event: &HookEvent) -> Option<Kind> {
    match event.hook_event_name.as_str() {
        "SessionStart" => Some(Kind::SessionStart),
        "SessionEnd" => Some(Kind::SessionEnd),
        "PreToolUse" => Some(Kind::ShellStart),
        "PostToolUse" => Some(Kind::ShellEnd),
        _ => None,
    }
}
```

Run: `cargo test -p agentdust-agents --test claude`
Expected: `test result: ok. 4 passed`

- [ ] **Step 5: Write the failing hook tests**

`crates/agentdust/tests/common/mod.rs`:

```rust
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agentdust-bin-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

pub fn run_hook(data_dir: &Path, input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .args(["hook", "claude"])
        .env("AGENTDUST_DATA_DIR", data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

pub fn pre_tool_use(session: &str, tool_use_id: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"{session}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"{tool_use_id}","tool_input":{{"command":"npm run dev"}}}}"#
    )
    .into_bytes()
}
```

`crates/agentdust/tests/hook.rs`:

```rust
mod common;

use std::fs;
use std::thread;

use agentdust_core::journal::{self, Agent, Kind};
use common::{pre_tool_use, run_hook, scratch_dir};

#[test]
fn pre_tool_use_appends_one_shell_start_record() {
    let dir = scratch_dir("pre");
    let output = run_hook(&dir, &pre_tool_use("s1", "toolu_1"));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records.len(), 1);
    let record = &report.records[0];
    assert_eq!(record.kind, Kind::ShellStart);
    assert_eq!(record.agent, Agent::Claude);
    assert_eq!(record.session_id, "s1");
    assert_eq!(record.tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(record.boot.len(), 36);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn events_outside_the_journal_append_nothing() {
    let dir = scratch_dir("stop");
    let output = run_hook(&dir, br#"{"session_id":"s1","hook_event_name":"Stop"}"#);
    assert!(output.status.success());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn malformed_input_exits_zero_and_appends_nothing() {
    let dir = scratch_dir("malformed");
    let output = run_hook(&dir, b"not json at all");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn a_large_tool_response_is_read_but_never_stored() {
    let dir = scratch_dir("large");
    let output_text = "secret-output-".repeat(700_000);
    let input = format!(
        r#"{{"session_id":"s2","hook_event_name":"PostToolUse","tool_use_id":"toolu_2","tool_response":{{"stdout":"{output_text}"}}}}"#
    );
    assert!(input.len() > 9_000_000);
    let output = run_hook(&dir, input.as_bytes());
    assert!(output.status.success());
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records.len(), 1);
    assert_eq!(report.records[0].kind, Kind::ShellEnd);
    let stored = fs::read_to_string(dir.join("journal.jsonl")).unwrap();
    assert!(!stored.contains("secret-output"));
    assert!(!stored.contains("npm run dev"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn empty_stdin_exits_zero_and_appends_nothing() {
    let dir = scratch_dir("empty");
    let output = run_hook(&dir, b"");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn an_unwritable_data_dir_exits_zero_silently() {
    let dir = scratch_dir("unwritable");
    fs::write(&dir, b"a file, not a directory").unwrap();
    let output = run_hook(&dir, &pre_tool_use("s1", "toolu_1"));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    fs::remove_file(&dir).unwrap();
}

#[test]
fn an_oversized_session_id_is_not_recorded() {
    let dir = scratch_dir("oversized");
    let long = "s".repeat(10_000);
    let output = run_hook(&dir, &pre_tool_use(&long, "toolu_1"));
    assert!(output.status.success());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn concurrent_hooks_append_complete_records() {
    let dir = scratch_dir("concurrent");
    let handles: Vec<_> = (0..16)
        .map(|i| {
            let dir = dir.clone();
            thread::spawn(move || run_hook(&dir, &pre_tool_use(&format!("s{i}"), &format!("toolu_{i}"))))
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap().status.success());
    }
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.skipped_lines, 0);
    assert_eq!(report.records.len(), 16);
    fs::remove_dir_all(&dir).unwrap();
}
```

`crates/agentdust/tests/hook_latency.rs`:

```rust
mod common;

use std::fs;
use std::time::Instant;

use common::{pre_tool_use, run_hook, scratch_dir};

#[test]
#[ignore = "measures the real binary: cargo test --release -p agentdust --test hook_latency -- --ignored"]
fn hook_latency_is_within_budget() {
    let dir = scratch_dir("latency");
    let input = pre_tool_use("latency", "toolu_latency");
    for _ in 0..20 {
        assert!(run_hook(&dir, &input).status.success());
    }
    let mut samples: Vec<f64> = (0..200)
        .map(|_| {
            let start = Instant::now();
            assert!(run_hook(&dir, &input).status.success());
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    let p50 = samples[samples.len() / 2];
    let p95 = samples[samples.len() * 95 / 100];
    println!("hook latency p50 {p50:.2} ms, p95 {p95:.2} ms");
    fs::remove_dir_all(&dir).unwrap();
    assert!(p50 < 10.0, "p50 {p50:.2} ms");
    assert!(p95 < 20.0, "p95 {p95:.2} ms");
}
```

`crates/agentdust/Cargo.toml`:

```toml
[package]
name = "agentdust"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
agentdust-core.workspace = true
agentdust-agents.workspace = true
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test -p agentdust --test hook`
Expected: FAIL. `pre_tool_use_appends_one_shell_start_record` panics on `output.status.success()`, because `hook claude` is still an unknown command that exits 2.

- [ ] **Step 7: Write the hook**

`crates/agentdust/src/hook.rs`:

```rust
use std::error::Error;
use std::io::{self, BufReader};

use agentdust_agents::claude::{self, HookEvent};
use agentdust_core::journal::{self, Agent, Record, SCHEMA_VERSION};
use agentdust_core::{clock, darwin, paths};

pub fn run_claude() {
    let mut input = BufReader::new(io::stdin().lock());
    let event = claude::parse_event(&mut input);
    let _ = io::copy(&mut input, &mut io::sink());
    if let Ok(event) = event {
        let _ = record(&event);
    }
}

fn record(event: &HookEvent) -> Result<(), Box<dyn Error>> {
    let Some(kind) = claude::journal_kind(event) else {
        return Ok(());
    };
    let record = Record {
        v: SCHEMA_VERSION,
        kind,
        agent: Agent::Claude,
        session_id: event.session_id.clone(),
        subagent_id: event.agent_id.clone(),
        tool_use_id: event.tool_use_id.clone(),
        wall_ts_ms: clock::wall_ms(),
        mono_ns: clock::monotonic_ns(),
        boot: darwin::boot_session_uuid()?,
    };
    journal::append(&paths::data_dir()?, &record)?;
    Ok(())
}
```

`crates/agentdust/src/main.rs`:

```rust
mod hook;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["hook", "claude"] => {
            hook::run_claude();
            ExitCode::SUCCESS
        }
        ["version"] => {
            println!("agentdust {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: agentdust hook claude | agentdust version");
            ExitCode::from(2)
        }
    }
}
```

- [ ] **Step 8: Run the hook tests to verify they pass**

Run: `cargo test -p agentdust --test hook --test version`
Expected: `test result: ok. 8 passed` and `test result: ok. 2 passed`

- [ ] **Step 9: Measure the release binary**

Run: `cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture`
Expected: a line `hook latency p50 <x> ms, p95 <y> ms` with p50 under 10 and p95 under 20, then `test result: ok. 1 passed`. The 2026-10-04 run measured p50 2.56 ms and p95 3.15 ms.

- [ ] **Step 10: Lint and commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

```bash
git add Cargo.toml Cargo.lock crates/agentdust-agents crates/agentdust
git commit -m "feat(hook): record Claude Code shell events in the journal" -m "Hooks run on every Bash call, so the hook skips output fields without storing them, drains stdin so the host never sees a closed pipe, and always exits 0 even when the journal cannot be written."
```

---

### Task 6: MCP approval probe

**Files:**
- Modify: `Cargo.toml` (members and one path dependency), `crates/agentdust/Cargo.toml`, `crates/agentdust/src/main.rs`
- Create: `crates/agentdust-mcp/Cargo.toml`, `crates/agentdust-mcp/src/lib.rs`, `crates/agentdust-mcp/src/probe.rs`
- Test: `crates/agentdust-mcp/tests/codes.rs`, `crates/agentdust-mcp/tests/probe.rs`

**Interfaces:**
- Consumes: nothing from earlier crates.
- Produces:
  - `agentdust_mcp::serve_stdio() -> impl Future<Output = Result<(), Box<dyn Error + Send + Sync>>>`
  - `agentdust_mcp::ProbeServer` (implements `rmcp::ServerHandler`, `Default`, `Clone`)
  - `agentdust_mcp::probe::{TOOL_NAME = "agentdust_probe_approval", INPUT_KEY = "approval", CODE_ALPHABET: &[u8; 25], CODE_LEN = 4}`
  - `agentdust_mcp::probe::Outcome { Approved, WrongCode, Empty, Declined, Cancelled, Expired, Unsupported, Failed }`, serialized in snake case
  - `agentdust_mcp::probe::ProbeReport { outcome: Outcome, protocol: String, path: String }`, where `path` is `retry`, `legacy` or `none`
  - `agentdust_mcp::probe::{approval_message(code: &str) -> String, check_code(answer: Option<&str>, expected: &str) -> Outcome, new_code() -> io::Result<String>}`
  - Subcommand `agentdust mcp`: a stdio MCP server that exits when stdin closes.

Clients on protocol 2026-07-28 get an `InputRequiredResult` and retry with the answer. Older clients get a server-initiated elicitation through `peer.elicit_with_timeout`. A client connects on 2026-07-28 only through `ClientLifecycleMode::Discover`; plain `serve()` uses the `initialize` handshake, which tops out at 2025-11-25.

- [ ] **Step 1: Register the MCP crate**

In `Cargo.toml`, replace the `members` line:

```toml
members = ["crates/agentdust-core", "crates/agentdust-agents", "crates/agentdust-mcp", "crates/agentdust", "crates/agentdust-testkit"]
```

and add under `agentdust-agents = ...` in `[workspace.dependencies]`:

```toml
agentdust-mcp = { path = "crates/agentdust-mcp" }
```

`crates/agentdust-mcp/Cargo.toml`:

```toml
[package]
name = "agentdust-mcp"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
rmcp.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true

[dev-dependencies]
rmcp = { workspace = true, features = ["client"] }
tokio = { workspace = true, features = ["rt-multi-thread"] }
```

- [ ] **Step 2: Write the failing tests**

`crates/agentdust-mcp/tests/codes.rs`:

```rust
use agentdust_mcp::probe::{CODE_ALPHABET, CODE_LEN, Outcome, check_code, new_code};

#[test]
fn codes_use_the_unambiguous_alphabet() {
    for _ in 0..200 {
        let code = new_code().unwrap();
        assert_eq!(code.len(), CODE_LEN);
        assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)), "{code}");
    }
}

#[test]
fn only_an_exact_code_approves() {
    assert_eq!(check_code(Some("AC3K"), "AC3K"), Outcome::Approved);
    assert_eq!(check_code(Some("ac3k"), "AC3K"), Outcome::WrongCode);
    assert_eq!(check_code(Some(" AC3K"), "AC3K"), Outcome::WrongCode);
    assert_eq!(check_code(Some(""), "AC3K"), Outcome::Empty);
    assert_eq!(check_code(None, "AC3K"), Outcome::Empty);
}
```

`crates/agentdust-mcp/tests/probe.rs`:

```rust
use agentdust_mcp::ProbeServer;
use agentdust_mcp::probe::{INPUT_KEY, Outcome, ProbeReport, TOOL_NAME};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, ClientConfig,
    ElicitRequestParams, ElicitResult, ElicitationAction, Implementation, InputRequest, InputResponses,
    ProtocolVersion,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RequestContext, RoleClient, RunningService};
use rmcp::{ClientHandler, ErrorData, ServiceExt};
use serde_json::json;

#[derive(Clone)]
enum Reply {
    EchoCode,
    Code(&'static str),
    Decline,
    Cancel,
}

#[derive(Clone)]
struct ScriptedClient {
    reply: Reply,
    protocol: ProtocolVersion,
    elicitation: bool,
}

impl ScriptedClient {
    fn new(protocol: ProtocolVersion, reply: Reply) -> Self {
        Self {
            reply,
            protocol,
            elicitation: true,
        }
    }
}

fn code_in(message: &str) -> String {
    message.split_whitespace().nth(1).unwrap().to_owned()
}

impl ClientHandler for ScriptedClient {
    fn get_info(&self) -> ClientConfig {
        let capabilities = if self.elicitation {
            ClientCapabilities::builder().enable_elicitation().build()
        } else {
            ClientCapabilities::default()
        };
        ClientConfig::new(capabilities, Implementation::new("scripted-client", "0.0.0"))
            .with_protocol_version(self.protocol.clone())
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let ElicitRequestParams::FormElicitationParams { message, .. } = &request else {
            return Ok(ElicitResult::new(ElicitationAction::Decline));
        };
        Ok(match &self.reply {
            Reply::EchoCode => {
                ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "code": code_in(message) }))
            }
            Reply::Code(code) => {
                ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "code": code }))
            }
            Reply::Decline => ElicitResult::new(ElicitationAction::Decline),
            Reply::Cancel => ElicitResult::new(ElicitationAction::Cancel),
        })
    }
}

async fn connect(client: ScriptedClient) -> RunningService<RoleClient, ScriptedClient> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let server = ProbeServer::default().serve(server_io).await.unwrap();
        let _ = server.waiting().await;
    });
    if client.protocol == ProtocolVersion::V_2026_07_28 {
        let lifecycle = ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        };
        client.serve_with_lifecycle(client_io, lifecycle).await.unwrap()
    } else {
        client.serve(client_io).await.unwrap()
    }
}

fn report_of(result: &CallToolResult) -> ProbeReport {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

async fn probe(client: ScriptedClient) -> ProbeReport {
    let client = connect(client).await;
    let result = client
        .call_tool(CallToolRequestParams::new(TOOL_NAME))
        .await
        .unwrap();
    client.cancel().await.unwrap();
    report_of(&result)
}

const RETRY: ProtocolVersion = ProtocolVersion::V_2026_07_28;
const LEGACY: ProtocolVersion = ProtocolVersion::V_2025_06_18;

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_client_with_the_right_code_is_approved() {
    let report = probe(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    assert_eq!(report.outcome, Outcome::Approved);
    assert_eq!(report.path, "retry");
    assert_eq!(report.protocol, "2026-07-28");
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_client_with_a_wrong_code_is_refused() {
    let report = probe(ScriptedClient::new(RETRY, Reply::Code("ZZZZ"))).await;
    assert_eq!(report.outcome, Outcome::WrongCode);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_client_with_an_empty_code_is_refused() {
    let report = probe(ScriptedClient::new(RETRY, Reply::Code(""))).await;
    assert_eq!(report.outcome, Outcome::Empty);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_decline_and_cancel_are_reported() {
    assert_eq!(
        probe(ScriptedClient::new(RETRY, Reply::Decline)).await.outcome,
        Outcome::Declined
    );
    assert_eq!(
        probe(ScriptedClient::new(RETRY, Reply::Cancel)).await.outcome,
        Outcome::Cancelled
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_client_with_the_right_code_is_approved() {
    let report = probe(ScriptedClient::new(LEGACY, Reply::EchoCode)).await;
    assert_eq!(report.outcome, Outcome::Approved);
    assert_eq!(report.path, "legacy");
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_decline_wrong_and_empty_codes_are_refused() {
    assert_eq!(
        probe(ScriptedClient::new(LEGACY, Reply::Decline)).await.outcome,
        Outcome::Declined
    );
    assert_eq!(
        probe(ScriptedClient::new(LEGACY, Reply::Code("ZZZZ")))
            .await
            .outcome,
        Outcome::WrongCode
    );
    assert_eq!(
        probe(ScriptedClient::new(LEGACY, Reply::Code(""))).await.outcome,
        Outcome::Empty
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_without_elicitation_is_unsupported() {
    for protocol in [RETRY, LEGACY] {
        let mut client = ScriptedClient::new(protocol, Reply::EchoCode);
        client.elicitation = false;
        assert_eq!(probe(client).await.outcome, Outcome::Unsupported);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_request_state_is_rejected() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    let first = client
        .call_tool_once(CallToolRequestParams::new(TOOL_NAME))
        .await
        .unwrap();
    let CallToolResponse::InputRequired(required) = first else {
        panic!("expected an input request");
    };
    let state = required.request_state.clone().unwrap();
    let Some(InputRequest::Elicitation(elicit)) = required.input_requests.unwrap().remove(INPUT_KEY) else {
        panic!("expected an elicitation");
    };
    let ElicitRequestParams::FormElicitationParams { message, .. } = elicit.params else {
        panic!("expected a form elicitation");
    };
    let answer =
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "code": code_in(&message) }));
    let responses: InputResponses = [(INPUT_KEY.to_owned(), serde_json::to_value(answer).unwrap())].into();
    let retry = CallToolRequestParams::new(TOOL_NAME)
        .with_request_state(state)
        .with_input_responses(responses);
    let CallToolResponse::Complete(result) = client.call_tool_once(retry.clone()).await.unwrap() else {
        panic!("expected a final result");
    };
    assert_eq!(report_of(&result).outcome, Outcome::Approved);
    assert!(client.call_tool_once(retry).await.is_err());
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_request_state_is_rejected() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    let forged = CallToolRequestParams::new(TOOL_NAME).with_request_state("00ff");
    assert!(client.call_tool_once(forged).await.is_err());
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_tool_is_an_error() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    assert!(
        client
            .call_tool_once(CallToolRequestParams::new("agentdust_apply"))
            .await
            .is_err()
    );
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retry_without_the_approval_response_is_rejected() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    let CallToolResponse::InputRequired(required) = client
        .call_tool_once(CallToolRequestParams::new(TOOL_NAME))
        .await
        .unwrap()
    else {
        panic!("expected an input request");
    };
    let state = required.request_state.unwrap();
    let wrong_key: InputResponses = [(
        "other".to_owned(),
        serde_json::to_value(ElicitResult::new(ElicitationAction::Accept)).unwrap(),
    )]
    .into();
    let retry = CallToolRequestParams::new(TOOL_NAME)
        .with_request_state(state)
        .with_input_responses(wrong_key);
    assert!(client.call_tool_once(retry).await.is_err());
    client.cancel().await.unwrap();
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p agentdust-mcp`
Expected: FAIL to compile, because `src/lib.rs` does not exist.

- [ ] **Step 4: Write the probe server**

`crates/agentdust-mcp/src/lib.rs`:

```rust
pub mod probe;

use rmcp::ServiceExt;

pub use probe::ProbeServer;

pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = ProbeServer::default().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

`crates/agentdust-mcp/src/probe.rs`:

```rust
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequest,
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema, Implementation, InputRequest,
    InputRequests, InputRequiredResult, JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::schemars::JsonSchema;
use rmcp::service::{ElicitationError, RequestContext, RoleServer, ServiceError};
use rmcp::{ErrorData, ServerHandler, elicit_safe};
use serde::{Deserialize, Serialize};

pub const TOOL_NAME: &str = "agentdust_probe_approval";
pub const INPUT_KEY: &str = "approval";
pub const CODE_ALPHABET: &[u8; 25] = b"ACDEFGHJKMNPQRTUVWXY34679";
pub const CODE_LEN: usize = 4;
const CODE_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ApprovalCode {
    #[schemars(description = "The code shown in the message")]
    pub code: String,
}

elicit_safe!(ApprovalCode);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Approved,
    WrongCode,
    Empty,
    Declined,
    Cancelled,
    Expired,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub outcome: Outcome,
    pub protocol: String,
    pub path: String,
}

struct Pending {
    code: String,
    expires: Instant,
}

#[derive(Clone, Default)]
pub struct ProbeServer {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl ServerHandler for ProbeServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("agentdust", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![probe_tool()]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != TOOL_NAME {
            return Err(ErrorData::invalid_params(
                format!("unknown tool {}", request.name),
                None,
            ));
        }
        let protocol = context.protocol_version();
        let supports_form = context
            .client_capabilities()
            .and_then(|caps| caps.elicitation)
            .is_some_and(|e| e.form.is_some() || e.url.is_none());
        if !supports_form {
            return Ok(report(Outcome::Unsupported, protocol.as_ref(), "none"));
        }
        let retry_based = protocol
            .as_ref()
            .is_some_and(|v| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str());
        if retry_based {
            self.call_retry_based(request, protocol.as_ref())
        } else {
            Ok(self.call_legacy(&context, protocol.as_ref()).await)
        }
    }
}

impl ProbeServer {
    fn call_retry_based(
        &self,
        request: CallToolRequestParams,
        protocol: Option<&ProtocolVersion>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(nonce) = request.request_state else {
            let code = new_code().map_err(internal)?;
            let nonce = hex(&random_bytes::<16>().map_err(internal)?);
            let message = approval_message(&code);
            self.pending.lock().expect("pending map lock").insert(
                nonce.clone(),
                Pending {
                    code,
                    expires: Instant::now() + CODE_TTL,
                },
            );
            let schema = ElicitationSchema::from_type::<ApprovalCode>().map_err(internal)?;
            let mut requests = InputRequests::new();
            requests.insert(
                INPUT_KEY.to_owned(),
                InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message,
                    requested_schema: schema,
                })),
            );
            return Ok(InputRequiredResult::new(Some(requests), Some(nonce)).into());
        };
        let pending = self
            .pending
            .lock()
            .expect("pending map lock")
            .remove(&nonce)
            .ok_or_else(|| ErrorData::invalid_params("unknown or already used request state", None))?;
        if Instant::now() > pending.expires {
            return Ok(report(Outcome::Expired, protocol, "retry"));
        }
        let response = request
            .input_responses
            .as_ref()
            .and_then(|responses| responses.get(INPUT_KEY))
            .ok_or_else(|| ErrorData::invalid_params("missing approval response", None))?;
        let response = ElicitResult::deserialize(response)
            .map_err(|_| ErrorData::invalid_params("invalid approval response", None))?;
        let outcome = match response.action {
            ElicitationAction::Accept => check_code(
                response
                    .content
                    .as_ref()
                    .and_then(|c| c.get("code"))
                    .and_then(|v| v.as_str()),
                &pending.code,
            ),
            ElicitationAction::Decline => Outcome::Declined,
            ElicitationAction::Cancel => Outcome::Cancelled,
            _ => Outcome::Failed,
        };
        Ok(report(outcome, protocol, "retry"))
    }

    async fn call_legacy(
        &self,
        context: &RequestContext<RoleServer>,
        protocol: Option<&ProtocolVersion>,
    ) -> CallToolResponse {
        let Ok(code) = new_code() else {
            return report(Outcome::Failed, protocol, "legacy");
        };
        let answer = context
            .peer
            .elicit_with_timeout::<ApprovalCode>(approval_message(&code), Some(CODE_TTL))
            .await;
        let outcome = match answer {
            Ok(Some(answer)) => check_code(Some(&answer.code), &code),
            Ok(None) | Err(ElicitationError::NoContent) => Outcome::Empty,
            Err(ElicitationError::UserDeclined) => Outcome::Declined,
            Err(ElicitationError::UserCancelled) => Outcome::Cancelled,
            Err(ElicitationError::CapabilityNotSupported) => Outcome::Unsupported,
            Err(ElicitationError::Service(ServiceError::Timeout { .. })) => Outcome::Expired,
            Err(_) => Outcome::Failed,
        };
        report(outcome, protocol, "legacy")
    }
}

pub fn approval_message(code: &str) -> String {
    format!("Type {code} to approve this agentdust probe. Nothing will be changed.")
}

pub fn check_code(answer: Option<&str>, expected: &str) -> Outcome {
    match answer {
        None | Some("") => Outcome::Empty,
        Some(answer) if answer == expected => Outcome::Approved,
        Some(_) => Outcome::WrongCode,
    }
}

fn report(outcome: Outcome, protocol: Option<&ProtocolVersion>, path: &str) -> CallToolResponse {
    let report = ProbeReport {
        outcome,
        protocol: protocol.map(|p| p.as_str().to_owned()).unwrap_or_default(),
        path: path.to_owned(),
    };
    let text = serde_json::to_string(&report).expect("probe report serializes");
    CallToolResult::success(vec![ContentBlock::text(text)]).into()
}

fn probe_tool() -> Tool {
    let mut schema = JsonObject::new();
    schema.insert("type".into(), "object".into());
    schema.insert("properties".into(), JsonObject::new().into());
    let mut tool = Tool::new(
        TOOL_NAME,
        "Asks the user to approve with a typed code. Changes nothing; used to test approval prompts.",
        Arc::new(schema),
    );
    tool.annotations = Some(ToolAnnotations::new().read_only(true).destructive(false));
    tool
}

pub fn new_code() -> io::Result<String> {
    let mut code = String::with_capacity(CODE_LEN);
    while code.len() < CODE_LEN {
        for byte in random_bytes::<8>()? {
            if code.len() < CODE_LEN && byte < 250 {
                code.push(CODE_ALPHABET[usize::from(byte) % CODE_ALPHABET.len()] as char);
            }
        }
    }
    Ok(code)
}

fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn internal(err: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(err.to_string(), None)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p agentdust-mcp`
Expected: `test result: ok. 2 passed` and `test result: ok. 11 passed`

- [ ] **Step 6: Wire the subcommand**

`crates/agentdust/Cargo.toml`:

```toml
[package]
name = "agentdust"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
agentdust-core.workspace = true
agentdust-agents.workspace = true
agentdust-mcp.workspace = true
tokio.workspace = true
```

`crates/agentdust/src/main.rs`:

```rust
mod hook;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["hook", "claude"] => {
            hook::run_claude();
            ExitCode::SUCCESS
        }
        ["mcp"] => mcp(),
        ["version"] => {
            println!("agentdust {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: agentdust hook claude | agentdust mcp | agentdust version");
            ExitCode::from(2)
        }
    }
}

fn mcp() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("agentdust mcp: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(agentdust_mcp::serve_stdio()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("agentdust mcp: {err}");
            ExitCode::FAILURE
        }
    }
}
```

Run: `cargo test -p agentdust --test version --test hook`
Expected: `test result: ok. 2 passed` and `test result: ok. 8 passed`

Run:

```bash
cargo build -p agentdust && printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' '{"jsonrpc":"2.0","method":"notifications/initialized"}' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | target/debug/agentdust mcp | grep -c agentdust_probe_approval
```

Expected: `1`, and the command returns because the server exits when stdin closes.

- [ ] **Step 7: Run the whole workspace and commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check && cargo deny check`
Expected: every `test result` line reports `ok`, clippy and fmt print nothing, and cargo deny prints `advisories ok, bans ok, licenses ok, sources ok`.

```bash
git add Cargo.toml Cargo.lock crates/agentdust-mcp crates/agentdust
git commit -m "feat(mcp): add a typed-code approval probe" -m "Approval for apply depends on how each client renders and answers a form, and the 2026-07-28 protocol replaced server requests with a retry round trip. The probe changes nothing and records which path each client takes, with a single-use nonce for the retry path."
```

---

### Task 7: Publish the repository and run CI

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the workspace from Tasks 1 to 6.
- Produces: the public repository `hamzahamidi/agentdust`, CI checks named `linux` and `macos`, and a protected `main`.

- [ ] **Step 1: Recheck the name**

Run:

```bash
printf "crates.io agentdust: "; curl -s -o /dev/null -w '%{http_code}\n' -A agentdust-name-check https://crates.io/api/v1/crates/agentdust; brew info --formula agentdust 2>&1 | head -1; gh repo view hamzahamidi/agentdust --json name 2>&1 | head -1
```

Expected: `404` for the crate name, `No available formula with the name "agentdust"`, and `Could not resolve to a Repository`. If any of them is taken, stop and ask the user for a name before going further.

- [ ] **Step 2: Check the commit identity**

Run: `git log --format='%ae' | sort -u && git grep -il thefork; echo "grep exit $?"`
Expected: only `22576950+hamzahamidi@users.noreply.github.com`, then `grep exit 1` (no match).

- [ ] **Step 3: Write the CI workflow**

`.github/workflows/ci.yml`:

```yaml
name: ci

on:
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ci-${{ github.head_ref || github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_TOOLCHAIN: "1.99.0"

jobs:
  linux:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - run: rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal -c clippy -c rustfmt
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --exclude agentdust --all-targets --locked -- -D warnings
      - run: cargo test --workspace --exclude agentdust --locked
      - uses: taiki-e/install-action@861a07ce7084f55488e375df125cdc99bba60eb7 # v2.87.23
        with:
          tool: cargo-deny,cargo-audit,cargo-fuzz
      - run: cargo deny check
      - run: cargo audit
      - run: rustup toolchain install nightly --profile minimal
      - run: cargo +nightly fuzz build procargs

  macos:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - run: rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal -c clippy
      - run: cargo clippy --workspace --all-targets --locked -- -D warnings
      - run: cargo test --workspace --locked
      - run: cargo test --release -p agentdust --test hook_latency --locked -- --ignored --nocapture
```

Run: `ruby -ryaml -e 'YAML.load_file(".github/workflows/ci.yml"); puts "ok"'`
Expected: `ok`

```bash
git add .github/workflows/ci.yml
git commit -m "ci: run lint, tests, dependency policy and the hook budget" -m "Linux covers every platform-independent crate and the fuzz build for free; one macOS job covers the live process tests and measures the release hook latency."
```

- [ ] **Step 4: Create the repository (ask the user first)**

Ask: "Create the public repository hamzahamidi/agentdust and push main?" Continue only on yes.

```bash
gh repo create hamzahamidi/agentdust --public --description "Finds and cleans processes that AI coding agents leave behind" --source . --remote origin
gh api -X PATCH repos/hamzahamidi/agentdust -F delete_branch_on_merge=true -F allow_squash_merge=true -F allow_merge_commit=false -F allow_rebase_merge=false -f squash_merge_commit_title=PR_TITLE -f squash_merge_commit_message=PR_BODY
gh api -X PUT repos/hamzahamidi/agentdust/vulnerability-alerts
gh api -X PUT repos/hamzahamidi/agentdust/private-vulnerability-reporting
```

Expected: the repository URL, then three calls without error output.

- [ ] **Step 5: Push main and open the pull request**

The design commits already on `main` go first; the M0 work goes on a branch.

```bash
git push -u origin main
git push -u origin m0/spike
gh pr create --draft --head m0/spike --title "feat: add the M0 risk spike" --body "Adds the workspace, process identity and environment reading on macOS, the Claude Code hook with its journal, the MCP approval probe and CI. Verify with the linux and macos checks; the macOS log prints the release hook latency."
```

If a push fails with a connection refused on port 22, run `git config core.sshCommand "ssh -p 443 -o Hostname=ssh.github.com"` and push again.

Expected: a pull request URL.

- [ ] **Step 6: Wait for CI**

Run: `gh pr checks --watch`
Expected: `linux` and `macos` both pass.

Run: `gh run view "$(gh run list --workflow ci.yml --branch m0/spike --limit 1 --json databaseId --jq '.[0].databaseId')" --log | grep "hook latency"`
Expected: the CI latency line. Copy it into the M0 report in Task 10.

- [ ] **Step 7: Protect main**

```bash
gh api -X PUT repos/hamzahamidi/agentdust/branches/main/protection --input - <<'EOF'
{"required_status_checks":{"strict":true,"contexts":["linux","macos"]},"enforce_admins":false,"required_pull_request_reviews":null,"restrictions":null}
EOF
```

Expected: a JSON response that contains `"contexts":["linux","macos"]`.

- [ ] **Step 8: Merge (ask the user first)**

Ask: "CI is green. Mark ready and squash-merge the M0 pull request?" Continue only on yes.

```bash
gh pr ready && gh pr merge --squash
```

---

### Task 8: Release dry run and Homebrew install

**Files:**
- Create: `scripts/package.py`, `scripts/compare_builds.py`, `scripts/toolchain.py`, `scripts/formula.py`, `scripts/brew_smoke.py`, `.github/workflows/release-dry-run.yml`
- Create after the first run: `release/toolchain.json`

**Interfaces:**
- Consumes: the merged `main` from Task 7. `workflow_dispatch` only runs workflows that exist on the default branch, so this task merges before the dry run.
- Produces: a dry-run workflow that builds twice, compares the binaries, writes a deterministic tarball, `SHA256SUMS` and an attestation, and installs the tarball through a local Homebrew tap.

- [ ] **Step 1: Start a branch**

```bash
git switch main && git pull --ff-only && git switch -c m0/release-dry-run
```

- [ ] **Step 2: Write the scripts**

`scripts/package.py`:

```python
import argparse
import gzip
import hashlib
import io
import os
import tarfile
from pathlib import Path


def add_file(tar: tarfile.TarFile, path: Path, arcname: str, mode: int, mtime: int) -> None:
    data = path.read_bytes()
    info = tarfile.TarInfo(arcname)
    info.size = len(data)
    info.mode = mode
    info.mtime = mtime
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    tar.addfile(info, io.BytesIO(data))


def main() -> None:
    parser = argparse.ArgumentParser(description="Create a reproducible agentdust release tarball.")
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", default="darwin-arm64")
    parser.add_argument("--out-dir", required=True, type=Path)
    args = parser.parse_args()

    mtime = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    name = f"agentdust-{args.version}-{args.target}"
    files = [
        (args.binary, f"{name}/agentdust", 0o755),
        (Path("LICENSE"), f"{name}/LICENSE", 0o644),
        (Path("README.md"), f"{name}/README.md", 0o644),
    ]
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for path, arcname, mode in sorted(files, key=lambda entry: entry[1]):
            add_file(tar, path, arcname, mode, mtime)

    args.out_dir.mkdir(parents=True, exist_ok=True)
    out = args.out_dir / f"{name}.tar.gz"
    with out.open("wb") as handle:
        with gzip.GzipFile(filename="", mode="wb", fileobj=handle, mtime=0, compresslevel=9) as gz:
            gz.write(raw.getvalue())
    print(f"{hashlib.sha256(out.read_bytes()).hexdigest()}  {out.name}")


if __name__ == "__main__":
    main()
```

`scripts/compare_builds.py`:

```python
import argparse
import hashlib
import json
import sys
from pathlib import Path


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description="Compare two independent release builds.")
    parser.add_argument("first", type=Path)
    parser.add_argument("second", type=Path)
    args = parser.parse_args()

    binaries = [args.first / "target/release/agentdust", args.second / "target/release/agentdust"]
    toolchains = [json.loads((d / "toolchain.json").read_text()) for d in (args.first, args.second)]
    hashes = [digest(b) for b in binaries]
    for path, value in zip(binaries, hashes):
        print(f"{value}  {path}")
    failures = []
    if toolchains[0] != toolchains[1]:
        failures.append("the two builds recorded different toolchains")
    if hashes[0] != hashes[1]:
        failures.append("the two binaries differ")
    for failure in failures:
        print(failure, file=sys.stderr)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
```

`scripts/toolchain.py`:

```python
import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path


def output(*command: str) -> str:
    result = subprocess.run(command, capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else "unavailable"


def record() -> dict:
    return {
        "runner_image": f"{os.environ.get('ImageOS', 'local')} {os.environ.get('ImageVersion', '')}".strip(),
        "rustc": output("rustc", "-Vv"),
        "cargo": output("cargo", "-V"),
        "developer_dir": output("xcode-select", "-p"),
        "xcode": output("xcodebuild", "-version"),
        "sdk_version": output("xcrun", "--show-sdk-version"),
        "sdk_path": output("xcrun", "--show-sdk-path"),
        "cargo_lock_sha256": hashlib.sha256(Path("Cargo.lock").read_bytes()).hexdigest(),
        "rustflags": os.environ.get("RUSTFLAGS", ""),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Record or check the release toolchain.")
    parser.add_argument("mode", choices=["record", "check"])
    parser.add_argument("--expected", type=Path, default=Path("release/toolchain.json"))
    parser.add_argument("--allow-missing", action="store_true")
    args = parser.parse_args()

    current = record()
    if args.mode == "record":
        print(json.dumps(current, indent=2, sort_keys=True))
        return
    if args.allow_missing and not args.expected.exists():
        print(f"no expected toolchain at {args.expected}; nothing to compare")
        return
    expected = json.loads(args.expected.read_text())
    keys = sorted(set(expected) - {"cargo_lock_sha256"})
    drift = [key for key in keys if expected[key] != current.get(key)]
    for key in drift:
        print(f"toolchain drift in {key}:\n  expected: {expected[key]}\n  current:  {current.get(key)}", file=sys.stderr)
    sys.exit(1 if drift else 0)


if __name__ == "__main__":
    main()
```

`scripts/formula.py`:

```python
import argparse
import hashlib
from pathlib import Path

TEMPLATE = """class Agentdust < Formula
  desc "Finds and cleans processes that AI coding agents leave behind"
  homepage "https://github.com/hamzahamidi/agentdust"
  url "{url}"
  version "{version}"
  sha256 "{sha256}"

  depends_on arch: :arm64
  depends_on :macos

  def install
    bin.install "agentdust"
  end

  test do
    assert_match "agentdust #{{version}}", shell_output("#{{bin}}/agentdust version")
  end
end
"""


def main() -> None:
    parser = argparse.ArgumentParser(description="Write the Homebrew formula for one agentdust tarball.")
    parser.add_argument("--tarball", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--url", required=True)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    sha256 = hashlib.sha256(args.tarball.read_bytes()).hexdigest()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(TEMPLATE.format(url=args.url, version=args.version, sha256=sha256))
    print(args.out)


if __name__ == "__main__":
    main()
```

`scripts/brew_smoke.py`:

```python
import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

TAP = "agentdust-local/m0"


def run(*command: str) -> str:
    print("+", " ".join(command), flush=True)
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout.strip()


def main() -> None:
    parser = argparse.ArgumentParser(description="Install a agentdust formula from a local tap and run it.")
    parser.add_argument("formula", type=Path)
    parser.add_argument("version")
    args = parser.parse_args()

    os.environ["HOMEBREW_NO_AUTO_UPDATE"] = "1"
    os.environ["HOMEBREW_NO_INSTALL_CLEANUP"] = "1"
    tap_dir = Path(run("brew", "--repository")) / "Library/Taps/agentdust-local/homebrew-m0/Formula"
    try:
        run("brew", "tap-new", "--no-git", TAP)
        tap_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy(args.formula, tap_dir / "agentdust.rb")
        run("brew", "trust", "--formula", f"{TAP}/agentdust")
        run("brew", "install", f"{TAP}/agentdust")
        reported = run(str(Path(run("brew", "--prefix")) / "bin/agentdust"), "version")
        expected = f"agentdust {args.version}"
        print(reported)
        if reported != expected:
            sys.exit(f"expected {expected!r}, got {reported!r}")
    finally:
        subprocess.run(["brew", "uninstall", "--formula", "agentdust"], capture_output=True)
        subprocess.run(["brew", "untap", TAP], capture_output=True)


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Check the scripts locally**

Run:

```bash
cargo build --release -p agentdust && export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) && python3 scripts/package.py --binary target/release/agentdust --version 0.0.0 --out-dir dist/a && python3 scripts/package.py --binary target/release/agentdust --version 0.0.0 --out-dir dist/b
```

Expected: two identical lines `<sha256>  agentdust-0.0.0-darwin-arm64.tar.gz`.

Run: `python3 scripts/formula.py --tarball dist/a/agentdust-0.0.0-darwin-arm64.tar.gz --version 0.0.0 --url "file://$PWD/dist/a/agentdust-0.0.0-darwin-arm64.tar.gz" --out dist/agentdust.rb && ruby -c dist/agentdust.rb`
Expected: `dist/agentdust.rb` then `Syntax OK`.

Run: `python3 scripts/toolchain.py check --allow-missing`
Expected: `no expected toolchain at release/toolchain.json; nothing to compare`

- [ ] **Step 4: Write the dry-run workflow**

`.github/workflows/release-dry-run.yml`:

```yaml
name: release-dry-run

on:
  workflow_dispatch:

permissions:
  contents: read

env:
  RUST_TOOLCHAIN: "1.99.0"
  VERSION: "0.0.0"

jobs:
  build:
    strategy:
      matrix:
        copy: [a, b]
    runs-on: macos-15
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - run: rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
      - run: python3 scripts/toolchain.py check --allow-missing
      - run: python3 scripts/toolchain.py record > toolchain.json
      - run: cargo build --release --locked -p agentdust
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: build-${{ matrix.copy }}
          path: |
            target/release/agentdust
            toolchain.json
          retention-days: 3

  package:
    needs: build
    runs-on: macos-15
    permissions:
      contents: read
      id-token: write
      attestations: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
        with:
          path: builds
      - run: python3 scripts/compare_builds.py builds/build-a builds/build-b
      - run: SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)" python3 scripts/package.py --binary builds/build-a/target/release/agentdust --version "$VERSION" --out-dir dist
      - run: shasum -a 256 dist/*.tar.gz > dist/SHA256SUMS
      - uses: actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8 # v4.2.2
        with:
          subject-path: |
            builds/build-a/target/release/agentdust
            dist/*.tar.gz
      - run: python3 scripts/formula.py --tarball "dist/agentdust-$VERSION-darwin-arm64.tar.gz" --version "$VERSION" --url "file://$PWD/dist/agentdust-$VERSION-darwin-arm64.tar.gz" --out dist/agentdust.rb
      - run: python3 scripts/brew_smoke.py dist/agentdust.rb "$VERSION"
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: release-dry-run
          path: |
            dist
            builds/build-a/toolchain.json
          retention-days: 3
```

Run: `ruby -ryaml -e 'YAML.load_file(".github/workflows/release-dry-run.yml"); puts "ok"'`
Expected: `ok`

- [ ] **Step 5: Commit, open the pull request and merge (ask the user before merging)**

```bash
git add scripts .github/workflows/release-dry-run.yml
git commit -m "ci: add a reproducible release dry run with a Homebrew install" -m "M0 has to prove that two clean builds match, that the tarball and attestation can be produced, and that Homebrew installs the result, before any release design is frozen."
git push -u origin HEAD
gh pr create --draft --title "ci: add the release dry run" --body "Adds the packaging, comparison, toolchain, formula and Homebrew smoke scripts and a workflow_dispatch dry run that uses them. Verify by running the workflow on main after merge."
gh pr checks --watch
```

Ask: "CI is green. Squash-merge the release dry-run pull request?" On yes: `gh pr ready && gh pr merge --squash`

- [ ] **Step 6: Run the dry run on main**

Run: `gh workflow run release-dry-run.yml --ref main`

Run: `gh run list --workflow release-dry-run.yml --limit 1` and repeat until the run started by the previous command is listed, then `gh run watch` with its ID.

Expected: `build (a)`, `build (b)` and `package` succeed. The `compare_builds.py` step prints the same SHA-256 twice, and `brew_smoke.py` prints `agentdust 0.0.0`. If a step fails, keep its log output for the Blockers section of the M0 report; M0 accepts a written blocker.

- [ ] **Step 7: Lock the toolchain**

```bash
git switch main && git pull --ff-only && git switch -c m0/toolchain-lock
gh run download "$(gh run list --workflow release-dry-run.yml --limit 1 --json databaseId --jq '.[0].databaseId')" -n release-dry-run -D /tmp/agentdust-dry-run
mkdir -p release && cp /tmp/agentdust-dry-run/builds/build-a/toolchain.json release/toolchain.json
git add release/toolchain.json
git commit -m "build: lock the release toolchain recorded by the first dry run" -m "Later releases must fail when the runner image, Xcode, SDK or Rust version drifts, and that check needs a recorded baseline."
```

This commit goes into the Task 10 pull request.

---

### Task 9: Client matrix and Cursor experiments

These steps need a person at the keyboard: the probe asks for a typed code in each client's own interface.

**Files:**
- Create: `docs/m0/client-matrix.md`, `docs/m0/cursor-experiments.md`, `scripts/m0/cursor_probe.py`

**Interfaces:**
- Consumes: `target/release/agentdust` built from `main` (`cargo build --release -p agentdust`).
- Produces: the recorded matrix and Cursor answers that M3 and M5 depend on.

- [ ] **Step 1: Add the templates and the probe hook**

`docs/m0/client-matrix.md`:

```markdown
# M0 client matrix

How each MCP client renders and answers the typed-code approval form of `agentdust_probe_approval`. One row per client and scenario. The tool result is the JSON the probe returns.

Probe build: `agentdust version` output and commit SHA:

| Client | Version | Scenario | Prompt shown | Tool result (`outcome`, `protocol`, `path`) | Notes |
| --- | --- | --- | --- | --- | --- |
| Claude Code | | correct code | | | |
| Claude Code | | wrong code | | | |
| Claude Code | | empty answer | | | |
| Claude Code | | decline | | | |
| Claude Code | | cancel (Esc or close) | | | |
| Claude Code | | no answer for 2 minutes | | | |
| Codex CLI | | correct code | | | |
| Codex CLI | | wrong code | | | |
| Codex CLI | | empty answer | | | |
| Codex CLI | | decline | | | |
| Codex CLI | | cancel | | | |
| Codex CLI | | no answer for 2 minutes | | | |
| Codex CLI | | "always allow" set for the tool | | | |
| Cursor | | correct code | | | |
| Cursor | | wrong code | | | |
| Cursor | | empty answer | | | |
| Cursor | | decline | | | |
| Cursor | | cancel | | | |
| Cursor | | no answer for 2 minutes | | | |

## Verdict per client

| Client | Typed-code approval works | Auto-accept risk observed | Decision for M3 |
| --- | --- | --- | --- |
| Claude Code | | | |
| Codex CLI | | | |
| Cursor | | | |
```

`docs/m0/cursor-experiments.md`:

```markdown
# M0 Cursor experiments

Two questions from section 10 of the design spec, answered with `scripts/m0/cursor_probe.py`. Raw entries are in `~/Library/Application Support/agentdust-m0/cursor-probe.jsonl`.

Cursor version:

## 1. Does `sessionStart` env reach shell processes?

| Check | Result |
| --- | --- |
| `AGENTDUST_M0_TAG` printed by `env` in an agent shell command | |
| `tag_in_hook_env` in the `afterShellExecution` entry | |

Answer:

## 2. Which process anchors a Cursor conversation?

| Event | Ancestry (pid and command, nearest first) |
| --- | --- |
| `sessionStart` | |
| `afterShellExecution` | |

| Check | Result |
| --- | --- |
| Lowest common ancestor of both chains | |
| Is that process still alive after the conversation is closed? | |
| Is the `sleep 600 &` child still alive after the conversation is closed, and what is its parent? | |

Answer:
```

`scripts/m0/cursor_probe.py`:

```python
import json
import os
import secrets
import subprocess
import sys
import time
from pathlib import Path

LOG = Path.home() / "Library/Application Support/agentdust-m0/cursor-probe.jsonl"
TAG = "AGENTDUST_M0_TAG"


def ancestry(pid: int) -> list[dict]:
    chain = []
    while pid > 1 and len(chain) < 12:
        line = subprocess.run(["ps", "-o", "ppid=,comm=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
        if not line:
            break
        ppid, comm = line.split(None, 1)
        chain.append({"pid": pid, "comm": comm})
        pid = int(ppid)
    return chain


def main() -> None:
    event = sys.argv[1] if len(sys.argv) > 1 else "unknown"
    payload = json.loads(sys.stdin.read() or "{}")
    LOG.parent.mkdir(parents=True, exist_ok=True)
    entry = {
        "ts": time.time(),
        "event": event,
        "session_id": payload.get("session_id"),
        "duration": payload.get("duration"),
        "tag_in_hook_env": os.environ.get(TAG),
        "ancestry": ancestry(os.getpid()),
    }
    with LOG.open("a") as handle:
        handle.write(json.dumps(entry) + "\n")
    if event == "sessionStart":
        print(json.dumps({"env": {TAG: f"probe-{secrets.token_hex(4)}"}}))


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Ask before touching agent configuration**

Ask: "Register the probe as an MCP server in Claude Code, Codex and Cursor, and add a temporary hook to Cursor? Each change is removed at the end of this task." Continue only on yes.

- [ ] **Step 3: Claude Code**

```bash
claude mcp add --scope user agentdust-probe -- "$PWD/target/release/agentdust" mcp
```

In a new Claude Code session, ask: "Call the agentdust_probe_approval tool and show its result." Repeat once per scenario row: type the shown code, type a wrong code, submit an empty answer, decline, cancel, and leave the prompt for more than 2 minutes. Copy each tool result into the table.

```bash
claude mcp remove --scope user agentdust-probe
```

- [ ] **Step 4: Codex CLI**

```bash
codex mcp add agentdust-probe -- "$PWD/target/release/agentdust" mcp
```

Run the same scenarios in a new `codex` session, plus one run with "always allow" set for the tool. Copy each result into the table.

```bash
codex mcp remove agentdust-probe
```

- [ ] **Step 5: Cursor MCP**

Check first: `ls ~/.cursor/mcp.json ~/.cursor/hooks.json`. On 2026-10-03 neither file existed. If one exists, copy it to `/tmp` and restore it in Step 7.

```bash
python3 - "$PWD/target/release/agentdust" <<'EOF'
import json
import pathlib
import sys

path = pathlib.Path.home() / ".cursor/mcp.json"
config = json.loads(path.read_text()) if path.exists() else {}
config.setdefault("mcpServers", {})["agentdust-probe"] = {"command": sys.argv[1], "args": ["mcp"]}
path.write_text(json.dumps(config, indent=2) + "\n")
EOF
```

Run the same scenarios in a Cursor agent chat and copy each result into the table.

- [ ] **Step 6: Cursor experiments**

```bash
python3 - "$PWD/scripts/m0/cursor_probe.py" <<'EOF'
import json
import pathlib
import sys

path = pathlib.Path.home() / ".cursor/hooks.json"
config = json.loads(path.read_text()) if path.exists() else {"version": 1, "hooks": {}}
hooks = config.setdefault("hooks", {})
for event in ("sessionStart", "afterShellExecution"):
    hooks.setdefault(event, []).append({"command": f"python3 {sys.argv[1]} {event}"})
path.write_text(json.dumps(config, indent=2) + "\n")
EOF
```

Restart Cursor, open a new agent chat, and ask it to run `env | grep AGENTDUST_M0_TAG` and then `sleep 600 &`. Close the chat. Then run `ps -o pid,ppid,command -p "$(pgrep -f 'sleep 600')"` and read `~/Library/Application Support/agentdust-m0/cursor-probe.jsonl`. Fill in both sections of `docs/m0/cursor-experiments.md`, and stop the sleeper with `pkill -f 'sleep 600'`.

- [ ] **Step 7: Remove every change**

```bash
python3 - <<'EOF'
import json
import pathlib

home = pathlib.Path.home()
mcp = home / ".cursor/mcp.json"
if mcp.exists():
    config = json.loads(mcp.read_text())
    config.get("mcpServers", {}).pop("agentdust-probe", None)
    mcp.unlink() if config == {"mcpServers": {}} else mcp.write_text(json.dumps(config, indent=2) + "\n")
hooks = home / ".cursor/hooks.json"
if hooks.exists():
    config = json.loads(hooks.read_text())
    for event in ("sessionStart", "afterShellExecution"):
        entries = [e for e in config.get("hooks", {}).get(event, []) if "cursor_probe.py" not in e.get("command", "")]
        if entries:
            config["hooks"][event] = entries
        else:
            config.get("hooks", {}).pop(event, None)
    hooks.unlink() if config == {"version": 1, "hooks": {}} else hooks.write_text(json.dumps(config, indent=2) + "\n")
EOF
claude mcp list | grep -c agentdust-probe; codex mcp list | grep -c agentdust-probe; ls ~/.cursor/mcp.json ~/.cursor/hooks.json 2>&1
```

Expected: `0`, `0`, and `No such file or directory` for both Cursor files (or the restored originals).

- [ ] **Step 8: Commit**

```bash
git switch m0/toolchain-lock
git add docs/m0/client-matrix.md docs/m0/cursor-experiments.md scripts/m0/cursor_probe.py
git commit -m "docs: record the M0 client matrix and Cursor experiments" -m "M3 decides per client whether apply is allowed, and M5 decides whether Cursor gets an env tag and an ended-agent upgrade. Both decisions read these records."
```

---

### Task 10: M0 report

**Files:**
- Create: `docs/m0/report.md`
- Modify: `ROADMAP.md` (status line)

**Interfaces:**
- Consumes: CI and dry-run logs (Tasks 7 and 8), the matrix and experiments (Task 9).
- Produces: the M0 verdict and the inputs for the M1 plan.

- [ ] **Step 1: Write the report**

`docs/m0/report.md`, filled in with a link to each run or file the result comes from:

```markdown
# M0 report

Results of the M0 risk spike against the exit criteria in [ROADMAP.md](../../ROADMAP.md). Each line links the run or file it was read from.

| Exit criterion | Result | Evidence |
| --- | --- | --- |
| Hook p50 under 10 ms on the release binary | | |
| Elicitation matrix recorded for Claude Code, Codex and Cursor | | [client-matrix.md](client-matrix.md) |
| Two clean builds produce a byte-identical binary | | |
| Tarball, checksum and attestation produced | | |
| Homebrew formula installs the tarball and `agentdust version` runs | | |
| Cursor questions answered | | [cursor-experiments.md](cursor-experiments.md) |

## Blockers

## Inputs for M1
```

- [ ] **Step 2: Update the roadmap status**

In `ROADMAP.md`, change `Status: planning, no code yet.` to `Status: M0 complete, M1 next.` if every exit criterion passed, or to `Status: M0 complete with blockers listed in docs/m0/report.md.` otherwise.

- [ ] **Step 3: Commit, open the pull request and merge (ask the user before merging)**

```bash
git add docs/m0/report.md ROADMAP.md
git commit -m "docs: report the M0 results" -m "The M1 plan starts from these numbers and decisions, so they are recorded with links to the runs that produced them."
git push -u origin HEAD
gh pr create --draft --title "docs: report the M0 results" --body "Adds the toolchain lock, the client matrix, the Cursor experiments and the M0 report, and updates the roadmap status."
gh pr checks --watch
```

Ask: "Squash-merge the M0 report pull request?" On yes: `gh pr ready && gh pr merge --squash`
