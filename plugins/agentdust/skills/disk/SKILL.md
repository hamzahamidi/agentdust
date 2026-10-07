---
name: disk
description: Use AgentDust's local MCP tool to report Claude Code disk usage when the tool is connected and the user asks about storage, history, caches or worktrees. Reports sizes and deletes nothing.
---

# Claude Code disk usage

Call `agentdust_disk` with no arguments when the local AgentDust MCP server is available. This skill package does not install or connect that server. A cloud chat without an explicit local connection cannot read disk usage from the user's Mac.

Explain the logical and allocated byte totals by root and category. Allocated bytes are not reclaimable space. History, memory, plugins and worktrees can contain work the user still needs.

Report missing, unavailable and partial roots and skipped counts. A zero in a partial or unavailable root does not mean it is empty. The worktree root is scoped to the MCP server's working directory; other projects and custom locations are excluded.

Do not read transcripts, credentials or other file contents to complete the report. Do not delete files or infer permission to delete from a category. If the tool is missing, ask the user to upgrade AgentDust and reconnect Claude Code.
