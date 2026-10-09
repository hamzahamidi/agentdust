# Releasing AgentDust

A release starts when a tag `vX.Y.Z` is pushed. [`release.yml`](../.github/workflows/release.yml) checks the tag, builds the binary twice on `macos-15` and creates a draft GitHub Release with native and npm archives. A person checks and publishes the draft, which starts [`npm-publish.yml`](../.github/workflows/npm-publish.yml), then merges the pull request that puts the new formula in the Homebrew tap. The tap job reuses its release branch on retry. [`homebrew-tap-recovery.yml`](../.github/workflows/homebrew-tap-recovery.yml) can open the tap pull request for an already published release. This is spec 8.4 and S20.

## Semantic Versioning

AgentDust follows [Semantic Versioning 2.0.0](https://semver.org/). Version 1.0.0 establishes the compatibility baseline for the installed product: documented CLI commands and flags, machine-readable JSON output, MCP tool names and request/response contracts, documented configuration, and hook/plugin integration contracts. Human-readable prose, internal Rust interfaces and implementation details are not public API. Updates must preserve access to existing local data, or provide a documented migration.

| Change from 1.0.0 | Next version | Rule |
| --- | --- | --- |
| Backward-compatible bug fixes | `1.0.1` | Increment PATCH |
| Backward-compatible features or public API deprecations | `1.1.0` | Increment MINOR and reset PATCH |
| Incompatible public API changes | `2.0.0` | Increment MAJOR and reset MINOR and PATCH |
| Website or repository documentation only | No binary release required | Update the documentation without assigning a new binary version |

Choose the bump from the complete diff since the previous release. Commit prefixes and milestone numbers do not determine it. The version PR must identify the affected public contracts and explain why the bump matches them. When changes span multiple categories, use the largest required bump. A major version is a compatibility decision, not a measure of effort or feature count.

Published tags and package contents are immutable. A change to a published binary requires a new version. A documentation merge does not change the source commit or assets of an existing release.

Release tags use `vMAJOR.MINOR.PATCH` and match the package version after removing the `v`. The GitHub release title is the exact tag, such as `v1.0.0`. Product names and descriptive labels belong in the release body.

A SemVer prerelease has an identifier such as `1.1.0-rc.1`; GitHub's prerelease checkbox does not add that identifier. The current workflow and tap scripts accept only stable `MAJOR.MINOR.PATCH` identifiers. Use the release dry-run workflow for unpublished validation. Publishing SemVer prereleases requires explicit workflow and packaging support before creating those tags.

## What the workflow does

| Job | Runs on | What it does |
| --- | --- | --- |
| `verify` | `ubuntu-24.04` | Refuses a tag that is not `vMAJOR.MINOR.PATCH`, a tag that differs from `version` under `[workspace.package]`, a commit that `origin/main` does not contain, and a commit without a passing `linux` and `macos` check run from GitHub Actions. A commit with no check run at all is refused. The decisions are in `scripts/check_release.py` |
| `audit` | `ubuntu-24.04` | `cargo audit` against `Cargo.lock` |
| `build` | `macos-15`, two jobs (`copy` a and b) | `scripts/toolchain.py check --include-lock` against `release/toolchain.json`, then `cargo build --release --locked -p agentdust` with the `DEVELOPER_DIR` and `SDKROOT` of the record. Any drift in the runner image, Rust, Cargo, Xcode, SDK, flags or the `Cargo.lock` hash stops the build |
| `package` | `macos-15` | Compares the two binaries, checks that the binary prints `agentdust X.Y.Z`, builds native and npm archives from each binary and compares both archive pairs, checks npm installation and upgrades with isolated Claude and Codex setup and worker restarts, generates a CycloneDX SBOM from the workspace, writes `SHA256SUMS` for both archives and the SBOM, attests the binary, both archives and SBOM, writes the formula for the release download URL, and installs the generated formula through a local tap. It runs setup and `setup --check` with an isolated home and a fake Claude CLI, requires setup to print its diff before completion and finish in under 5 seconds, then runs an MCP apply test against the Homebrew-installed binary. A test client returns the requested approval code, and the harness verifies that only the selected process is signaled. |
| `release` | `ubuntu-24.04` | Creates a draft release for the existing tag with both archives, the SBOM and `SHA256SUMS`. It has no checkout of the source and the only write permission in the workflow besides the attestation |
| `tap-pr` | `ubuntu-24.04` | Uses the `release` environment secret `HOMEBREW_TAP_TOKEN` to open a pull request in the tap with the new formula. A retry reuses the existing branch and skips a duplicate commit or pull request. Without the secret the job prints a notice and does nothing |
| Homebrew tap recovery | `ubuntu-24.04` | Manually dispatched for a published version. Downloads and verifies its release assets, regenerates the formula, then reuses the version branch or opens its pull request |

The artifacts of a run are `build-a` and `build-b` (the binary and the toolchain record), `release-files` (native tarball, npm archive, SBOM and `SHA256SUMS`) and `homebrew-formula` (`agentdust.rb`). They are kept for 3 days. The native tarball is `agentdust-X.Y.Z-darwin-arm64.tar.gz` and the npm archive is `agentdust-X.Y.Z-npm.tgz`. Both have sorted entries, a fixed time and owner, and a gzip header without name or time.

The release workflow starts on a pushed tag that matches `v[0-9]+.[0-9]+.[0-9]+`. Every action is pinned to a full commit SHA, the default token can only read, and the jobs that write have their own permissions: `package` gets `id-token` and `attestations`, `release` gets `contents`. The recovery workflow starts only by manual dispatch from `main`; it uses `HOMEBREW_TAP_TOKEN` only for the tap branch and pull request.

## npm publication

[`npm_package.py`](../scripts/npm_package.py) generates `package/package.json` with the workspace and binary version. The package supports `darwin` and `arm64`, exposes the native executable through `bin`, and has no dependencies or lifecycle scripts. The release and dry-run workflows compare npm archives made from both independent binaries. CI and release packaging run [`npm_smoke.py`](../scripts/npm_smoke.py) against a temporary global npm prefix with spaces in its path. The check upgrades a package fixture, preserves Claude and Codex registrations, starts and restarts a worker with no authorized projects, and removes the installation. It uses stub agent CLIs and writes no user configuration.

[`npm-publish.yml`](../.github/workflows/npm-publish.yml) runs after a stable GitHub release is published, or by manual dispatch for recovery. Its `publish` job uses the `npm` environment and OIDC on a GitHub-hosted runner. It downloads the public release's npm archive, native archive and checksum list. [`npm_publish.py`](../scripts/npm_publish.py) checks the release state, archive digests, manifest, executable permission and equality of both embedded binaries. The workflow verifies the npm archive's attestation against `release.yml` and the exact tag before publishing the archived bytes. A retry skips an npm version only when its SHA-512 integrity matches the release archive. Different bytes or registry errors fail the job. Published versions remain immutable.

Configure the npm account once:

1. Sign in with `npm login --registry=https://registry.npmjs.org/`. Passwords, login links and 2FA belong in the maintainer's terminal or browser.
2. After the first GitHub release containing an npm archive is public, download and verify its artifacts using the commands below. Run `npm stage publish ./agentdust-X.Y.Z-npm.tgz --access=public --ignore-scripts`. npm creates a `0.0.0-stage` placeholder for a new package. This reserves the package without publishing the release version.
3. In the package settings, add a GitHub Actions trusted publisher: user `hamzahamidi`, repository `agentdust`, workflow filename `npm-publish.yml`, environment `npm`. Allow direct `npm publish`. Complete its first successful publication within 2 days of creating the trust configuration.
4. Dispatch `gh workflow run npm-publish.yml --ref main -f tag=vX.Y.Z`. The workflow publishes the verified release version through OIDC and supplies npm provenance. Reject the staged bootstrap candidate with `npm stage reject STAGE_ID` after the workflow succeeds. No npm token is stored in GitHub.

For later releases, publishing the stable GitHub draft starts npm publication. If release publication uses a GitHub Actions token, GitHub does not trigger another workflow from that event; dispatch `npm-publish.yml` manually instead. Recovery uses the same dispatch command and never recreates a release tag or replaces assets. Releases without an npm archive cannot be published through this workflow.

The setup, hook and worker paths use the global npm prefix's `bin/agentdust` when it resolves to the installed binary. Keep that prefix stable for upgrades. A Node version manager can change it: pause cleanup, remove each connected agent's setup with the old binary, then install and register the new prefix and resume cleanup. Persistent setup through `npx` or a project-local package is unsupported. Homebrew and npm installations should not share active registrations.

npm setup follows the [trusted publishing](https://docs.npmjs.com/trusted-publishers/) and [staged publishing](https://docs.npmjs.com/staged-publishing/) contracts.

## Cut a release

Once, before the first release:

1. Confirm that the tap repository `hamzahamidi/homebrew-agentdust` has a `Formula` directory.
2. Store a fine-grained token with write access to that repository's contents and pull requests as `HOMEBREW_TAP_TOKEN` in the `release` environment. Without it, `tap-pr` does nothing.
3. Check in the repository settings that `main` requires the `linux` and `macos` checks and allows squash merges only. The workflow checks that `main` contains the commit. It does not read branch protection.
4. Rehearse in a fork (last part of this section).

For each release:

1. Merge a pull request that sets `version` under `[workspace.package]` in `Cargo.toml` and the matching `Cargo.lock`. `cargo update --workspace` changes only the versions of the workspace crates in the lock file.
2. Refresh the toolchain record. Run `gh workflow run release-dry-run.yml --ref main` and wait for it. Download the `build-a` artifact, which holds `toolchain.json` as recorded on that runner, whether the check passed or not. Copy it to `release/toolchain.json`, read the diff (the runner image, Xcode, SDK and Rust should change only on purpose) and merge it. The `cargo_lock_sha256` in it must be the hash of the `Cargo.lock` that will be tagged.
3. Run CI on the commit to tag: `gh workflow run ci.yml --ref main`, then wait for the `linux` and `macos` jobs (`gh run watch`). CI runs on pull requests and not on pushes to `main`, so a squash merge commit has no check run until this step, and the gate refuses a commit without one.
4. Tag that commit and push the tag: `git tag -a vX.Y.Z -m "agentdust X.Y.Z" <commit>`, then `git push origin vX.Y.Z`.
5. Watch the run with `gh run watch`. A refusal by `verify` names the reason on the first failing step.
6. Check the draft release as described in the next section. For the 1.0 candidate, publish it as a prerelease with `gh release edit vX.Y.Z --draft=false --prerelease`. For a release that has passed its acceptance checks, publish it with `gh release edit vX.Y.Z --draft=false`.
7. Merge the tap pull request after the release is published, because the formula URL only works for a published release. For the 1.0 candidate, this makes the prerelease available to Homebrew users during acceptance. Treat it as a public prerelease, not a stable release. If `tap-pr` fails after publication, use the recovery workflow below. Without `HOMEBREW_TAP_TOKEN`, the Action does not open a pull request; restore the token permission, then run recovery. Do not create the formula branch by hand.
8. For 1.0, install the prerelease candidate from Homebrew on the maintainer's Mac. Confirm `command -v agentdust` resolves to the formula install and `agentdust version` matches the `v1.0.0` tag. Record the archive checksum and formula `sha256`; do not change the tag, release assets or formula between this check and stable promotion. Run `agentdust setup`, review its printed diff, approve it, then run `agentdust setup --check`. Use a harness-created stale helper for the apply check: declining sends no signal, and approval sends SIGTERM only to that helper after fresh identity checks. Cross-user rejection remains covered by the automated apply tests. Promote the release to stable with `gh release edit v1.0.0 --prerelease=false` only after this passes. No separate macOS account or multiweek dogfood run is required. The automated smoke uses a fake Claude CLI and a test client, so it does not replace this human approval check.

If the release is already public but `tap-pr` failed, do not recreate the tag. Start the recovery workflow with the published version, without `v`:

```bash
gh workflow run homebrew-tap-recovery.yml --ref main -f version=0.1.0
gh run watch
```

The workflow checks the tarball and SBOM checksums and verifies the tarball attestation before it updates the tap. Merge the generated pull request after confirming the release is public.

`scripts/tap_update.py` writes the new `Formula/agentdust.rb`. When the minor version changes it first saves the old formula as `Formula/agentdust@MAJOR.MINOR.rb` with the class name Homebrew expects (`AgentdustAT01` for 0.1). It refuses a version older than the one in the tap, the same version with another digest, and a versioned formula that already exists with other content.

If a run fails before `release` creates a draft, fix the failing step and rerun the failed jobs when their inputs remain valid. If only `tap-pr` failed after the release was published, use the recovery workflow above. Do not delete or recreate a published release tag to recover the tap pull request.

Rehearsal in a fork: fork the repository, push `main`, run `gh workflow run ci.yml --ref main --repo <fork>` and wait for it, then push a tag equal to the crate version on that commit to the fork. The run builds, attests and creates a draft release in the fork, and `tap-pr` stays off because the fork has no `HOMEBREW_TAP_TOKEN`. To rehearse a drift refusal, change one value in `release/toolchain.json` in the fork, such as `xcode`, and push a new tag. Delete the drafts and tags afterwards.

## Verify a release

Download both archives, the SBOM and the checksum list, check all digests, then check the attestations of both archives, the SBOM and the native binary:

```bash
gh release download vX.Y.Z --repo hamzahamidi/agentdust --pattern 'agentdust-*.tar.gz' --pattern 'agentdust-*-npm.tgz' --pattern 'agentdust-*-sbom.cdx.json' --pattern SHA256SUMS
shasum -a 256 -c SHA256SUMS
gh attestation verify agentdust-X.Y.Z-npm.tgz --repo hamzahamidi/agentdust --signer-workflow hamzahamidi/agentdust/.github/workflows/release.yml --source-ref refs/tags/vX.Y.Z
gh attestation verify agentdust-X.Y.Z-darwin-arm64.tar.gz --repo hamzahamidi/agentdust --signer-workflow hamzahamidi/agentdust/.github/workflows/release.yml
gh attestation verify agentdust-X.Y.Z-sbom.cdx.json --repo hamzahamidi/agentdust --signer-workflow hamzahamidi/agentdust/.github/workflows/release.yml
tar -xzf agentdust-X.Y.Z-darwin-arm64.tar.gz
gh attestation verify agentdust-X.Y.Z-darwin-arm64/agentdust --repo hamzahamidi/agentdust --signer-workflow hamzahamidi/agentdust/.github/workflows/release.yml
```

An attestation says which workflow built a file and from which commit. It says nothing about whether the source is trustworthy. Homebrew does not verify attestations: the formula pins the SHA-256 of the tarball, which protects the download, and whoever can change the tap can change the URL and the digest together. Verification with `gh` is the check against that.

## Roll back

When a release must be withdrawn, two actions work at once on a user's machine.

1. Stop all signalling. Set `apply = false` in `config.toml` in the data directory, which is `~/Library/Application Support/agentdust`:

   ```bash
   umask 077
   printf 'apply = false\n' >> "$HOME/Library/Application Support/agentdust/config.toml"
   ```

   While it is set, manual and automatic cleanup refuse every item. `agentdust doctor` keeps working. A `config.toml` that exists and cannot be read or parsed has the same effect. Delete the line to allow apply again.
2. Install the previous formula. The tap keeps one versioned formula for each earlier minor release: `brew uninstall agentdust`, then `brew install hamzahamidi/agentdust/agentdust@0.1`, with the previous minor in place of 0.1.

Before disconnecting Claude Code, run `agentdust auto disable` in your foreground terminal to clear automatic permissions and remove the worker. `agentdust setup --remove` disconnects Claude Code (the six hooks and the MCP server) and leaves the data directory in place. [SECURITY.md](../SECURITY.md) gives users the same two actions.

For the maintainer: revert the tap commit that added the withdrawn release, which restores the previous `Formula/agentdust.rb` and removes the versioned file it created. Edit the GitHub release notes to say the release is withdrawn and why, and cut a fixed release under the next version number.

## For the integrator

These are the points this page and the workflow cannot settle by themselves. Each one is true of the branch where this page was written.

- `version` under `[workspace.package]` is 1.5.0. The gate compares the tag with the version in `Cargo.toml`.
- `release/toolchain.json` must match the `Cargo.lock` and the runner used by the release. Refresh it after the final lockfile change with the dry-run workflow.
- The README describes 1.5.0 and gives the Homebrew install command for Claude Code and Codex on Apple silicon, with Codex cleanup gated on exact host exit. The [readiness record](v1/readiness.md) covers the exact Homebrew binary and controlled human acceptance.
- `agentdust doctor`, `agentdust apply` and the MCP tools `agentdust_doctor`, `agentdust_plan` and `agentdust_apply` are implemented. M7 adds `agentdust disk [--json]`, `agentdust_disk` and `/agentdust:disk` with [scope and limits](m7/disk.md). The doctor and non-TTY apply refusal have local macOS smoke coverage. The typed approval form was exercised in Claude Code 2.1.289 and Codex CLI 0.156.1 ([client matrix](m0/client-matrix.md)). The release docs test compares the documented setup flags with the command parser.
- The typed-code form in Cursor remains untested because Cursor is not installed on the test machine ([client matrix](m0/client-matrix.md)).
- The rollback relies on `apply = false` (S21). The reader is `agentdust_core::config::apply_switch`. Confirm that the MCP and the terminal apply both call it and that the S21 tests exist.
- The v1.0.0 release is public and stable. The formula in `hamzahamidi/homebrew-agentdust` points to the published Apple silicon archive and is merged on the tap's `main` branch.
- `HOMEBREW_TAP_TOKEN` in the `release` environment needs write access to the tap repository's contents and pull requests. The release workflow cannot grant those permissions or renew an expired token.
- The workflow produces a CycloneDX SBOM from the workspace, includes its checksum and attests it with the binary and tarball.
- The [1.0.0 release run](https://github.com/hamzahamidi/agentdust/actions/runs/37658369862) provides the attested arm64 archive and SBOM. Their checksums and the archive, SBOM and unpacked binary attestations passed locally. The installed Homebrew binary matches the release binary. Setup, live Claude Code discovery, idle measurement, human decline and one-helper approved apply passed on the maintainer's Mac ([acceptance record](v1/acceptance-1.0.0.json)).
