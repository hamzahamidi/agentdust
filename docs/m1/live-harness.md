# Live process-tree harness

Spec 9.2 lists a live layer that "spawns real process trees and signals only PIDs it created". S8 asks for "SIGTERM only, one exact PID, no group, no SIGKILL" and reads it from a signal log. This page describes the code that provides both, in `agentdust-testkit`.

The harness (`agentdust_testkit::harness`) exists on macOS only, because it reads kernel identities through `agentdust_core::darwin`. The fixture binary, its argument grammar (`spec`), its report format (`report`) and `wait_until` build and test on Linux too.

## The fixture process

`fixture-sleeper [SECONDS] [flags]` sleeps for `SECONDS` (default 30). A bare number works as it did in M0.

| Flag | Effect |
| --- | --- |
| `--ignore-term` | Ignore SIGTERM. Without it, SIGTERM is set to its default action explicitly, because an ignored disposition survives `exec` |
| `--setsid` | Become the leader of a new session before anything else is reported |
| `--exit-after-ms N` | Exit with status 0 after N milliseconds. The timer starts after the report is written. With `SECONDS`, the shorter one wins |
| `--spawn N` | Start N children (at most 32). Each child gets every flag of the parent except `--spawn`, so a tree is one level deep |
| `--echo-env NAME` | Put the value of `NAME` in the report. Needs `--report-file` |
| `--report-file PATH` | Write the report to `PATH` when setup is finished. Children write `PATH.1`, `PATH.2` and so on |

Setup runs in a fixed order: SIGTERM disposition, new session, children, wait for each child's report, own report, then sleep. A reader that sees the report knows SIGTERM is already ignored, the session already exists and every child has finished its own setup. Without that, a SIGTERM sent right after spawn could arrive before the handler that ignores it.

Arguments are parsed strictly (`ProcSpec::parse`). An unknown flag, a missing value, a number written as `+3` or `1.5`, a repeated flag and an argument that is not Unicode are errors. Exit codes:

| Code | Cause |
| --- | --- |
| 2 | Arguments do not parse |
| 3 | `setsid` failed (the process already leads a process group) |
| 4 | A child could not be started |
| 5 | A child did not report within 60 s, or exited first. The parent kills the children it started |
| 6 | The report could not be written |

The report is written to `PATH.partial` and renamed, so a reader never sees half of it:

```
pid=4242
ppid=100
sid=4242
env=<raw bytes of the variable, to the end of the file>
```

The first three lines are canonical decimal numbers, with `pid` and `sid` above 0 and `ppid` not negative. The `env` line is absent when the variable is not set, so a variable set to nothing (`env=`) differs from an unset one. The value is raw bytes, so a newline or a non-UTF-8 byte round-trips.

`fixture-journal` is a separate binary with its own modes (`write`, `straggle`, `rotate`) for the journal stress test. It takes none of the flags above, and the two binaries share only the kill-on-drop `Fixture` guard of the crate.

## The harness

```rust
let mut harness = Harness::new(env!("CARGO_BIN_EXE_fixture-sleeper"))?;
let tree = harness.spawn_tree(&ProcSpec::new().spawn(2).ignore_term().setsid())?;
harness.orphan(tree.parent)?;
harness.signal(tree.children[0], Signal::Term)?;
assert_eq!(harness.signal_log().len(), 2);
```

| Call | Behaviour |
| --- | --- |
| `Harness::new(fixture)` | Creates a private scratch directory (mode 0700) for reports and stderr files. The fixture path is canonicalised |
| `spawn(spec)` | One process. Returns after the fixture has reported and its kernel identity is registered. A spec with children is `Error::TreeRequested` |
| `spawn_tree(spec)` | A parent and `spec.spawn` children. Every child is registered with its own identity |
| `signal(handle, Signal::Term or Kill)` | Revalidates, sends, logs |
| `orphan(handle)` | SIGKILL to a direct child of the harness, reap it, return when each registered child of it has launchd as its parent |
| `signal_log()` | Every signal that was sent, as `(pid, signal, Instant)`, in order |
| `pid`, `identity`, `report`, `ppid`, `revalidate`, `provider` | Read access for assertions. `revalidate` is T2's `revalidate` against the identity stored at registration |
| `shutdown()` | Teardown that returns the survivors as an error instead of panicking |
| `wait_until(predicate, timeout)` | Polls every 10 ms and evaluates the predicate at least once, whatever the timeout. `wait_until_on(timer, ...)` runs the same loop on a `Timer`, which a test can replace with a simulated clock |

## What keeps the harness to its own processes

- A `ProcHandle` has no public constructor and carries the id of the harness that returned it. A handle used with another harness is `Error::ForeignHandle` and nothing is sent. There is no function that takes a raw PID.
- One private function, `deliver(pid, signal)`, calls `kill`. It looks the pid up in the registry first (a pid of 0 or below is never in it), then revalidates the stored identity (boot session, PID, start time, UID, executable path). Only `Match` leads to a signal. `Gone` is `NotRunning`, anything else is `IdentityChanged`. A PID that was recycled after its process exited is therefore refused. The in-crate tests call `deliver` with a PID that is not registered, with each identity field altered, and with an exited process, next to a live control process that must still be running afterwards.
- A signal enters the log only after `kill` returned 0. A refused signal leaves no entry.
- The registry gets a PID from two places only. A process started with `Command` by the harness, after it reported. A child named in a parent's report, which is adopted only when its report names the registered parent, its executable is the fixture binary, and the kernel shows the registered parent as its live parent. A report that fails a check leaves the named process alone and unregistered, and teardown never touches it.
- Every fixture starts without the variable `AGENTDUST_SESSION`, whatever the test process has, before the variables of its own spec are set. A test run inside an agent session that exports a tag therefore does not tag its fixtures by accident.
- A fixture that does not report within 60 s is `Error::NotReady`, and its process is killed and reaped with the launch. One that exits first is `Error::ExitedBeforeReady` with its stderr.
- Teardown (`Drop` or `shutdown`) sends SIGKILL to every registered process through `deliver`, falls back to `Child::kill` for a direct child whose identity cannot be revalidated, waits for direct children, and polls up to 60 s until every identity reads `Gone` or `Changed`. A survivor panics in `Drop`, except while the thread is already unwinding, where it is printed instead because a second panic aborts. The scratch directory is removed.

## How the tests treat time

- No test asserts how long something took. A wait that expects an event ends after 60 s at most and fails only then.
- Fixtures in those tests live 120 s, longer than the 60 s wait, so a process that is meant to die cannot pass by running out of time.
- A check that a process is still running (it ignores SIGTERM, it was never signalled) watches for 100 to 400 ms. A slow runner cannot turn that into a failure, because the only thing that fails it is the process dying.
- A process that must exit on its own does so 5 s after its report, so a test that reads its identity first has a wide margin.
- The poll loop is tested on a simulated `Timer`, so the number of evaluations, the size of every sleep and the cut of the last sleep are asserted exactly.
- No test times the first start of a freshly built fixture.

## Facts about the platform

- A child that has exited and is not yet reaped reads `Gone` through `DarwinProvider` (`proc_pidinfo` returns nothing for it), so a test of the "exits during approval" case does not need to reap first.
- A shell with `trap '' TERM` passes an ignored SIGTERM to the process it execs. The fixture sets the default disposition explicitly for that reason.
- A child started by `sh` through `&` is still a copy of the shell until it has called `exec`. Its executable path reads the shell's (`/bin/bash` here) during that window. `Command::spawn` returns after the exec, so the harness never sees it, and the in-crate adoption tests wait for the executable path before they adopt.
- The in-crate adoption tests start a shell family (`sleep` and `tail -f /dev/null`). They record the kernel identity of both and kill them when the test ends, if the identity is unchanged.

## Limits

- Between the revalidation and the `kill` there is a window of microseconds in which a child of a fixture parent could exit and its PID be reused. macOS has no pidfd. A direct child of the harness is not affected, because an unreaped child keeps its PID.
- Trees are one level deep and the children share the flags of the parent. A parent that ignores SIGTERM has children that ignore it too. A tree with a cooperative parent and ignoring children needs a new flag.
- The log holds the signals sent through the harness. Product code that calls `kill` itself does not appear in it. For S8 on `agentdust_apply` (M3), the apply path needs an injected signal function that the test points at `Harness::signal`. That function does not exist yet. S9 needs several servers applying at once, and the harness is single threaded (`&mut self`), so it covers the log side of S9 only.
- If `spawn_tree` fails after the parent was registered, because adoption refused a child, the registered processes stay until teardown. The refused child is not registered, and it exits by itself after its lifetime (30 s by default).
- A test process that is killed with SIGKILL runs no teardown. The fixtures it started exit by themselves after their lifetime.
- Failure to kill is not testable here: nothing in the suite produces a process that SIGKILL cannot end, so the `Survivors` branch is exercised only by breaking teardown on purpose.
- That SIGTERM is already ignored when the report becomes visible is checked by signalling right after the report, five times. A violation would show up as a race and not as a deterministic failure, because `ps` on this Mac has no keyword for signal dispositions and `libc` 0.2.190 has no `kinfo_proc` for macOS.
- `NotReparented` (children not reparented to launchd within 60 s) has no test, because nothing in the suite can keep a child from being reparented.

## Tests

`cargo test -p agentdust-testkit` runs all of them. The files are `tests/spec.rs`, `tests/report.rs`, `tests/wait_until.rs` (portable), `tests/fixture_flags.rs` (the binary driven directly), `tests/harness.rs`, `tests/harness_tree.rs` and the in-crate tests in `src/harness/tests.rs` (macOS).
