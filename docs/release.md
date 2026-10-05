# Releasing AgentDust

A release starts when a tag `vX.Y.Z` is pushed. [`release.yml`](../.github/workflows/release.yml) checks the tag, builds the binary twice on `macos-15` and creates a draft GitHub Release. A person checks and publishes the draft, then merges the pull request that puts the new formula in the Homebrew tap. The tap job reuses its release branch on retry. [`homebrew-tap-recovery.yml`](../.github/workflows/homebrew-tap-recovery.yml) can open the tap pull request for an already published release. This is spec 8.4 and S20.

## What the workflow does

| Job | Runs on | What it does |
| --- | --- | --- |
| `verify` | `ubuntu-24.04` | Refuses a tag that is not `vMAJOR.MINOR.PATCH`, a tag that differs from `version` under `[workspace.package]`, a commit that `origin/main` does not contain, and a commit without a passing `linux` and `macos` check run from GitHub Actions. A commit with no check run at all is refused. The decisions are in `scripts/check_release.py` |
| `audit` | `ubuntu-24.04` | `cargo audit` against `Cargo.lock` |
| `build` | `macos-15`, two jobs (`copy` a and b) | `scripts/toolchain.py check --include-lock` against `release/toolchain.json`, then `cargo build --release --locked -p agentdust` with the `DEVELOPER_DIR` and `SDKROOT` of the record. Any drift in the runner image, Rust, Cargo, Xcode, SDK, flags or the `Cargo.lock` hash stops the build |
| `package` | `macos-15` | Compares the two binaries, checks that the binary prints `agentdust X.Y.Z`, builds the tarball from each binary and compares the tarballs, generates a CycloneDX SBOM from the workspace, writes `SHA256SUMS` for the tarball and SBOM, attests the binary, tarball and SBOM, writes the formula for the release download URL, and installs the tarball through a local tap on the runner |
| `release` | `ubuntu-24.04` | Creates a draft release for the existing tag with the tarball and `SHA256SUMS`. It has no checkout of the source and the only write permission in the workflow besides the attestation |
| `tap-pr` | `ubuntu-24.04` | Uses the `release` environment secret `HOMEBREW_TAP_TOKEN` to open a pull request in the tap with the new formula. A retry reuses the existing branch and skips a duplicate commit or pull request. Without the secret the job prints a notice and does nothing |
| Homebrew tap recovery | `ubuntu-24.04` | Manually dispatched for a published version. Downloads and verifies its release assets, regenerates the formula, then reuses the version branch or opens its pull request |

The artifacts of a run are `build-a` and `build-b` (the binary and the toolchain record), `release-files` (tarball, SBOM and `SHA256SUMS`) and `homebrew-formula` (`agentdust.rb`). They are kept for 3 days. The tarball is `agentdust-X.Y.Z-darwin-arm64.tar.gz` with sorted entries, a fixed time and owner, and a gzip header without name or time.

The release workflow starts on a pushed tag that matches `v[0-9]+.[0-9]+.[0-9]+`. Every action is pinned to a full commit SHA, the default token can only read, and the jobs that write have their own permissions: `package` gets `id-token` and `attestations`, `release` gets `contents`. The recovery workflow starts only by manual dispatch from `main`; it uses `HOMEBREW_TAP_TOKEN` only for the tap branch and pull request.

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
6. Check the draft release as described in the next section. Publish it with `gh release edit vX.Y.Z --draft=false`.
7. Merge the tap pull request after the release is published, because the formula URL only works for a published release. If `tap-pr` fails after publication, use the recovery workflow below. Without `HOMEBREW_TAP_TOKEN`, the Action does not open a pull request; restore the token permission, then run recovery. Do not create the formula branch by hand.
8. On a clean account run `brew install hamzahamidi/agentdust/agentdust`, `agentdust version` and `agentdust setup`.

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

Download the tarball, the SBOM and the checksum list, check both digests, then check the attestations of the tarball, the SBOM and the binary inside it:

```bash
gh release download vX.Y.Z --repo hamzahamidi/agentdust --pattern 'agentdust-*.tar.gz' --pattern 'agentdust-*-sbom.cdx.json' --pattern SHA256SUMS
shasum -a 256 -c SHA256SUMS
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

   While it is set, `agentdust apply` and the `agentdust_apply` tool refuse every item. `agentdust doctor` keeps working. A `config.toml` that exists and cannot be read or parsed has the same effect. Delete the line to allow apply again.
2. Install the previous formula. The tap keeps one versioned formula for each earlier minor release: `brew uninstall agentdust`, then `brew install hamzahamidi/agentdust/agentdust@0.1`, with the previous minor in place of 0.1.

`agentdust setup --remove` disconnects Claude Code (the four hooks and the MCP server) and leaves the data directory in place. [SECURITY.md](../SECURITY.md) gives users the same two actions.

For the maintainer: revert the tap commit that added the withdrawn release, which restores the previous `Formula/agentdust.rb` and removes the versioned file it created. Edit the GitHub release notes to say the release is withdrawn and why, and cut a fixed release under the next version number.

## For the integrator

These are the points this page and the workflow cannot settle by themselves. Each one is true of the branch where this page was written.

- `version` under `[workspace.package]` is 0.1.0. The gate compares the tag with the version in `Cargo.toml`.
- `release/toolchain.json` must match the `Cargo.lock` and the runner used by the release. Refresh it after the final lockfile change with the dry-run workflow.
- The README describes 0.1.0 and gives the Homebrew install command for Claude Code on Apple silicon.
- `agentdust doctor`, `agentdust apply` and the MCP tools `agentdust_doctor`, `agentdust_plan` and `agentdust_apply` are implemented. The doctor and non-TTY apply refusal have local macOS smoke coverage. The typed approval form was exercised in Claude Code 2.1.289 and Codex CLI 0.156.1 ([client matrix](m0/client-matrix.md)). The release docs test compares the documented setup flags with the command parser.
- The typed-code form in Cursor remains untested because Cursor is not installed on the test machine ([client matrix](m0/client-matrix.md)).
- The rollback relies on `apply = false` (S21). The reader is `agentdust_core::config::apply_switch`. Confirm that the MCP and the terminal apply both call it and that the S21 tests exist.
- The v0.1.0 release is public. The formula in `hamzahamidi/homebrew-agentdust` points to the published Apple silicon archive and is merged on the tap's `main` branch.
- The workflow produces a CycloneDX SBOM from the workspace, includes its checksum and attests it with the binary and tarball.
- The [v0.1.0 recovery run](https://github.com/hamzahamidi/agentdust/actions/runs/37307055565) used `HOMEBREW_TAP_TOKEN` from the `release` environment to open the tap pull request, then verified the tarball and SBOM checksums and attestations against `refs/tags/v0.1.0`. A clean-account Homebrew installation and verification of the installed binary's attestation remain unverified.
