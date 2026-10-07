# v1 readiness

Status checked 2026-10-07. A merge, release or test pass closes only the evidence it directly verifies.

| Criterion | Status | Evidence and remaining work |
| --- | --- | --- |
| 1. Install | Open | On 2026-10-07, `agentdust setup --yes` from a local 0.2.0 release-profile binary took 1.42 seconds in a clean temporary `HOME` and `CLAUDE_CONFIG_DIR`; the post-setup check and removal passed. This does not verify the Homebrew pour. Verify the prebuilt Homebrew install, setup execution under 5 seconds excluding review and consent time, diff review, controlled discovery, typed approval and the one approved `SIGTERM`. See [release instructions](../release.md). |
| 2. Hook cost | Open | On 2026-10-07, the local release-profile binary measured p50/p95 of 3.30/3.67 ms with `cwd`, 3.71/5.96 ms without `cwd`, 4.13/6.74 ms for `SessionStart`, and 8.54/10.64 ms for the first hook in an empty data directory. The captured scenario benchmark replayed 108 hook events from five sanitized Claude Code 2.1.292 fixtures sequentially and measured p50/p95 of 3.96/9.44 ms. It includes the recorded three-session and subagent scenarios, but does not reproduce live concurrency. Abrupt exit and Agent Teams are unrecorded. Measure the complete workload on the M6 release. Earlier measurements in [provenance notes](../m2/provenance.md) used a different workload. |
| 3. Idle | 0.2.0 baseline observed; M6 release gate open | On 2026-10-07, the Homebrew 0.2.0 MCP server answered a `server/discover` request carrying protocol metadata `2026-07-28`, then received no further requests for 81.3 seconds. During that period, 61 samples were collected at one second intervals; each reported 0.0% CPU. A separate 60 second run sampled RSS from 2,336 to 3,024 KiB. These are 0.2.0 baseline observations, not evidence for the release containing M6. Repeat both measurements for that release. |
| 4. Actionability | Code and CI evidence | Apply rules and typed approval are implemented and covered by [apply tests](../m3/apply.md) and [PR #25 CI](https://github.com/hamzahamidi/agentdust/pull/25). Exercise the release flow on a clean account under criterion 1 and the full multi-agent cases under criterion 9. |
| 5. Classifier | Fixture and CI evidence | The ground-truth corpus and classifier checks are recorded in [fixture documentation](../fixtures.md) and `crates/agentdust-testkit/tests/fixture_corpus.rs`. The suspect age threshold still needs real false-proposal evidence. |
| 6. Privacy | Test and CI evidence | The data-directory privacy checks are in `crates/agentdust/tests/privacy.rs` and `crates/agentdust/tests/hook_session.rs`. The latest CI passed in [PR #26](https://github.com/hamzahamidi/agentdust/pull/26). |
| 7. Supply chain | Evidence verified for 0.2.0 | `Cargo.lock` has been present since the first Rust code commit, `c26f21d`. The [0.2.0 release workflow](https://github.com/hamzahamidi/agentdust/actions/runs/37355787746) succeeded and compared two independent macOS arm64 builds. On 2026-10-07, the archive and SBOM checksums passed, and GitHub attestations verified for the archive, SBOM and unpacked binary. |
| 8. Field evidence | Open | Three independent Apple silicon Macs must run 0.x builds for at least two weeks. Count sessions, proposed kills, false positives and failures. |
| 9. Multi-agent isolation | Open | [PR #25](https://github.com/hamzahamidi/agentdust/pull/25) adds synthetic ownership and apply regressions. [PR #26](https://github.com/hamzahamidi/agentdust/pull/26) adds the first real capture for two active subagents on Claude Code 2.1.289. New 2.1.292 captures cover the session and subagent scenarios except abrupt exit and Agent Teams, and replay through the adapter test. The full real-client matrix and process identity validation remain open. |

M6 has a separate one-machine, two-week dogfood gate with attribution errors, false proposals, blocked applies, signals, failures and hook latency recorded. Run that observation during the three-machine period, with one machine collecting the fuller M6 metrics. The [dogfood record](../m6/dogfood.md) has the fields.

## Remaining work

1. Run the clean-account Homebrew install and controlled apply flow.
2. Select candidate Claude Code versions, capture and replay the full M6 scenario matrix for each, then list versions with complete passing matrices as supported.
3. Measure hook latency during that workload. Repeat interval CPU and resident memory measurements for the M6 release.
4. Run the three-machine observation period for at least two weeks, collecting the M6 metrics on one machine.
5. Reassess suspect false proposals and close the remaining evidence gates before calling v1 ready.

The local Claude Code CLI reports 2.1.292 on macOS 26.6.2 arm64. Five sanitized M6 captures cover its session and subagent scenarios. Abrupt exit and Agent Teams remain unrecorded, so 2.1.292 is not listed as supported. The 2.1.289 fixture covers only its listed scenario.
