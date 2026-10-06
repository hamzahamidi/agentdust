# Plan and apply

This page describes the library and client adapters behind `agentdust_plan` and `agentdust_apply`: how a plan is made, how a person approves items with typed codes, and how one approved item is revalidated and sent a single SIGTERM. Spec sections 6.1 to 6.5 hold the design, and S1 to S11 and S21 of section 9.1 are the safety requirements it serves. The last sections list where the code differs from the spec and what remains outside this release.

The protocol, plan store and execution logic are in `agentdust-core`. `agentdust-mcp` exposes doctor, plan and apply, using form elicitation when the client supports it. The terminal command `agentdust apply` uses the same server and asks for the same code through `/dev/tty`.

## The parts

| Part | Where | Job |
| --- | --- | --- |
| `Surveyor` | `survey.rs` | A fresh inventory, classified: `survey()` returns every process with its class, and `describe()` gives the model view of one finding. `LiveSurveyor` (`live.rs`, macOS) reads the secret, the journal, the process list and the launchd list on every call and keeps nothing between calls |
| `PlanStore` | `plan.rs` | Plans in memory under random IDs, their expiry, the claim on each item and the inspection report |
| `Server` | `apply/server.rs` | The two phase approval protocol (`begin`, `answer`, and `run` for a caller that can wait) over a plan store and an executor |
| `Executor` | `apply/exec.rs` | One approved item: lock, fresh inventory, reclassification, identity read and SIGTERM, wait, audit |
| `IdentityLock` | `apply/lock.rs` | A non-blocking `flock` per identity |
| `AuditLog` | `apply/audit.rs` | JSON lines, 0600, bounded |
| `Signaller` | `apply/signal.rs` | The only way to signal. It has one method, `sigterm(pid)`. `KillSignaller` is the real one |
| `Timer` | `apply/timer.rs` | A monotonic clock and a sleep, so expiry and the wait run in tests without waiting |
| `code` | `code.rs` | The approval code, shared with the M0 probe |
| `config` | `config.rs` | The `apply` switch of `config.toml` |

The traits `Surveyor`, `Signaller` and `Timer`, and the `ProcessProvider` that already existed, are the seams. Every guard is tested with scripted implementations that signal nothing, and the live tests use the real process list, the real provider and a signaller that forwards to the harness.

## Plan

`Server::plan()` takes a fresh survey and keeps the findings that `PlanItem::plannable` accepts: the class is owned-ended or suspect (the actionability of `class::actionable_as`), the PID is above 1 and the executable path was read. Everything else never enters a plan. Items are ordered owned-ended first and then suspects, by PID inside a class. For each item the plan keeps the model view (`ModelFinding`) and the process identity: boot session, PID, start time, user and executable path.

The plan ID is 16 bytes from `getentropy` as 32 lowercase hex characters. A plan lives in the memory of one `PlanStore` for 10 minutes by the store's monotonic timer and for as long as the store lives. A second store, which is what a restarted server is, does not know any plan of the first. Nothing about a plan is written to disk except the inspection report.

The caller gets the plan ID, the expiry, the model views and the name of the report file (not its path, which holds the user name). The report is `inspection/<plan id>.txt`, mode 0600 in a 0700 directory, created exclusively. It has the header, the counts and one display line per item, which is the same text the prompt shows. It holds typed fields only, so no path, command or process chosen name can reach the file, and it holds no code, nonce or approval state. The plan ID is its file name and is not in its text. Apply takes a plan only from the memory of the server, so the report, its name, its path and its text are all unknown plans when passed back.

The server has no timers, so a report is removed at the next `plan` or `begin` call after its plan expired, when the store is dropped and when a store starts. The startup sweep removes only regular files whose names are 32 lowercase hex characters and `.txt`, and does nothing when `inspection/` is a link or has a loose mode.

A plan can have more than 10 items. `apply` takes at most 10 per call.

## Approval

A call is a plan ID and an ordered list of item IDs. It has no field for a code.

`begin(call)` checks, in this order: the apply switch, that the call has between 1 and 10 items, that no ID is named twice, that the plan exists and has not expired, that every ID is in the plan, and that no item is already waiting for approval or applied. The first failure is the error and nothing is claimed. Otherwise it claims every item (they are pending in memory) and returns the first challenge.

The items form approval units. All owned-ended items of the call are one unit, in call order, and come first. Each suspect is its own unit, in call order. A challenge carries a nonce (16 random bytes as 32 hex characters), the prompt, the unit's position and the IDs in the unit. The code is 4 symbols from `ACDEFGHJKMNPQRTUVWXY34679` (25 symbols, bytes of 250 and above are discarded so the draw is uniform) and is valid for 120 seconds. It is in the prompt text and nowhere else.

The prompts are fixed texts around the display line of each item, which holds the item ID, class, PID, plain executable name, age, `cwd_relation` and evidence kinds:

```
Stop 2 processes that a Claude Code session left behind? The session has ended. Each gets one SIGTERM.

<display line>
<display line>

Type <code> to approve all 2.
```

A batch of one reads "Stop 1 process ... It gets one SIGTERM." and "Type <code> to approve.". A suspect reads "Stop this process? No session that AgentDust knows started it, and it has run detached and idle for a long time. It gets one SIGTERM.", then the line, then "Type <code> to approve.". The owned batch lists every item it covers. No prompt holds a path, a command or text that a process chose.

`answer(call, nonce, response)` does this, in order:

1. Take the nonce out of the memory of the server. An unknown, malformed, replayed or already used nonce is `UnknownRequestState`. From here on the nonce is gone, so nothing below can be retried with it, and no signal has been sent.
2. If the call is not equal to the call the nonce was issued for (plan ID, item IDs, order, count), release the claims and return `ArgumentsChanged`.
3. If apply is switched off, release the claims and return `Disabled`.
4. If the plan has expired, every item that is still to run gets `plan_expired`.
5. Decide. Decline, cancel and timeout are `declined`, `cancelled` and `timed_out`. An accepted answer that arrives 120 seconds or more after the challenge is `expired`. Otherwise the answer must be the exact code: no trimming and no case folding, `empty` for a missing or empty answer and `wrong_code` for any other.
6. An approved unit runs each item in order through the executor. A refused unit is noted in the audit log and its items are released, so they can be asked about again.
7. The next unit, if any, gets a new challenge with a new nonce and a new code. A refused unit does not stop the units after it. If apply was switched off while the unit ran, the items of the later units get `disabled` and are not asked.

When the last unit is done the answer is a `Report` with one entry per item in the order of the call. `run(call, ask)` loops `begin` and `answer` for a caller that blocks on the person.

No lock is held while a prompt waits. The only state of a waiting approval is in memory: the claim on its items and the nonce entry. A second call on other items goes through. A second call on the same items is `ItemUnavailable` until the first ends or its code expires.

A nonce entry whose code has expired is dropped, and its claims released, at the next `plan` or `begin` call. An `answer` that comes after that gets `UnknownRequestState`, and one that comes before it gets `expired`. Both signal nothing.

## Execution

`Executor::execute(plan_id, item)` runs spec 6.4 for one approved item:

1. Read the apply switch. Off means `disabled` and nothing else happens: no lock, no read and no audit line.
2. Take a non-blocking `flock` on `locks/<item id>.lock`. Held means `handled_elsewhere`. A lock directory or file that fails a safety check means `lock_unavailable`.
3. Take a fresh survey. The item must be in it with the same kernel identity, class, exact attribution owner set and executable path as before. Missing is `gone`, the same PID with another start time is `revalidation_failed` with `identity_changed`, another class is `class_changed`, a changed owner set is `ownership_changed`, and another path is `path_changed`. Any class or owner change aborts, including a move to a more certain class, because the person approved the evidence that was shown.
4. Write the attempt line to the audit log. If that fails the item stops with `audit_unavailable` and nothing is signalled.
5. `revalidate` reads the identity (boot session, PID, start time, user, path) and, on a match, the next call is `Signaller::sigterm(pid)`. Nothing is logged, allocated or read in between: the match arm holds the one call. Gone is `gone`. A different identity is `identity_changed` or `path_changed`, and an unreadable path or a failed read is `unreadable`, so a missing path fails closed. `ESRCH` is `gone`, and any other failure of the signal is `signal_failed`.
6. Poll the identity every 50 ms for up to 5 seconds. Gone, or the same PID with another start time or boot session, is `terminated`. The same identity, an `exec` with another path, or a read that fails is still alive. After 5 seconds it is `survivor`. No second signal is sent and SIGKILL does not exist in the code.
7. Write the result line and release the lock, which removes its file.

`KillSignaller` refuses a PID at or below 1 (`signalable`), because `kill` reads 0 and negative values as groups. The trait has no method for another signal, and the executor calls it with one PID and nothing else.

The survey of step 3 includes the idle sample, so each item costs at least 2 seconds, and a batch of 10 costs at least 20 seconds before any wait.

## Results

| `result` | Meaning |
| --- | --- |
| `terminated` | SIGTERM was sent and the process was gone, or its PID was taken by another process, within 5 seconds |
| `survivor` | SIGTERM was sent and the same process was still there after 5 seconds |
| `gone` | Not in the fresh inventory, gone at the identity read, or `kill` said `ESRCH` |
| `revalidation_failed` | The item was not signalled. `reason` is `class_changed`, `ownership_changed`, `identity_changed`, `path_changed`, `unreadable` or `survey_failed` |
| `handled_elsewhere` | Another server holds the lock of this identity |
| `signal_failed` | `kill` failed for another reason, or the signaller refused |
| `audit_unavailable` | The attempt line could not be written, so there was no signal |
| `lock_unavailable` | The lock directory or file failed a safety check |
| `disabled` | Apply is switched off |
| `declined`, `cancelled`, `timed_out`, `empty`, `wrong_code`, `expired` | The approval of the unit ended without a signal |
| `plan_expired` | The plan expired before the unit ran |

A unit that ends in `handled_elsewhere`, `disabled`, `lock_unavailable`, `audit_unavailable`, `plan_expired` or any refusal releases its items, and a later call can ask about them again. Every other result finishes the item, and a finished item is not offered again.

The report is `{"plan_id": ..., "items": [{"item_id": ..., "result": ..., "reason": ...}]}` and holds no text from a process.

## The apply switch

`config.toml` in the data directory is read by `config::apply_switch` for every call of `begin` and `answer` and for every item. Apply is on when the file does not exist or has no `apply` key, and when `apply = true`. It is off in these cases, each with a stable code and a sentence:

| Code | Cause |
| --- | --- |
| `switched_off` | `apply = false` |
| `unsafe` | The file is a symbolic link, is hard linked, is not a regular file, is owned by another user or has a mode looser than 0600. The sentence names the mode, so a person whose editor wrote a 0644 file learns why |
| `unreadable` | The file exists and cannot be opened or read |
| `invalid` | The file is over 64 KiB, is not UTF-8, does not parse as TOML, or its `apply` key is not a boolean |

The TOML parser is the `toml` crate, so every spelling of `false` that TOML allows is recognised and a key of another table is not the setting. `plan` still works when apply is off, and `doctor` is not affected.

## Files

| File | Mode | Content |
| --- | --- | --- |
| `audit.log` and `audit.log.1` | 0600 | One JSON line per record, 13 keys in a fixed order: `v`, `wall_ms`, `plan`, `item`, `pid`, `start_us`, `uid`, `class`, `evidence`, `exe_base`, `phase`, `result`, `reason`. `exe_base` is the plain executable name or null. A line holds no path, command or boot ID. The file rotates to `audit.log.1` before an append that would pass 5 MiB (the limit is a setting), so the two files hold at most 10 MiB |
| `audit.lock` | 0600 | Empty. A blocking `flock` around each append and each rotation |
| `inspection/<plan id>.txt` | 0600 in a 0700 directory | The plan report |
| `locks/<item id>.lock` | 0600 in a 0700 directory | Empty. Created when an item starts and removed when it ends |

`phase` is `attempt` for the line written before the identity read and `result` for the line written after. An attempt line means the item reached the signal step, not that a signal went out: the result line says what happened. Every item of an approved or refused unit gets one result line, except `disabled`.

Every file is opened without following links and must be a regular file with one link, owned by the user and with no mode bit outside 0600. A directory must be owned by the user and have no bit outside 0700. A violation refuses the operation: `audit_unavailable`, `lock_unavailable` or a plan that cannot be made, and nothing is written through a link. The lock re-checks, after it holds the `flock`, that its file is still the file at the path. A holder that removes its file on release would otherwise let one caller lock the old file and another lock a new one.

## Safety requirements and where they are tested

| Requirement | What the tests pin |
| --- | --- |
| S1 | A plan holds only owned-ended and suspect items, whatever the survey returns. The ID of a managed, owned-live, likely-owned or unknown process is `UnknownItem`, alone or beside a valid item, and nothing is signalled. `PlanItem::plannable` is the one gate, over the one total function `class::actionable_as` |
| S2, S3 | Approval is a `Response` to a challenge and a `Call` has no field for a code. Wrong, empty, lower case, padded and longer answers, decline, cancel, timeout, an expired code (the boundary is 120 s) and a code of another unit signal nothing. A used nonce is unknown |
| S4 | Unknown and malformed nonces, a replay, and a changed plan ID, reordered, added, removed, duplicated and replaced item IDs are refused and burn the nonce. A hook inside the signal call shows that the nonce is already gone when the first signal is sent |
| S5 | 0 and 11 items, 6 owned and 5 suspect together, and a duplicate are refused. 10 items are one batch prompt that lists every item. The three prompt texts are pinned and a hostile executable name never reaches a prompt |
| S6 | Any class change during approval aborts the item, in the scripted tests, with a real process that becomes managed, with a lost journal, with a journal of a newer version and with a lost secret. The other items of a batch still run |
| S7 | A scripted provider returns another start time, user, boot, path, an unreadable path or an error at the read before the signal, and nothing is signalled. A spy shows that the audit file does not change between the read and the signal. A live provider that shifts the start time aborts the item |
| S8 | The signaller has one method. `signalable` rejects 0 and below and 1. A real `SIGTERM` reaches a child of the test. The harness signal log shows one `Term` per approved PID and no other signal |
| S9 | Two servers on one item: exactly one signal, the other `handled_elsewhere`, with a gate that holds the first inside the signal call. Four servers at once: at most one signal. The lock tests include a file replaced between open and lock, and a loop of four threads that never has two holders |
| S10 | While a prompt waits, no lock file exists, a second call on other items completes, and the lock of the waiting item can be taken |
| S11 | A plan is refused at 600 s and accepted at 600 s minus 1 ns. A plan that expires while a prompt waits gives `plan_expired`. A new server knows no plan and no nonce of an old one |
| S14, S15 | After a full run with planted user, repository and executable names, none is in the report, the audit log, a prompt, the plan JSON or the result JSON, every file is private and one of five known names, and the audit keys are the 13 named |
| S16 | The lock file and the audit file are refused as a symbolic link, a hard link, a loose mode, another owner, a FIFO or a directory, the audit lock and the lock directory as a link or a loose mode, the inspection directory as a link or a loose mode, and the config file as a link, a hard link, a loose mode, a FIFO or a directory |
| S18 | A journal with a version line of 2 appended during approval aborts the items and a fresh plan holds none of them |
| S21 | `apply = false`, a malformed file and a loose file refuse `begin`, `answer` and each item. A switch set while a prompt waits refuses the answer and frees the items, and a switch set during a batch stops the later items and units |

## Where this differs from the spec

- Execution runs when a unit is answered, so the first unit signals before the second unit is asked. Spec 6.4 says "for each approved item, in order" and does not say when.
- A failed approval (decline, cancel, timeout, empty, wrong or expired) ends that unit only, and later units are still asked, as spec 6.3 step 5 says. A front end can stop asking by not calling `answer` again, which releases nothing until the nonce expires.
- Any change of class aborts the item. The spec says a change to a less certain class aborts. A suspect that became owned-ended is also refused, because the person approved a suspect.
- The prompt of an owned batch says "a Claude Code session" and not "session X". The finding does not carry a session ID, and the prompt may hold typed fields only.
- The inspection report holds the typed fields of the model view and not the redacted command and directory. Spec 7.2 lists `inspection/` among the files that the privacy test scans for commands and paths, so a report with a command or a directory would break S14. The terminal block of `doctor` shows both.
- The server has no timer, so a report is removed at the next call after expiry, at drop and at startup, and not at the instant of expiry.
- The audit log has an attempt line before the identity read. Spec 6.4 has one append after the wait. The attempt line is on record before the read, so a signal cannot go out unrecorded, and an attempt that cannot be written stops the item. A result line that cannot be written after the signal changes nothing about the result and is not reported.
- `audit.lock` is a file of the data directory that spec 7.2 does not list.
- `config.toml` is checked like the other state files, so a 0644 file disables apply. Spec 7.3 lists the file among those opened without following links. A symbolic linked `config.toml` (a dotfiles manager makes one) disables apply too.
- A data directory, `inspection/` or `locks/` with a loose mode, a link or another owner is refused and not corrected. A missing one is created 0700. Spec 7.3 corrects a wrong directory mode in commands that write.
- `plan` works when apply is off, so a caller can still read the model views. Spec 6.5 says apply refuses everything.

## Remaining release limits

- The correction of a wrong data directory mode.
- A churn stress run that reuses PIDs while a plan is applied. PID reuse is tested with a scripted provider and with a provider that shifts the start time.
- Reporting a result line that could not be written.
- A bound on the number of items in a plan. A plan lists every owned-ended and suspect process of the machine, and `apply` takes 10 at a time.
- The inspection report of the terminal block (command and directory), which would need a privacy decision.

## Tests

| Subject | Files |
| --- | --- |
| The code | `crates/agentdust-core/tests/code.rs`, `crates/agentdust-mcp/tests/codes_shared.rs` |
| The switch | `crates/agentdust-core/tests/config.rs` |
| Safe opens, lock, audit, signaller, timer | `crates/agentdust-core/tests/safe_open_private.rs`, `crates/agentdust-core/tests/apply_lock.rs`, `crates/agentdust-core/tests/apply_audit.rs`, `crates/agentdust-core/tests/apply_signal.rs` |
| One item | `crates/agentdust-core/tests/apply_exec.rs` |
| The plan and its report | `crates/agentdust-core/tests/plan.rs` |
| The protocol and its guards | `crates/agentdust-core/tests/apply_server.rs`, `crates/agentdust-core/tests/apply_server_run.rs` |
| Privacy of the files, prompts and results | `crates/agentdust-core/tests/apply_privacy.rs` |
| Real processes, with the harness signal log as the oracle | `crates/agentdust-testkit/tests/apply_live.rs` |
| The live surveyor with a real journal and secret | `crates/agentdust-testkit/tests/apply_end_to_end.rs` |

The scripted tests share `crates/agentdust-core/tests/apply_support/mod.rs`: a surveyor, a provider, a signaller and a timer that record what they are called with, and a small world of processes that exit when signalled, ignore SIGTERM, change class or are replaced. The live tests share `crates/agentdust-testkit/tests/apply_support/mod.rs`, whose signaller forwards only to the harness and returns `Refused` for any PID the harness did not start.
