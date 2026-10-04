# Ground-truth fixtures

Success criterion 5 of the roadmap says the classifier is judged against fixtures that carry the truth about each process. Spec 9.2 lists the five labels, spec 3.4 defines the classes they map to, and spec section 10 says the suspect age and idleness thresholds come from this corpus. This page describes the schema, the loader, the planner, the materialiser and the corpus in `fixtures/m1/`, and how M2 will use them.

The schema, loader, planner and materialiser are in `agentdust-testkit` (`agentdust_testkit::fixture`). The class table is `agentdust_core::class`. The processes a fixture starts run under the [live harness](m1/live-harness.md).

## Classes and what each may be asked to do

`Class` is serialised as the names in the spec 3.4 table, listed here in the order the classifier checks them. `actionable_as(class)` is a total function with one answer per class.

| Class | `actionable_as` | Meaning |
| --- | --- | --- |
| `managed` | `Never` | Loaded launchd job, exact Homebrew service, verified app helper, or an entry of the deny list (PID 1, UID 0, another user, the AgentDust server, live agent processes) |
| `owned-live` | `Never` | Ownership evidence and the agent identity is alive |
| `owned-ended` | `BatchCode` | Session tag match or sampled as a descendant, and the agent identity is gone |
| `likely-owned` | `ReportOnly` | Working directory key and executable match a paired shell window of an ended session |
| `suspect` | `PerItemCode` | Parent is launchd or the launcher chain is dead, same user, old and idle enough, not managed |
| `unknown` | `Never` | Everything else |

## Labels

A label is what a process truly is. Each label maps to one class, and `must_never_signal` is false only for the two labels a test may signal.

| Label | Expected class | `must_never_signal` | The process is |
| --- | --- | --- | --- |
| `true_owned_ended` | `owned-ended` | false | A leftover of an ended session, with ownership evidence in the journal |
| `true_live_owned` | `owned-live` | true | A process of a session whose agent is still running |
| `true_detached` | `suspect` | false | An orphan under launchd with a session of its own and no ownership evidence, such as a tunnel |
| `true_managed` | `managed` | true | Protected by a managing job, a service, an app or the deny list |
| `true_unknown` | `unknown` | true | A process with no evidence whose parent is alive and is not launchd |

`likely-owned` has no label. Nothing in spec 9.2 names one, and the correlation engine that produces it arrives with the Codex adapter. A test checks that `must_never_signal` agrees with `actionable_as`: the flag is true exactly when the expected class is `Never` or `ReportOnly`.

An orphan with no ownership evidence and no session of its own meets every suspect condition except the thresholds, so it is not `true_unknown`. It has no label either, and it is not in the corpus.

## File format

One JSON object per file, `fixtures/m1/<name>.json`, where `<name>` is the `name` field. Every object refuses unknown fields.

```json
{
  "schema_version": 1,
  "name": "detached_tunnel",
  "description": "A tunnel the agent started in its own session ...",
  "agent": "claude",
  "processes": [
    { "role": "agent", "flags": ["--setsid"],
      "expected": { "label": "true_managed", "class": "managed", "must_never_signal": true } },
    { "role": "tunnel", "parent_role": "agent", "flags": ["--setsid"],
      "expected": { "label": "true_detached", "class": "suspect", "must_never_signal": false } }
  ],
  "journal": [
    { "kind": "session_start", "session_id": "s1" },
    { "kind": "shell_start", "session_id": "s1", "tool_use_id": "t1", "exe_base": "ssh" },
    { "kind": "session_end", "session_id": "s1" }
  ],
  "session_ended": true
}
```

| Field | Rule |
| --- | --- |
| `schema_version` | Must be 1. Any other value is refused and nothing else in the file is checked |
| `name`, `description` | Text. The name matches the file name |
| `agent` | `claude`, `codex` or `cursor`, as in the journal |
| `processes[].role` | 1 to 64 characters of `a` to `z`, `0` to `9` and `_`. Unique in the fixture |
| `processes[].parent_role` | Optional. A role of the same fixture. No cycles |
| `processes[].flags` | Arguments of `fixture-sleeper`, parsed by `ProcSpec::parse`. `--spawn`, `--report-file` and `--echo-env` are refused because the harness sets them |
| `processes[].expected` | `label`, `class` and `must_never_signal`, all required. `class` and the flag must agree with the label |
| `processes[].record_only` | Optional, false by default. Only allowed for `true_managed` |
| `journal[]` | `kind` and `session_id` are required. `subagent_id`, `tool_use_id` and `exe_base` are optional and follow the journal record rules. `roles` is allowed on `sample` only and names roles of the fixture. `shell_start` and `shell_end` need a `tool_use_id` |
| `session_ended` | Whether the agent process is gone when the classifier looks. A `session_end` event in the journal does not imply it: the clear command writes one while the agent keeps running |

A journal event holds no timestamp, no boot, no process id, no working directory key and no path. Their order in the array is their order in time. M2 turns them into journal records with real timestamps when it replays a fixture.

## Reading a fixture

- A role is a process. `parent_role` names the process that started it.
- The agent is a stand-in: a root role with children, labelled `true_managed` because the deny list names live agent processes. When `session_ended` is true, every root with children is killed before observation and its children are orphaned to launchd. That process is gone and is not classified.
- A root without children is started directly by the test and is left alone, so it has the test process as its parent.
- The observed processes (`Fixture::observed`) are all processes except the ended stand-ins, and are the ones a classifier sees.
- A `record_only` process is never started. It carries its label and class, and M2 feeds recorded attributes to the classifier for it. A test cannot start a launchd job, a service of another user or an application, so only `true_managed` may be record only. Any other label has to be started for real. A record-only root never ends with the session, because it is never started and cannot be orphaned.

## The loader

`load(path)` reads and checks one file. `load_dir(dir)` loads every `*.json` file in a directory, in name order, and skips everything else. `parse_str(file, text)` works on text.

A failure is a list of `Problem { file, role, kind }`, printed as `path: role name: reason`:

```
fixtures/m1/x.json: role server: parent_role "ghost" is not a role of this fixture
```

The loader returns every problem it can find, in one run, for every file of a directory, in the same order each time. Which problems it finds together:

| Stage | Problems | Reported |
| --- | --- | --- |
| File | Unreadable, not JSON, not an object, `schema_version` missing or not 1 | One per file, and the stage stops there |
| Shape | Unknown field, missing field (a process without a label), wrong type | One per process, one per journal event, one for the top level. serde stops at the first mistake inside one of them |
| Roles | Bad role name, duplicate role, `parent_role` that does not exist, cycle | All of them. A process that failed the shape stage still counts as a known role, so it causes no second, false error |
| Processes | Class or `must_never_signal` contradicting the label, flags that do not parse, harness flags, `record_only` on another label than `true_managed` | All of them |
| Journal | `roles` on a kind other than `sample`, unknown role, shell event without `tool_use_id` | All of them |
| Name | `name` different from the file name (`load` and `load_dir`) | One per file |

## Planning and starting

`Fixture::plan()` decides how the harness can start a fixture. It is pure, so it runs on Linux too. The harness starts one level trees whose children inherit the parent's flags, so a fixture is startable only when it has that shape:

- A live root with its live children is one group: one `spawn_tree`, with the root as the parent and as the stand-in for the agent.
- A live process whose parent is not a root (a grandchild), children with other flags than their parent, more than 32 children and a live child of a record-only role are refused.
- Flags that do not parse are refused as `BadFlags`. A fixture built without the loader gets no default flags.
- Each member carries the structure the kernel should show afterwards: the parent (the test, a role, or launchd) and whether it leads a session of its own (`--setsid`).
- The label has to agree with that structure. `true_owned_ended` needs launchd as its parent, `true_detached` needs launchd and `--setsid`, and `true_live_owned` needs a live role as its parent. A corpus mistake here is reported on Linux, in a unit test, and not first on a Mac.

`materialise(fixture, fixture_binary)` (macOS) starts a plan through `Harness`, orphans the ended stand-ins, and returns a `Materialised` that gives access to each role's handle and to the harness. `check_structure()` asks the kernel three questions per observed process and returns a `Violation` for each wrong answer:

1. Is it alive? A process whose lifetime has run out may be gone.
2. Is its parent the one the plan states?
3. Does it lead its own session exactly when the plan says so?

Only the harness signals. An ended fixture sends one SIGKILL to each stand-in agent through the harness and nothing else, and a live fixture sends nothing. Dropping a `Materialised` ends every process, including detached ones that ignore SIGTERM.

## Time

- A started process lives `FIXTURE_SECONDS` (120) unless its flags give another lifetime, so a test that waits up to 60 s for a process to die cannot pass because the process ran out of time. A corpus test checks that every process that does not exit by itself lives longer than 60 s.
- A process that exits by itself (`--exit-after-ms`) does so at least 5000 ms after it reports. A corpus test requires it, so a test that reads its identity right after the start has a wide margin.
- The excuse in question 1 above is `elapsed >= lifetime` for a process that is gone. `check_structure()` passes the real time since the start, and `check_structure_at(elapsed)` takes it as an argument, so tests assert the boundary on both sides and the excuse for a live process without waiting for a clock.
- Checks that a process is still running after SIGTERM watch for 400 ms. A slow runner cannot turn them into a failure, because only the process dying fails them.
- No test asserts how long something took, and no test times the first start of a freshly built fixture binary.

## The corpus

22 fixtures in `fixtures/m1/`. 16 start real processes, 5 are record only and 1 has no processes.

| Fixture | What it covers | Observed labels |
| --- | --- | --- |
| `owned_ended_background_server` | A sampled dev server left after the session ended | owned_ended |
| `owned_ended_ignores_sigterm` | The same, ignoring SIGTERM | owned_ended |
| `owned_ended_batch_of_three` | Three leftovers for one batch code | owned_ended (3) |
| `owned_ended_exits_during_approval` | A leftover that exits by itself after 5 s | owned_ended |
| `detached_tunnel` | An orphan with its own session, not sampled | detached |
| `detached_ignores_sigterm` | The same, ignoring SIGTERM | detached |
| `live_session_background_server` | A sampled server of a live session | live_owned, managed |
| `live_session_subagent_shell` | A subagent's helper in a live session | live_owned, managed |
| `session_end_event_agent_still_alive` | A `session_end` is journaled and the agent runs on | live_owned, managed |
| `resumed_session_leftovers` | One session id, two agent processes, a leftover from each | owned_ended (2) |
| `subagent_leftover_after_session_end` | A subagent's leftover | owned_ended |
| `session_end_without_descendants` | A session with nothing left (no processes) | none |
| `abrupt_termination_no_session_end` | No `session_end`, a shell call without an end | owned_ended |
| `unrelated_process_no_evidence` | A process with no journal record | unknown |
| `unrelated_process_beside_live_session` | A bystander next to a live session | live_owned, managed, unknown |
| `unrelated_process_with_live_launcher` | A shell and the server under it | unknown (2) |
| `agent_process_is_protected` | The agent process itself | managed |
| `system_process_pid_one` | PID 1 (record only) | managed |
| `other_user_process` | Another user's process (record only) | managed |
| `launchd_job_lookalike` | A launchd job with a helper (record only) | managed (2) |
| `homebrew_service_lookalike` | A Homebrew service that looks like a leftover (record only) | managed |
| `app_helper_process` | A running application's helper (record only) | managed (2) |

The labels observed across the corpus, counting only processes the classifier sees:

| Label | Processes | Fixtures |
| --- | --- | --- |
| `true_owned_ended` | 10 | 7 |
| `true_live_owned` | 4 | 4 |
| `true_detached` | 2 | 2 |
| `true_managed` | 12 | 10 |
| `true_unknown` | 4 | 3 |

The tests in `tests/fixture_corpus.rs` pin the corpus: at least 14 fixtures, each label observed in at least 2, the six protected cases and the edge cases present by name, every fixture loadable and startable, no fixture naming a home directory or the home directory of the machine that runs the test, and the three roadmap claims restated over the data (only `true_owned_ended` is expected `owned-ended`, no managed or unknown process is expected to be actionable, nothing that must never be signalled is). Every `true_unknown` process has a live parent that is not launchd.

The lookalike fixtures stand for the real service or job. They resemble a leftover (parent launchd, long running, idle) and are protected by their identity.

## How M2 uses the corpus

M2 materialises each live fixture, builds the journal records from `journal` with real timestamps and the process ids the harness reports, runs the classifier over the observed processes, and compares each class with `expected.class`. Record-only processes are fed to the classifier as recorded attributes.

Success criterion 5 then reads as three checks over the result: no process outside `true_owned_ended` is classified `owned-ended`, no `true_managed`, `true_unknown` or `true_live_owned` process is classified `suspect` or `owned-ended`, and every `true_owned_ended` process is classified `owned-ended`. `expected.class` for `true_detached` is the class once the process is old and idle enough, so M2 has to give the classifier an injected clock and an idleness input to place a process at any age without waiting.

### Measuring the suspect thresholds

No threshold is set here. M2 sets them from measurements:

1. For every observed process of every live fixture, record the label, the age and idleness the classifier was given, and the class it returned, over a grid of ages and idleness values.
2. The age threshold and the idleness threshold are the smallest values at which every observed `true_detached` process is classified `suspect` while every process with `must_never_signal` stays out of `suspect`.
3. If a `must_never_signal` process is classified `suspect` at every age and idleness that make a `true_detached` process suspect, no threshold separates them. That is a finding about the rules, and the measurement does not hide it behind a number.

What the corpus cannot do yet:

- It bounds the thresholds from one side only. Every process that must stay out of `suspect` fails a condition that no threshold changes (a live owner, a live non-launchd parent, a deny list entry), so none of them marks the lower edge. M2 has to add `true_detached` and `true_unknown` fixtures that are young or busy before a number can be chosen.
- `fixture-sleeper` only sleeps, so a busy process cannot be started. A flag that burns CPU is needed for the idleness boundary.
- A chain with a dead intermediate (an agent, a shell under it, a server under the shell, and the shell gone) cannot be started, because the harness starts one level. The launcher-chain threshold has no live fixture: `unrelated_process_with_live_launcher` holds a live chain only.

## Limits

- A fixture holds labels and structure, not kernel attributes. A record-only process has no recorded user id, executable path or parent id, so M2 has to define that recording before the protected cases reach the classifier.
- Journal events are ordered, not timed, and carry no working directory key, so `likely-owned` has no fixture. Replaying them with spacing that matters (an idle window) is M2's work.
- Children inherit the parent's flags, so a fixture cannot mix a cooperative stand-in agent with SIGTERM-ignoring children, or a plain leftover with a detached one in the same tree. Two stand-ins in one fixture (as in `resumed_session_leftovers`) start two trees.
- `session_ended` is one value per fixture. The resumed session fixture has both agents ended.
- The corpus is Claude Code only (`agent` is `claude` in every fixture). Codex and Cursor fixtures come with their adapters.
- Tests that start processes run on macOS only. The schema, loader, plan and corpus tests are portable and belong to the Linux CI job.
- A process of a fixture that is still running when its test process is killed with SIGKILL runs out its lifetime (120 s, or 5 s for the one that exits by itself), because no teardown runs.

## Tests

`cargo test -p agentdust-testkit --test fixture_schema --test fixture_validate --test fixture_plan --test fixture_corpus --test fixture_materialise` runs all of them, and `cargo test -p agentdust-core --test class` runs the class table. `fixture_materialise` is macOS only.
