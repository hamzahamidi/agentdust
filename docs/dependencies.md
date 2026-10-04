# Dependency allowlist

Every direct dependency has one line here. A pull request that adds a dependency adds its line.

| Crate | Used by | Why |
| --- | --- | --- |
| `libc` | agentdust-core, agentdust-bench and agentdust-testkit (not shipped) | `proc_pidinfo`, `proc_pidpath`, `sysctl`, `clock_gettime`, `statfs`, `geteuid`, `getentropy` for the install secret and the session tag, `renamex_np` and `renameat2` for the exclusive install of the secret, the `O_NOFOLLOW`, `O_NONBLOCK` and `O_NOCTTY` flags (the last for the env file of the host), and for the process inventory `proc_listallpids`, `proc_pidinfo` with `PROC_PIDTASKINFO` (cumulative CPU time) and `PROC_PIDVNODEPATHINFO` (working directory), and `sysctlbyname` for `hw.tbfrequency` (the unit of CPU time). The testkit fixture uses `signal`, `setsid` and `getsid`, and its tests use `kill` |
| `serde`, `serde_json` | all crates (the fuzz crate uses `serde_json` only) | hook payloads, journal records, MCP results, the round trip check in the payload fuzz target |
| `thiserror` | agentdust-core, agentdust-agents, agentdust-bench and agentdust-testkit (not shipped) | error types |
| `hmac`, `sha2` | agentdust-core | HMAC-SHA256 for the keyed working directory and session digests (spec 3.2). RustCrypto crates, MIT or Apache-2.0. `sha2` has default features off, so the OID support crate is not built |
| `rmcp` | agentdust-mcp | official MCP SDK (Tier 1), elicitation for both protocol generations |
| `tokio` | agentdust-mcp, agentdust | runtime for the `mcp` subcommand only |
| `proptest` | agentdust-core (tests only) | property tests for identity revalidation, the journal codec, truncated streams and the reader. Default features are off, so the fork and timeout support crates are not built |
| `libfuzzer-sys` | fuzz (not shipped) | fuzz targets for the `KERN_PROCARGS2` parser, the journal decoder, the journal reader and the hook payload parser |
