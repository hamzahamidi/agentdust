# `agentdust setup` and `agentdust status`

`setup` connects AgentDust to Claude Code: four hook entries in `settings.json` and one MCP server registered through the `claude` CLI. It is spec 8.1 and 8.2, and the tests for S17 and S18. `status` is a read-only report. Neither command signals a process.

```
agentdust setup [--check | --remove] [--yes]
agentdust status
```

| Exit code | Meaning |
| --- | --- |
| 0 | Done, nothing to change, or `--check` found a complete installation |
| 1 | A step failed, the user declined, setup is not installed or is modified, or a refusal (no terminal and no `--yes`, a future manifest version) |
| 2 | Usage error: `--check` with `--remove` or `--yes`, a repeated flag, an unknown flag or argument |

## What setup writes

| Resource | Owner of the change | Where |
| --- | --- | --- |
| Four hook groups: `SessionStart`, `SessionEnd`, `PreToolUse` and `PostToolUse` (matcher `Bash`) | `agentdust`, by splicing text into the file | `settings.json` in `CLAUDE_CONFIG_DIR`, or `~/.claude` when it is unset |
| The MCP server `agentdust` | the `claude` CLI: `claude mcp add --scope user agentdust -- <binary> mcp` | the CLI's user configuration |
| The ownership manifest | `agentdust` | `manifest.json` in the data directory, mode 0600 |

Every hook group has this shape. The timeout is in seconds, as Claude Code reads it, and is the 10 seconds of spec 4.4. The two session events have no matcher.

```json
{
  "matcher": "Bash",
  "hooks": [
    { "type": "command", "command": "/opt/homebrew/bin/agentdust hook claude", "timeout": 10 }
  ]
}
```

The binary path is the path the process was started with, made absolute and not resolved through symlinks. A path of the form `<prefix>/Cellar/<formula>/<version>/bin/<name>` is rewritten to `<prefix>/bin/<name>`, so an upgrade does not leave the settings pointing at a removed directory. A path with a character the shell treats as special is wrapped in single quotes, because Claude Code runs the command through a shell.

## The flow

1. Read the manifest. A version other than 1, an unknown field or a file that is not safe to open stops the command before anything else.
2. Run `claude mcp get agentdust` when the CLI is on `PATH`, then read `settings.json`. The order matters (see the end of this page).
3. Print the plan: the three resolved locations (config directory, binary, data directory), a unified diff of `settings.json`, the exact `claude` command, and the manifest change. `--yes` prints it too.
4. Ask `Apply these changes? [y/N]`. Only `y` and `yes` (any case) approve. Anything else, including an empty line and end of input, declines and exits 1. Without a terminal on both stdin and stdout, and without `--yes`, the command refuses before step 1 and runs nothing.
5. Apply three steps in order: write the manifest, replace `settings.json`, register the server. If a step fails, the completed steps are undone in reverse order and the report names each undo. A step that cannot be undone is listed with the reason. Nothing is retried.

Writing the manifest first means a crash leaves entries that the next run finds intact or adds again. It never leaves hooks that look like the user's own.

A plan with nothing to do prints `Nothing to change.` and does not ask.

## The settings file

- The file is opened without following symlinks and must be a regular file with one link and the current user as owner. Any mode is accepted, because `settings.json` is normally 0644. A symlink (even to a regular file), a hard link, a FIFO or a directory blocks the hooks.
- Edits are text splices on the byte spans of the objects and arrays that change. Every byte outside the insertion is unchanged, the insertion follows the indent the file already uses (default two spaces), and removing it restores the original bytes.
- The result is parsed again before it is written. A document with a duplicate key, nesting deeper than 128 levels, a top level that is not an object, a `hooks` that is not an object or an event that is not an array is refused and left as it is.
- The write goes to a temporary file in the same directory (mode 0600 at creation), is set to the original mode, synced, renamed over the file, and the directory is synced. A new file gets mode 0600, and a missing config directory is created with mode 0700. No backup copy is made, because settings can hold secrets.
- Just before the rename the file is read again. If its bytes differ from the bytes the diff was computed from, the step fails with `the file changed after it was read`.

A settings file that cannot be edited blocks only the hooks. Setup prints the JSON to add by hand and still registers the server. The run exits 1, and the next run is idempotent. `--check` and `status` read a symlinked file through the link, never write to it, and accept hooks that were added by hand.

## The manifest

`manifest.json` has `version` (1), `entries` and `agent_executables` (the resolved path of the `claude` CLI found on `PATH`). Each entry records `resource` (`hook` or `mcp_server`), `target` (the settings file, or the config directory for the server), `key` (the event, or the server name), `origin` (`created` or `pre_existing`), the exact `command` and `args`, and `hash`. The hash is SHA-256 over the installed value: the hook group as JSON with sorted keys and no whitespace, or the server name, command and arguments. Hook entries also record `created_hooks_key` and `created_event_key`, which say whether setup had to create the `hooks` object or the event array.

An entry that equals what setup would write and was already there is recorded as `pre_existing` and is never changed or removed. Entries are matched on the settings file path, and the server entry on the config directory, so `--check` and `--remove` act on the config directory that `CLAUDE_CONFIG_DIR` selects and leave entries recorded for another one alone.

| State | Meaning | `setup` | `--remove` |
| --- | --- | --- | --- |
| installed | recorded as created, the hash still matches, same binary | nothing | removes it |
| present | an equal entry the user already had | nothing | forgets it, never touches it |
| not installed | no entry | adds it | nothing |
| missing | recorded as created and gone | adds it again | forgets it |
| modified | recorded as created and its value or the registration changed | leaves it, exits 1 | leaves it, exits 1 |
| stale | the hash matches but the entry points at another binary | leaves it, exits 1 | removes it |

`--remove` deletes an event array or the `hooks` object only if the manifest says setup created it and it is now empty. For a settings file that setup only appended to, the result is the original file byte for byte. The manifest file is deleted when no entry is left.

## The MCP server

`claude mcp get agentdust` is the listing. It prints the command and the arguments on separate lines, and `claude mcp list` health checks every configured server. The output is read for `Scope`, `Type`, `Command`, `Args` and the `Environment` block. A registration matches only if the scope is the user scope, the transport is stdio, the command and arguments are exactly the expected ones and there is no environment.

| Found by `get` | Result |
| --- | --- |
| nothing (exit 1 and the message `No MCP server named`) | `add` runs, then `get` must show the exact registration |
| the exact registration, not recorded | recorded as `pre_existing` |
| another command, another scope or an environment | left alone and reported, exit 1 |
| anything else (another exit code, unparseable output, a timeout of 30 seconds) | not registered, the manual command is printed, exit 1 |

An exit code of 0 from `add` or `remove` is never taken as success. A CLI that exits 0 without registering anything, registers another command, or prints a listing that cannot be read fails the step with the reason. Removal is checked the same way.

Without the CLI on `PATH`, setup installs the hooks, prints `claude mcp add --scope user agentdust -- <binary> mcp` with the path quoted when needed, and exits 0. `setup --check` then exits 1, because it cannot verify the registration.

The CLI runs with stdin closed and inherits the environment, so `CLAUDE_CONFIG_DIR` reaches it only when it is set for `agentdust`.

## `status`

`status` never creates a file and never starts the `claude` CLI.

| Line | Content |
| --- | --- |
| `agentdust <version>` | the version |
| `data directory`, `data directory state` | the path, and `ok`, `missing` or `refused: <reason>` |
| `filesystem` | file system name, whether it is local, whether it is supported |
| `journal records`, `journal skipped lines` and four more counters | from the journal reader: skipped lines (with a detail line when not 0), truncated last line, unsupported version, duplicates removed, unsafe files. `journal: unavailable (<reason>)` replaces them when the reader refuses |
| `setup` | `installed`, `not installed`, `partly installed`, `modified` or `unknown`, from the manifest and `settings.json`. The registration counts as installed when the manifest records it |
| `apply` | `enabled`, `disabled (apply = false in config.toml)` or `disabled (config.toml cannot be used: <reason>)` |

## `config.toml`

The reader for the `apply` switch accepts blank lines, comments, table headers, and `key = value` lines with a bare key and a boolean, an integer or a single line string. `apply` at the top level must be `true` or `false`. A missing file means enabled. A file that is not opened safely (symlink, hard link, mode looser than 0600), is not UTF-8, sets `apply` twice, quotes the key or uses syntax outside that subset is reported as unusable and disables apply.

## Claude Code behaviour that setup works around

- The first `claude` command after a settings migration can rewrite `settings.json`. In Claude Code 2.1.289 a plain `claude mcp get` on an absent server changed `"model": "opus"` to `"opus[1m]"` and reordered the keys. Setup therefore runs the lookup before it reads the file, and the diff is computed from the file as the CLI left it.
- With the hooks installed, each `claude mcp get` ends with a `SessionEnd` hook call, so the journal receives one `session_end` record for a session it never saw start. `setup --check` and the verification step after `add` each cause one.

## Limits

What setup does not do:

- It handles Claude Code only. There is no Codex or Cursor adapter, and no `--claude-config-dir` flag: set `CLAUDE_CONFIG_DIR`.
- It does not change an entry that points at another binary. It reports it as stale and the user runs `agentdust setup --remove` and then `setup`.
- It does not edit a symlinked settings file, and it cannot verify a hand edit beyond finding an equal hook entry.
- It does not remove the data directory, the install secret or the journal. There is no `--purge-data`.

What Claude Code imposes:

- The registration lives in a file the CLI owns. Setup reads it only through `claude mcp get`, which prints one line for the arguments, so an argument that contains a space cannot be told apart from two arguments.
- `claude mcp get` health checks the server and ends with a `SessionEnd` hook call, with the effect listed above.
