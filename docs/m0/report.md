# M0 report

Results of the M0 risk spike against the exit criteria in [ROADMAP.md](../../ROADMAP.md). Each line links the run or file it was read from.

| Exit criterion | Result | Evidence |
| --- | --- | --- |
| Hook p50 under 10 ms on the release binary | Met. p50 2.50 ms, p95 2.85 ms on `macos-15`; p50 2.68 ms, p95 3.36 ms on an Apple silicon Mac | [CI run 37163003161, macos job](https://github.com/hamzahamidi/agentdust/actions/runs/37163003161/job/111320158276) |
| Elicitation matrix recorded for Claude Code, Codex and Cursor | Partly met. Codex 0.156.1 (8 scenarios) and Claude Code 2.1.289 (7 scenarios) recorded. Cursor not run (see Blockers) | [client-matrix.md](client-matrix.md) |
| Two clean builds produce a byte-identical binary | Met. Both `macos-15` builds hash to `78ee5e6b298081689f15d726b644079ca43bc8bea0f25fb8bff3e58467eec60b` | [dry run, package job](https://github.com/hamzahamidi/agentdust/actions/runs/37163204485) |
| Tarball, checksum and attestation produced | Met. `agentdust-0.0.0-darwin-arm64.tar.gz` (`94f879c4d4677d132d701a1d213d1f7fd5ad50db25ed6589d14cb6cc9ce71697`), a `SHA256SUMS` that passes `shasum -c`, and a build provenance attestation that `gh attestation verify` accepts with signer `release-dry-run.yml@refs/heads/main` | [dry run](https://github.com/hamzahamidi/agentdust/actions/runs/37163204485), [Rekor entry](https://search.sigstore.dev?logIndex=3075810468) |
| Homebrew formula installs the tarball and `agentdust version` runs | Met. `brew install agentdust-local/m0/agentdust` from a `file://` tarball printed `agentdust 0.0.0` | [dry run, package job](https://github.com/hamzahamidi/agentdust/actions/runs/37163204485) |
| Cursor questions answered | Not met. Cursor is not installed on the test Mac | [cursor-experiments.md](cursor-experiments.md) |

Repository basics from the roadmap are in place: MIT licence, `SECURITY.md` with private vulnerability reporting, squash-only merges with required `linux` and `macos` checks, `cargo deny` and `cargo audit` in CI, every action pinned to a commit SHA, and the release toolchain recorded in [release/toolchain.json](../../release/toolchain.json) (`macos15` image 20260907.0337.1, Xcode 16.4, SDK 15.5, Rust 1.99.0). The `KERN_PROCARGS2` parser ran 27,196,230 fuzz inputs in 61 seconds without a crash.

## Blockers

- Cursor rows and both Cursor experiments: Cursor is not installed. They move to M5.

## Inputs for M1

- rmcp 3.5 defaults do not satisfy protocol 2026-07-28 on their own. `tools/list` needs `ttlMs` and `cacheScope`, which Claude Code 2.1.282 enforces. M3's server keeps the cache fields and the test that pins them.
- Codex 0.156.1 negotiates protocol 2025-06-18 and uses server-initiated elicitation. It accepts only `$schema`, `type`, `properties` and `required` at the top of a form schema and silently reports any other form as cancelled. M3 builds every form through one schema function with the two schema key tests.
- Codex's form has no decline control and cannot be submitted empty, so Esc (`cancelled`) is the only refusal a Codex user can give. M3 treats `cancelled` and `declined` alike.
- The approval code was rendered only in the client's form on both paths. The forged run waited for the person in Codex (legacy path) and in Claude Code (retry path), and no auto-approval was observed.
- Claude Code 2.1.289 negotiates protocol 2026-07-28 and answers through the retry path. Its form has a Decline button that is distinct from Esc, and a required field that blocks an empty Accept. M3 treats `declined`, `cancelled` and `expired` as refusals.
- Deferred review findings to settle before the journal schema is frozen: the reader takes no lock and does not refuse symlinks, a non-UTF-8 line aborts a read, newer-version lines are not counted apart from malformed ones, and the record fields `wall_ts_ms` and `mono_ns` differ from `wall_ts` and `mono_ts` in spec section 3.2.
- CI installs `cargo-fuzz` as a musl build that defaults to the musl target, so the fuzz build passes `--target x86_64-unknown-linux-gnu`. Pin the `cargo-fuzz` version and a dated nightly.
- The Codex scenarios ran through a scripted pseudo-terminal driver that renders the screen and types answers. It lives outside the repository; M1 decides whether to add it for matrix reruns.
