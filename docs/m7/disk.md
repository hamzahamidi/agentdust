# Claude Code disk report

`agentdust disk` reports local Claude Code storage without opening regular files or deleting anything. The MCP tool `agentdust_disk` returns the same report as `agentdust disk --json`.

## Run it

```bash
agentdust disk
agentdust disk --json
```

Run the command from a project root to include that project's `.claude/worktrees`. In Claude Code, ask: “Use AgentDust to show where Claude Code uses disk space.” The optional plugin provides `/agentdust:disk`. Reconnect Claude Code after upgrading so it discovers the new MCP tool.

## What is included

| Root label | Location | Scope |
| --- | --- | --- |
| `claude_config` | `~/.claude`, or absolute `CLAUDE_CONFIG_DIR` | Configuration, session history, snapshots, plugins and other entries |
| `current_project_worktrees` | `.claude/worktrees` under the command or MCP server's working directory | That project's worktrees only |

The MCP tool takes no arguments. Its working directory is the directory where Claude Code starts the AgentDust server. The report does not discover other repositories, follow custom worktree locations, or include scratchpads, temporary attachments, the Claude Code executable, Claude Desktop or Cowork storage. A missing root is `not_present`. A symlink anywhere in a root path makes that root `unavailable`.

Root labels and categories are fixed. Neither CLI nor MCP output contains absolute paths, entry names, file contents, prompts or credentials. The report is not saved in the AgentDust journal.

## Categories describe purpose

| Category | Entries |
| --- | --- |
| `rebuildable` | `cache/changelog.md`, `remote-settings.json` and the two `policy-limits.json` files that Claude refreshes |
| `history` | `projects`, `file-history`, prompt history, plans, backups, attachments, paste and image caches, feedback, usage reports and historical stats |
| `worktree` | The current project's `.claude/worktrees` root |
| `application_state` | Settings, credentials, plugins, skills, instructions, subagent memory, background jobs, session state, task lists, shell snapshots and logs |
| `unknown` | Entries without a classification, including other files under `cache` |

Auto memory under `projects/<project>/memory` is `application_state`, even though the rest of `projects` is `history`. Installed plugins are application state, including their cache directory. A category does not authorize deletion. History and worktrees can contain work you still need.

The directory rules follow [Claude Code's directory reference](https://code.claude.com/docs/en/claude-directory). New names remain `unknown` until a rule exists.

## Read the sizes

**Logical bytes** sum the length of regular files. **Allocated bytes** sum `st_blocks × 512` for regular files and directories. Hard links and overlapping roots are counted once by device and inode. When links span categories, the first encountered entry receives the size; traversal order is not fixed.

Allocated bytes are not an estimate of reclaimable space. APFS clones, compression and snapshots can share or retain blocks. Sparse files can have a logical size larger than their allocation. This is a live inventory rather than an atomic filesystem snapshot, so concurrent writes can change the numbers.

## Partial reports and limits

The two roots share a budget of 100,000 entries, 64 directory levels and 10 seconds checked between directory entries and before each root. A slow filesystem call can exceed the time budget. One MCP disk scan runs at a time; another call receives a busy error while it runs.

A `partial` root has an error, a depth limit, an excluded filesystem or an exhausted budget. `unavailable` means the root could not be opened. `already_counted` means an earlier root encountered the same directory; consult that root's status for coverage. Zero bytes in any of those states do not mean the directory is empty.

The `skipped` counts record symbolic links, special files, filesystem boundaries, duplicate inodes, errors and depth limits. Symbolic links and special files are excluded by policy even from a `complete` root. A complete report covers regular files and directories that remain on the root's filesystem. Directory operations use file descriptors, do not follow symbolic links, and verify directory identity after opening. Raw filesystem errors are not returned.

The CLI returns exit status `0` when it produces a report, including a partial or unavailable root. Scripts must inspect the root statuses and `limit_reached` to determine coverage. `application_state` describes purpose, not a retention rule. Claude Code can automatically remove some of that state.
