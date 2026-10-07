---
name: disk
description: Explain AgentDust disk reports and guide local setup. Use when the user asks about Claude Code storage, history, caches, worktrees, or AgentDust disk output.
---

# Claude Code disk usage

This skill package contains guidance only. It does not connect to a Mac or provide live disk access. Live inspection requires a separately configured local AgentDust MCP connection. AgentDust 0.3.0 supports Claude Code on Apple silicon; setting it up there does not connect it to ChatGPT or Codex.

If the user provides `agentdust disk --json`, explain the logical and allocated byte totals by root and category. Allocated bytes are not reclaimable space. History, memory, plugins and worktrees can contain work the user still needs. The report contains sizes and scan status, not file paths or contents.

Report missing, unavailable and partial roots and skipped counts. A zero in a partial or unavailable root does not mean it is empty. The worktree root is scoped to the MCP server's working directory; other projects and custom locations are excluded.

Do not read transcripts, credentials or other file contents to complete the report. Do not delete files or infer permission to delete from a category. If the user has not provided a report and live `agentdust_disk` is unavailable, say this package does not create that connection. Explain how to obtain the report through an explicitly configured local AgentDust MCP connection or the local CLI. Do not change client settings or install AgentDust as part of this skill.
