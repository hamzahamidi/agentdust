# Security policy

## Reporting a vulnerability

Report it privately through GitHub: open the repository's Security tab and choose "Report a vulnerability". Please do not open a public issue for a security problem.

Include the AgentDust version (`agentdust version`), the macOS version, and the steps that reproduce the problem.

## Supported versions

AgentDust is before 1.0. Security fixes go into the latest 0.x release and into `main`.

| Version | Supported |
| --- | --- |
| The latest 0.x release | Yes |
| An earlier 0.x release | No. Upgrade to the latest |
| `main` | Reports are welcome, and fixes land here first |

## When a release is withdrawn

A release is withdrawn when it can signal a process that the rules say it must never signal, or when its artifacts are in doubt. Until the replacement is out, either of these works on your machine:

1. Set `apply = false` in `config.toml` in the data directory (`~/Library/Application Support/agentdust`). `agentdust apply` and the `agentdust_apply` tool then refuse every item, and `agentdust doctor` still works.
2. Install the previous formula from the tap: `brew uninstall agentdust`, then `brew install hamzahamidi/agentdust/agentdust@0.1`, with the previous minor release in place of 0.1.

The steps, and how to check a release with `gh attestation verify`, are in [docs/release.md](docs/release.md#roll-back).

## Threat model

The adversaries AgentDust is designed against, the controls for each and the risk that remains are in the [threat model](docs/threat-model.md).

## What counts as a vulnerability

- AgentDust signals a process that its rules say it must never signal.
- Approval can be completed without the typed code.
- Data that the design says is never stored (commands, command output, process environments) reaches disk or a model.
