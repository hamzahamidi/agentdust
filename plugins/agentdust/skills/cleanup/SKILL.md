---
name: cleanup
description: Inspect or clean processes left running after Claude Code sessions with AgentDust. Use when the user asks to find, review, or stop leftover processes.
---

# AgentDust cleanup

Use the AgentDust MCP tools to inspect processes left by Claude Code sessions. This skill requires the `agentdust` MCP server from `agentdust setup`.

1. For a read-only inspection request, call `agentdust_doctor` and explain the classes and evidence. Do not infer that a process is safe to stop from its name, parent PID, age, or CPU use alone.
2. For a cleanup request, call `agentdust_plan` to get a fresh plan. Show the eligible processes and their evidence, then apply only the items the user asked to clean. Use the plan ID and item IDs from that fresh response.
3. Call `agentdust_apply` for the selected items. AgentDust presents the typed approval form and generates the code. The user must review the items and type the code. Never invent, copy, or submit an approval code.
4. If the user declines or cancels, stop. If the client cannot show the form, tell the user to run `agentdust apply` in a terminal outside Claude Code. Do not run that command yourself.
5. Report the result returned by `agentdust_apply`. Do not repeat an apply call for an item that already has a result.

If the MCP tools are missing, tell the user to run `agentdust setup` and reconnect Claude Code. Do not edit Claude Code settings or install AgentDust as part of this skill.
