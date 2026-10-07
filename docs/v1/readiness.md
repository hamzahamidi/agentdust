# v1 readiness

Status checked 2026-10-07. A merge, release or test pass closes only the evidence it directly verifies.

| Criterion | Status | Evidence and remaining work |
| --- | --- | --- |
| 1. Install | Open | The clean-account install and setup flow is unverified. Verify the prebuilt Homebrew install, setup execution under 5 seconds excluding review and consent time, diff review, controlled discovery, typed approval and the one approved `SIGTERM`. See [release instructions](../release.md). |
| 2. Hook cost | Open | Measure release-binary p50 under 10 ms and p95 under 20 ms during criterion 9's full workload. Earlier measurements in [provenance notes](../m2/provenance.md) used a different workload. |
| 3. Idle | 0.2.0 baseline recorded; repeat for M6 release | On 2026-10-07, the Homebrew 0.2.0 MCP server answered a `server/discover` request carrying protocol metadata `2026-07-28`, then idled for 60.0 seconds. Across 61 `ps` snapshots, reported CPU was 0.0% each time and sampled RSS ranged from 2,336 to 3,024 KiB. The `ps` CPU percentage is averaged over process lifetime, including startup and discovery. Repeat for the release containing M6. |
| 4. Actionability | Code and CI evidence | Apply rules and typed approval are implemented and covered by [apply tests](../m3/apply.md) and [PR #25 CI](https://github.com/hamzahamidi/agentdust/pull/25). Exercise the release flow on a clean account under criterion 1 and the full multi-agent cases under criterion 9. |
| 5. Classifier | Fixture and CI evidence | The ground-truth corpus and classifier checks are recorded in [fixture documentation](../fixtures.md) and `crates/agentdust-testkit/tests/fixture_corpus.rs`. The suspect age threshold still needs real false-proposal evidence. |
| 6. Privacy | Test and CI evidence | The data-directory privacy checks are in `crates/agentdust/tests/privacy.rs` and `crates/agentdust/tests/hook_session.rs`. The latest CI passed in [PR #26](https://github.com/hamzahamidi/agentdust/pull/26). |
| 7. Supply chain | Evidence verified for 0.2.0 | `Cargo.lock` has been present since the first Rust code commit, `c26f21d`. The [0.2.0 release workflow](https://github.com/hamzahamidi/agentdust/actions/runs/37355787746) succeeded and compared two independent macOS arm64 builds. On 2026-10-07, the archive and SBOM checksums passed, and GitHub attestations verified for the archive, SBOM and unpacked binary. |
| 8. Field evidence | Open | Three independent Apple silicon Macs must run 0.x builds for at least two weeks. Count sessions, proposed kills, false positives and failures. |
| 9. Multi-agent isolation | Open | [PR #25](https://github.com/hamzahamidi/agentdust/pull/25) adds synthetic ownership and apply regressions. [PR #26](https://github.com/hamzahamidi/agentdust/pull/26) adds one real capture for two active subagents on Claude Code 2.1.289. The full real-client matrix remains open. |

M6 has a separate one-machine, two-week dogfood gate with attribution errors, false proposals, blocked applies, signals, failures and hook latency recorded. Run that observation during the three-machine period, with one machine collecting the fuller M6 metrics. The [dogfood record](../m6/dogfood.md) has the fields.

## Remaining work

1. Run the clean-account Homebrew install and controlled apply flow.
2. Select candidate Claude Code versions, capture and replay the full M6 scenario matrix for each, then list versions with complete passing matrices as supported.
3. Measure hook latency during that workload, then repeat the idle CPU and resident memory measurement for the M6 release.
4. Run the three-machine observation period for at least two weeks, collecting the M6 metrics on one machine.
5. Reassess suspect false proposals and close the remaining evidence gates before calling v1 ready.

The current local Claude Code CLI reports 2.1.292 on macOS 26.6.2 arm64. No M6 fixture has been recorded for that version. The recorded 2.1.289 fixture covers only its listed scenario and is not a version-wide compatibility claim.
