# Dependency allowlist

Every direct dependency has one line here. A pull request that adds a dependency adds its line.

| Crate | Used by | Why |
| --- | --- | --- |
| `libc` | agentdust-core, agentdust-bench (not shipped) | `proc_pidinfo`, `proc_pidpath`, `sysctl`, `clock_gettime`, `statfs`, `geteuid` and the `O_NOFOLLOW` and `O_NONBLOCK` flags |
| `serde`, `serde_json` | all crates | hook payloads, journal records, MCP results |
| `thiserror` | agentdust-core, agentdust-agents, agentdust-bench (not shipped) | error types |
| `hmac`, `sha2` | agentdust-core | HMAC-SHA256 for the keyed working directory and session digests (spec 3.2). RustCrypto crates, MIT or Apache-2.0. `sha2` has default features off, so the OID support crate is not built |
| `rmcp` | agentdust-mcp | official MCP SDK (Tier 1), elicitation for both protocol generations |
| `tokio` | agentdust-mcp, agentdust | runtime for the `mcp` subcommand only |
| `proptest` | agentdust-core (tests only) | property tests for identity revalidation. Default features are off, so the fork and timeout support crates are not built |
| `libfuzzer-sys` | fuzz (not shipped) | fuzz targets |
