# M6 dogfood record

M6 requires one machine to complete at least two weeks of dogfood. Criterion 8 requires three independent Apple silicon Macs to run 0.x builds for at least two weeks. To count one period toward both gates, use the same M6-capable 0.x build on all three machines and have one machine collect the fuller M6 metrics. The 0.2.0 release predates M6 and does not qualify for this period. The M6 candidate installed on machine A is identified by its source commit and binary digest in the record.

On 2026-10-07, only machine A was available. Machines B and C remain unenrolled, so the v1 field evidence gate cannot complete with the current hardware.

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
