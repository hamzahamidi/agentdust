---
name: disk
description: Show Claude Code disk usage with AgentDust when the user asks about agent storage, history, caches or worktrees. Reports sizes and deletes nothing.
---

# Claude Code disk usage

Call `agentdust_disk` with no arguments. This requires the AgentDust MCP server registered by `agentdust setup`.

Explain the logical and allocated byte totals by root and category. Allocated bytes are not reclaimable space. History, memory, plugins and worktrees can contain work the user still needs.

Report missing, unavailable and partial roots and skipped counts. A zero in a partial or unavailable root does not mean it is empty. The worktree root is scoped to the MCP server's working directory; other projects and custom locations are excluded.

Do not read transcripts, credentials or other file contents to complete the report. Do not delete files or infer permission to delete from a category. If the tool is missing, ask the user to upgrade AgentDust and reconnect Claude Code.
