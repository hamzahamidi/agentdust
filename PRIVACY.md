# Privacy

Last updated: 2026-10-07

AgentDust has no account service, analytics service, or hosted process-control service. The OpenAI skills package contains instructions only. It has no MCP server and does not connect ChatGPT or Codex to a computer.

## Information handled on your Mac

The local AgentDust program reads process metadata to classify processes, including process IDs, executable names, start times, and session relationships. Its Claude Code hooks read supported session event fields so the program can associate helper processes with a session.

The local journal stores an allowlist of session event metadata. It can include Claude Code session IDs and tool-use IDs, timestamps, process identity metadata, and keyed digests of working directories and subagent IDs. It does not store shell commands, tool output, process environments, transcript paths, or raw working-directory paths. Existing journal records from older versions may contain raw subagent IDs until those records are removed by journal retention or by you.

The `agentdust doctor --json` report includes process IDs, executable basenames, process ages, classifications, evidence, and working-directory relationships. The human-readable `agentdust doctor` report can also show command arguments and directory paths. The `agentdust disk --json` report contains size totals, categories, and scan status, not file paths or file contents.

AgentDust stores its configuration, install secret, and journal in its local data directory. The default on macOS is `~/Library/Application Support/agentdust`; `AGENTDUST_DATA_DIR` can select another directory. Data remains there until local journal retention changes it or you remove it. `agentdust setup --remove` removes the Claude Code integration and does not delete this data. You control whether to share a report, and which fields to include, with an AI service or another person.

## Information shared with services

The AgentDust runtime does not send process data, disk reports, or telemetry to the project author or to an AgentDust server. The skills package itself sends no data. If you configure a local AgentDust MCP connection in an AI client, the tool results you request are returned to that client for the conversation. The client's own privacy terms govern how it handles conversation data. For ChatGPT, see [OpenAI's Privacy Policy](https://openai.com/policies/row-privacy-policy/).

If you voluntarily open a GitHub issue or security report, GitHub receives the information you submit under GitHub's terms. Avoid including process reports, commands, paths, or identifiers unless they are needed to explain the issue.

## Your choices

- Do not paste a report into a chat if you do not want that information processed by that service.
- Remove identifiers and other fields before sharing a report.
- Run `agentdust setup --remove` to remove the Claude Code integration.
- Delete the configured AgentDust data directory to remove its local configuration, secret, and journal. This does not uninstall the AgentDust binary.

AgentDust is open source. Questions about this policy can be raised in the [repository](https://github.com/hamzahamidi/agentdust/issues).
