# AgentDust

Finds the processes that AI coding agents (Claude Code, Codex, Cursor) leave running after their sessions end, and stops them after you approve each one with a typed code.

Status: in development. Nothing is released yet. The plan is in [ROADMAP.md](ROADMAP.md) and the design in [docs/superpowers/specs](docs/superpowers/specs).

## Build

```bash
cargo build --release
```

The binary is `target/release/agentdust`. It runs on macOS on Apple silicon.

## Network access

None. The binary contains no network code.
