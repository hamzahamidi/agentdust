# M6 dogfood record

This record tracks real use after the limited beta. The v1 release gate is one controlled end-to-end run on the maintainer's Mac, recorded in the [readiness record](../v1/readiness.md). Setup checks and scripted acceptance tests do not replace that run. Review product value after the next ten substantive sessions on the available Mac. There is no multi-Mac or fixed-duration gate. Identify each recorded build by its source commit and binary digest.

On 2026-10-07, only machine A was available. Its setup check passed, but no interactive Claude Code session has been observed. Machines B and C are unavailable; the optional confidence study has not started.

On 2026-10-08, AgentDust `1.4.0` was installed through Homebrew from source commit `c89beae63520550c756c59fd9bab979d4cf90b32`; the installed binary SHA-256 was `a80f676ad2fef62e676e11df97fa590fa1a7a2dd195bba57f6844f715346ec4f`. Automatic cleanup was enabled for 37 exact directory scopes and the worker was running. The bounded report contained no recent cleanup results and one `owned-live` review item. No natural cleanup or port incident was observed during release verification. CI fixtures and installed smoke checks demonstrate controlled behavior only; normal-use product value has not yet been observed.

On 2026-10-09, a read-only status check of the same binary showed 37 enabled directory scopes, a running worker, a ready report, one recent `terminated` result, no review items and no pending work. The bounded report now contains one recent result that was absent from the 2026-10-08 check. The report does not retain a result timestamp or session scenario, so the event cannot be assigned to a specific session or date row. It confirms that an automatic cleanup outcome was recorded, not when it occurred or whether it solved a user problem.

On 2026-10-11, Homebrew installed AgentDust `1.7.1` (binary SHA-256 `c1011d1bd0868db5211f40d7e6e3ebcede3213b657f614082ac9d94416aa7a7e`). Setup check passed. Automatic cleanup remained enabled for 37 directory scopes. The loaded worker was still `1.4.0`, so its historical `terminated` report predates M13 outcome metrics and cannot be counted as useful cleanup evidence. `agentdust auto resume` restarted the worker from the installed `1.7.1` binary. Metrics still reported zero recorded outcomes immediately afterward. No synthetic cleanup was run. Start ordinary-use evidence from this worker build.

Later on 2026-10-11, Homebrew installed AgentDust `1.8.1` (binary SHA-256 `d21f9b3eed9394371e02aac24bfb8b3512156481d89184945cae0208ca7bb33f`). The Claude Code setup check passed. Automatic cleanup remained enabled for 37 exact project scopes. The worker was running and its report was ready, but its version status was `unknown`. The report had no pending work, one recent `terminated` result and one review item. It has no timestamp or session scenario, so the result cannot be attributed to a session or counted as user benefit. `agentdust metrics --json` reported zero events. The foreground `agentdust auto resume` completed after typed confirmation. A subsequent status check reported CLI `1.8.1`, worker `1.8.1` and version status `same`, with automatic cleanup still enabled for 37 exact project scopes. Metrics still reported zero events. No synthetic cleanup was run. Ordinary-use outcome evidence starts from this worker build.

An aggregate parse of the retained local journal generations on 2026-10-11 covered 2026-10-07 through 2026-10-11. It found 29,747 valid records across 75 agent/session pairs (41 Claude and 34 Codex). The records included 212 `session_start` events, 92 `session_end` events, 14,777 `shell_start` events and 14,543 `shell_end` events. Twenty-nine pairs contained both a session start and a session end. The journal contained no `sample` records. These counts confirm hook activity, but do not identify substantive sessions, process ownership ground truth, useful cleanup or user benefit. The records do not identify their AgentDust build or scenario, so they cannot validate the suspect thresholds or be attributed to a particular release. Metrics remained at zero. The historical `terminated` result remains unattributed.

For each real incident during the next ten substantive sessions, record the smallest evidence that supports one of these signals:

| Signal | Evidence |
| --- | --- |
| Useful cleanup | Confirm an unwanted helper was stopped and how that helped. |
| Deliberate keep | Record that a helper was intentionally preserved. |
| Incorrect proposal | Record what AgentDust proposed and why the proposal was wrong. |
| Port recovery | Confirm the original operation succeeded after cleanup. |

Use labels `A`, `B` and `C` for the machines. Record aggregate counts only. Do not add usernames, serial numbers, process IDs, paths, command text, session IDs, raw hook payloads or transcript content.

Record the AgentDust version and exact commit SHA or artifact digest. Add one row for each observation date and machine. Use scenario names such as `session`, `foreground subagent`, `background subagent`, `resume`, `clear`, `abrupt exit` and `Agent Team`.

| Date | Machine | Claude Code | AgentDust version, commit or digest | macOS | Scenarios | Sessions | Proposed kills | False proposals | Attribution errors | Blocked applies | `SIGTERM` sent | Failures | Hook samples | Hook p50 ms | Hook p95 ms |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2026-10-07 | A | 2.1.292 | 0.2.0, commit `d727dda68703df1e9ab632c1ad5b8647bc0d5e08`, binary SHA-256 `6a84baa674a1ffb342d1baa9f7b87af93da4bdb37b949b5dfebbe8eb742b70f3` | macOS 26.6.2 | setup | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | not measured | not measured |
|  | B |  |  |  |  |  |  |  |  |  |  |  |  |  |  |
|  | C |  |  |  |  |  |  |  |  |  |  |  |  |  |  |

Count a false proposal when a proposed process is not safe to stop under the classifier rules. Count an attribution error when an observed helper has a missing, extra or incorrect owner. Count a blocked apply when safety revalidation refuses to signal an item. Keep user declines separate from blocked applies. Count each `SIGTERM` sent and each failed hook, MCP call or CLI operation.

Count a hook failure only when expected hook evidence is missing or invalid, or a hook diagnostic reports failure. Exit status alone does not establish hook success because the production hook exits successfully on some failures. Count an MCP or CLI failure when the operation reports failure. Write `not measured` when a value is unavailable; use `0` only for a measured zero.

Record hook sample count, p50 and p95 from the release binary under the full criterion 9 workload. Ordinary idle use or a smaller fixture does not satisfy the latency gate. Keep detailed local diagnostics out of this shared record.
