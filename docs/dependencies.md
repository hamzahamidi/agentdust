# Dependency allowlist

Every direct dependency has one line here. A pull request that adds a dependency adds its line.

| Crate | Used by | Why |
| --- | --- | --- |
| `libc` | agentdust-core | `proc_pidinfo`, `proc_pidpath`, `sysctl`, `flock` and `clock_gettime` |
| `serde`, `serde_json` | all crates | hook payloads, journal records, MCP results |
| `thiserror` | agentdust-core, agentdust-agents | error types |
| `rmcp` | agentdust-mcp | official MCP SDK (Tier 1), elicitation for both protocol generations |
| `tokio` | agentdust-mcp, agentdust | runtime for the `mcp` subcommand only |
| `proptest` | agentdust-core (tests only) | property tests for identity revalidation. Default features are off, so the fork and timeout support crates are not built |
| `libfuzzer-sys` | fuzz (not shipped) | fuzz targets |
