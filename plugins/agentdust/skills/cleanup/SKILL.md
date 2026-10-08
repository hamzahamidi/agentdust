---
name: cleanup
description: Explain AgentDust process reports and guide safe local cleanup. Use when the user asks about processes left by Claude Code sessions, AgentDust doctor output, or cleanup.
---

# AgentDust cleanup

This skill package contains guidance only. It does not connect to a Mac or provide live process access. Live inspection requires a separately configured local AgentDust MCP connection. The guidance package has its own version, separate from the installed binary. AgentDust supports Claude Code on Apple silicon; setting it up there does not connect it to ChatGPT or Codex.

## Explain a report the user provides

Ask the user to paste `agentdust doctor --json` for process classifications or `agentdust disk --json` for disk totals. Do not ask for the terminal `doctor` output unless needed, because it can include command arguments and directory paths. The JSON doctor report still contains process IDs, executable basenames, ages, classes, and evidence, so the user should remove anything they do not want to share.

Explain classes and evidence as reported. A process name, parent PID, age, or CPU use alone does not show that a process is safe to stop. A pasted report is a snapshot and cannot authorize or support a later signal.

## If a local AgentDust MCP connection is available

1. For read-only inspection, call `agentdust_doctor` and explain the classes and evidence.
2. For cleanup, call `agentdust_plan` to obtain a fresh plan. Show the eligible processes and evidence, then use only the plan ID and item IDs from that response for items the user requested.
3. Call `agentdust_apply` for those items. AgentDust presents the approval form and generates the code. The user must review the items and type the code. Never invent, copy, or submit an approval code.
4. If the user declines or cancels, stop. If the client cannot show the form, tell the user to run `agentdust apply` locally. Do not run that command yourself.
5. Report the result returned by `agentdust_apply`. Do not repeat an apply call for an item that already has a result.

If live tools are unavailable, say this package does not create that connection. Explain the report the user provides, or direct them to AgentDust's local Claude Code setup. Do not change client settings or install AgentDust as part of this skill.

## Automatic cleanup when the installed binary supports it

If `agentdust_auto_status` is available, call it to explain the human-enabled policy, worker state, recent results and review items. Published `1.0.0` has no automatic cleanup. Do not claim it is enabled unless the status reports it.

The human enables an exact canonical session-start directory using `agentdust auto enable PROJECT` in their own foreground terminal. A local worker continues after Claude Code exits. Only proven owned-ended helpers in enabled scope are eligible; uncertain cases use the existing typed approval flow. Do not invoke policy mutation commands, supply consent text, remove keeps, or broaden the enabled scope. Kept identities stay protected during manual apply too.

Explain survivors and prior-attempt cases without retrying automatically. For uncertain items the user wants to handle, request a fresh plan and follow the manual approval steps above. A pasted automatic report is historical evidence, not approval.
