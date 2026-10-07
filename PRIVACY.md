# Privacy

Last updated: 2026-10-07

AgentDust has no account service, analytics service, or hosted process-control service. The OpenAI skills package contains instructions only. It has no MCP server and does not connect ChatGPT or Codex to a computer.

## Information handled on your Mac

The local AgentDust program reads process metadata to classify processes, including process IDs, executable names, start times, user IDs, and session relationships. Its Claude Code hooks read supported session event fields so the program can associate helper processes with a session.

The journal stores an allowlist of session event metadata. It can include Claude Code session IDs and tool-use IDs, timestamps, process identity metadata, and keyed digests of working directories and subagent IDs. It does not store shell commands, tool output, process environments, transcript paths, or raw working-directory paths. Older journal records may contain raw subagent IDs. AgentDust 0.3.0 does not automatically prune the journal during normal hook operation, so journal data remains until you remove it.

The `agentdust doctor --json` report includes process IDs, executable basenames, process ages, classifications, evidence, and working-directory relationships. The human-readable `agentdust doctor` report can also show command arguments and directory paths. The `agentdust disk --json` report contains size totals, categories, and scan status, not file paths or file contents.

## Local files and retention

The default data directory on macOS is `~/Library/Application Support/agentdust`. `AGENTDUST_DATA_DIR` can select another directory.

| Local data | Purpose and retention |
| --- | --- |
| `config.toml` | Stores whether cleanup is enabled. It remains until you delete it or the data directory. `agentdust setup --remove` does not remove it. |
| `install.secret` | Random secret used to create keyed digests. It remains until you delete it or the data directory. |
| `journal.jsonl` and any rotated generations | Store the allowlisted hook event records described above. AgentDust 0.3.0 does not automatically prune these during normal hook operation. They remain until you delete them or the data directory. |
| `manifest.json` | Tracks AgentDust-owned Claude Code settings entries, including their target, command, arguments, and integrity hash. It is removed when setup removal leaves no tracked entries. |
| Claude Code settings | Store the hook and MCP command configuration outside the AgentDust data directory. `agentdust setup --remove` removes only matching entries recorded as AgentDust-owned. Other settings remain under your control. |
| `audit.log` and `audit.log.1` | Record cleanup attempts and results, including time, plan and item IDs, process ID, start time, user ID, class, evidence, executable basename, and result. They do not include command arguments or paths. Each file is limited to 5 MiB. On rotation, the prior `audit.log.1` is replaced, so the two files hold up to 10 MiB. There is no age-based expiry; the files remain until a later rotation or deletion. |
| `inspection/<plan-id>.txt` | Holds a sanitized plan report with process IDs, executable names, ages, classes, working-directory relationships, and evidence. A plan expires after 10 minutes. Its report is removed on the next plan or approval call after expiry, when the server stops, or at the next server start. If the server stays idle, an expired report can remain until one of those events. |
| `audit.lock` | Empty synchronization file. It remains until you delete it or the data directory. |
| `locks/<item-id>.lock` | Empty per-process synchronization file. It is normally removed when that process item finishes. An interrupted operation can leave one behind; a later operation for that item reuses it. |

You control whether to share a report, and which fields to include, with an AI service or another person. Remove process identifiers and other fields you do not want to share.

## Information shared with services

The AgentDust runtime does not send process data, disk reports, or telemetry to the project author or to an AgentDust server. The skills package itself sends no data. If you configure a local AgentDust MCP connection in an AI client, the tool results you request are returned to that client for the conversation. The client's own privacy terms govern how it handles conversation data. For ChatGPT, see [OpenAI's Privacy Policy](https://openai.com/policies/row-privacy-policy/).

If you voluntarily open a GitHub issue or security report, GitHub receives the information you submit under GitHub's terms. Avoid including process reports, commands, paths, or identifiers unless they are needed to explain the issue.

## Your choices

- Do not paste a report into a chat if you do not want that information processed by that service.
- Remove identifiers and other fields before sharing a report.
- Run `agentdust setup --remove` to remove the Claude Code integration.
- Delete the configured AgentDust data directory to remove its local configuration, secret, journal, audit records, reports, and lock files. This does not uninstall the AgentDust binary or remove Claude Code settings.

AgentDust is open source. Questions about this policy can be raised in the [repository](https://github.com/hamzahamidi/agentdust/issues).
