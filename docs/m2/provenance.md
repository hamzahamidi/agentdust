# Provenance: the agent identity, the session tag and session scopes

This page describes what the Claude Code hook records about the agent behind a session, and how `agentdust_core::session` turns the journal into session scopes. Spec sections 3.1 to 3.3 and 4.1 to 4.4 hold the design. The last sections list where the code differs from the spec and what is not built. Nothing here signals a process. The classifier and `doctor` read these results ([doctor](doctor.md)), and `apply` is separate work.

## What the hook writes

Only four events are journaled. Every other event and every other tool writes nothing and creates nothing.

| Event | Record kind | `agent_identity` | `session_tag_key` | Line in `CLAUDE_ENV_FILE` |
| --- | --- | --- | --- | --- |
| `SessionStart` | `session_start` | when an agent is found | when the install secret is usable | when the record was appended and the variable names a usable file |
| `PreToolUse` for `Bash` | `shell_start` | when an agent is found | never | never |
| `PostToolUse` for `Bash` | `shell_end` | when an agent is found | never | never |
| `SessionEnd` | `session_end` | when an agent is found | never | never |

The hook still exits 0 and prints nothing in every case below. A failed step costs the evidence it would have written and nothing else.

## The agent identity

`agent_identity` is a nested object with four keys, written in this order. `exe_base` is left out when the executable name cannot be stored.

```
"agent_identity":{"pid":4242,"start_time_us":1800000000000000,"uid":501,"exe_base":"claude"}
```

| Key | Rule |
| --- | --- |
| `pid` | 1 to 2,147,483,647 |
| `start_time_us` | unsigned 64-bit, microseconds since the epoch |
| `uid` | unsigned 32-bit |
| `exe_base` | at most 64 bytes, no C0, C1 or bidi controls. It is the basename of the executable path |

The widest identity encodes to 150 bytes, and `MAX_AGENT_IDENTITY_LEN` is 160. The path of the executable is not stored, because it holds the user name. The boot session UUID is not stored in the identity either: it is the `boot` field of the same record, and `AgentIdentity::kernel(boot)` rebuilds the `KernelIdentity` of spec 3.1. A line that breaks a rule is malformed on read, as for `exe_base` and `cwd_key`. A key inside the identity that is not in `journal::AGENT_IDENTITY_KEYS` fails the privacy test.

## Finding the agent

`ancestry::find_agent(provider, start)` starts at the parent of the hook and walks up. It returns the first process that is an agent:

- Its executable basename is exactly `claude`. The comparison is on bytes and case sensitive, so the desktop application `Claude`, `claude-code`, `claudex` and a path that ends in a slash do not match.
- Or its executable path ends in `claude/versions/<version>`, the layout of the native installer, with exactly those two directory names and a non-empty last component.
- Or its executable basename is exactly `node` and its script argument path contains `claude`.

The script argument is the first argument after the program name that does not start with `-`. The value of `-r`, `--require`, `--import`, `--loader`, `--experimental-loader`, `-C` and `--conditions` is skipped. An argument after `--` is the script. `-e`, `--eval`, `-p` and `--print` mean the code is on the command line and there is no script. `procargs::script_argument` parses only the argument part of the `KERN_PROCARGS2` buffer. The environment part is never read, and the arguments of a process are read only when its executable is `node`.

| Situation | Result |
| --- | --- |
| A matching process is found | Its identity. Nothing above it is read |
| The chain reaches PID 1 or below, or a PID it has seen | No agent |
| 64 processes were examined | No agent |
| An ancestor has gone, cannot be read, or was replaced between the read and the lookup of its parent | No agent |
| An ancestor whose executable path cannot be read | Passed. Its parent is looked up through its kernel identity |
| A `node` whose arguments cannot be read, or whose script does not mention `claude` | Passed |

Every link is verified. The provider reads a process twice around its path, so a PID reused during the read is not reported as present. The walk then asks for the parent of that exact kernel identity (boot session, PID, start time and user), and the answer is `None` when any field differs. A recycled PID therefore ends the walk and never leads it to another process.

An agent that is not found leaves `agent_identity` out. The session is still journaled and its scope is degraded (see below).

## The session tag

The tag is 16 bytes from `getentropy`, written as 32 lowercase hexadecimal characters. Its `Debug` form prints `SessionTag(redacted)`. `session_tag_key` is the HMAC-SHA256 of those 32 characters under the install secret, with the domain `AGENTDUST-SESSION-v1` and a NUL byte in front, which is the digest a sample of a process environment will compute from the variable it reads. The raw tag is written in one place only, the file `CLAUDE_ENV_FILE` names.

The hook works in this order for a `SessionStart`: load the install secret, draw the tag, compute its key, append the record, then write the line. A tag is therefore handed out only after its key is in the journal. The line is `export AGENTDUST_SESSION=<tag>` and a newline, preceded by a newline when the file is not empty, so a last line without a newline is not joined to it. The write is one `write(2)` call, so racing appends stay whole lines.

| Rule for the file `CLAUDE_ENV_FILE` names | Result |
| --- | --- |
| The variable is unset or empty | No line. The record still carries the key |
| The path is relative | Refused |
| The last component is a symlink, dangling or not | Refused, and the target is not touched or created |
| The file has another hard link, is not a regular file, or has another owner | Refused. A FIFO is refused without blocking |
| The directory is missing | Refused, and the directory is not created |
| The file is missing | Created with mode 0600 |
| The file exists | Appended to. Its mode is not changed and does not matter, because Claude Code made it |

| What fails | Effect |
| --- | --- |
| The install secret is missing, malformed or unusable | No `session_tag_key`, no `cwd_key`, no line. The secret is left alone |
| The entropy call fails | No key and no line |
| The journal append fails | No line |
| The environment file cannot be used | The record keeps its key, and no line is written |

The `AGENTDUST_SESSION` variable of the hook process itself is never read.

## Session scopes

`session::scopes(&records, &probe)` takes records in journal order and a `LivenessProbe` and returns the scopes in the order they were opened. It is a pure function: it reads no file and calls the probe once for each distinct kernel identity.

A scope belongs to one agent and one session ID. These rules decide which scope a record joins:

- The first record of a session opens a scope, with the identity of the record if it has one.
- A record with an identity joins the scope with that kernel identity (boot, PID, start time, user). When no scope has it, a new scope opens. This is the resumed scope of spec 3.3. A late record of an old agent joins its own old scope.
- A record without an identity joins the latest scope. A `session_start` without an identity does the same when the latest scope has no identity, and opens a scope of its own when it has one. A scope never receives an identity it did not start with.
- A record with a `subagent_id` joins the scope of its identity, or the latest scope, and never opens one. It adds its ID to `subagent_ids`. It never ends the session and never adds a tag key.
- A repeated `session_start` with the same identity opens nothing. It adds its tag key to the scope.

| State | When |
| --- | --- |
| `Ended` | A `session_end` record joined the scope, or the probe says the recorded process is gone |
| `Active` | Not ended, and a `session_start` joined the scope or the probe says the process is alive |
| `Unknown` | Anything else |

`Ended` is absorbing: records added to the journal never move a scope out of it, and a late event of an ended scope does not reopen it. A property test checks this over random records. Two fields keep the two ways of ending apart, because a `session_end` alone never makes a process actionable (spec 3.3):

| `Scope` field | Meaning |
| --- | --- |
| `session_ended` | A `session_end` record joined the scope |
| `liveness` | `Some(Alive)`, `Some(Gone)` or `Some(Unknown)` when the scope has an identity, `None` when it has none |
| `agent_gone()` | `liveness` is `Some(Gone)`. The classifier needs this and not `state` for owned-ended |
| `degraded()` | The scope has no identity, so no liveness evidence is possible |
| `tag_keys`, `subagent_ids`, `exe_base`, `identity` | Collected from the records of the scope |

`ProviderLiveness` is the probe over a `ProcessProvider`:

| Fresh read | Liveness |
| --- | --- |
| Present or path unreadable, with the same boot, PID, start time and user | `Alive` |
| Present or path unreadable, with any of them different | `Gone` |
| The process has gone | `Gone` |
| The read failed, or the PID is 0 or below | `Unknown`. It never ends a scope |

A changed executable path with the same kernel identity is still `Alive`: the process is the same one after an `exec`, and declaring a live session ended is the unsafe error.

## Cost

The identity walk reads at most 64 processes, and `node` ancestors cost one more `sysctl`. On the machine of the measurement, an interleaved run of 400 hooks of the previous release binary and of this one gave the same p50 and p95 within the noise of the load, which was 50 to 63 on 10 cores:

| Event | Previous binary p50 / p95 | This binary p50 / p95 |
| --- | ---: | ---: |
| `PreToolUse` | 8.58 / 25.32 ms and 9.19 / 26.41 ms | 8.50 / 25.53 ms and 8.84 / 27.46 ms |
| `SessionStart` with an env file | 9.74 / 24.76 ms and 8.47 / 25.02 ms | 9.51 / 27.48 ms and 8.62 / 26.25 ms |

Each cell is one of two runs. At that load the previous binary is over the 20 ms p95 budget as well, so these numbers show the added cost, under 0.5 ms at p50, and do not show whether the budget holds on a quiet machine. The ignored release test `cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture --test-threads=1` prints the three cases of the hook: an event with a cwd, an event without one, and `session_start_latency_is_within_budget` for a `SessionStart` with a tag and an env file. The result of three runs at a load of 52 to 65 was p50 5.4 to 8.0 ms and p95 15.9 to 24.4 ms across the three cases. M1 measured p50 2.2 ms at a load of 6.

## Where this differs from the spec

- Spec 3.2 lists `agent_identity` as an `AgentIdentity` with kind, kernel identity and session. The record stores pid, start time, user and executable basename. Kind, session and subagent are fields of the record, and the boot is the `boot` field.
- Spec 3.1 says a signal needs every field of the kernel identity and the executable path. The liveness check for a session scope compares the kernel identity only, so a process that changed its executable by `exec` stays alive. Signals will use `revalidate`.
- Spec 4.1 says known executables start from `setup` and are refreshed by every genuine adapter invocation. The hook uses the fixed rule above. `setup` does not exist yet.
- Spec 3.3 says a duplicate `session_start` with the same identity is ignored. It opens no scope and changes no state. Its tag key is added to the scope, because a process started after the second start can carry either tag.
- The hook records `session_tag_key` on every `SessionStart` whose secret is usable, also when the variable `CLAUDE_ENV_FILE` is missing. The key then matches no process.
- A missing environment file is created with mode 0600, as a shell `>>` would.
- The hook records `agent_identity` on `shell_start`, `shell_end` and `session_end` as well as on `session_start`, which is what the spec means by any event with a live agent identity (3.3).

## Not built here

- The known executable list of `setup`. A Claude Code installed with the native installer runs from `~/.local/share/claude/versions/<version>`, which `~/.local/bin/claude` points to on the development machine, and the layout rule finds it. An installation in another layout under another name is journaled with no identity, and its sessions are degraded. The Claude Code of the desktop application runs as `claude` and is found, and so is a `node` process whose script path mentions `claude`.
- A hook wrapper that is itself a `node` script with `claude` in its path would be taken for the agent, and would end before it, which makes a live session look ended. The tests do not cover this case.
- `health.json`: the per-agent hook health record of spec 4.4. A degraded scope is derived from the journal.
- Sampling and `apply`. `session::scopes` is called by `doctor` and by nothing else.

## Tests

| Subject | Files |
| --- | --- |
| Identity and tag key fields: bounds, order, round trip | `crates/agentdust-core/tests/journal_agent_identity.rs`, `crates/agentdust-core/tests/journal_record.rs`, `crates/agentdust-core/tests/journal_keys.rs`, `crates/agentdust-core/tests/journal_codec_props.rs` |
| Tag, key and env file append | `crates/agentdust-core/tests/tag.rs`, `crates/agentdust-core/tests/tag_env_file.rs` |
| Ancestry walk and the script argument | `crates/agentdust-core/tests/ancestry.rs`, `crates/agentdust-core/tests/procargs_script.rs`, `crates/agentdust-core/tests/procargs_corpus.rs`, `fuzz/fuzz_targets/procargs.rs` |
| Darwin provider on live processes | `crates/agentdust-testkit/tests/ancestry_live.rs` |
| Session scopes: table, probe and properties | `crates/agentdust-core/tests/session.rs`, `crates/agentdust-core/tests/session_liveness.rs`, `crates/agentdust-core/tests/session_props.rs` |
| The hook binary under a chain of processes | `crates/agentdust/tests/hook_session.rs`, `crates/agentdust/tests/common/chain.rs` |
| Privacy of the tag and of the agent path | `crates/agentdust/tests/privacy.rs` |
| Latency | `crates/agentdust/tests/hook_latency.rs` |

The hook tests run the real binary under copies of the test executable named `claude`, `node` and `relay`. Each copy reports its own identity to the test, so the test compares the recorded identity with the process that wrote it. A relay that is reparented to launchd before it starts the hook gives a chain with no agent whatever runs the test.
