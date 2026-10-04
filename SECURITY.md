# Security policy

## Reporting a vulnerability

Report it privately through GitHub: open the repository's Security tab and choose "Report a vulnerability". Please do not open a public issue for a security problem.

Include the AgentDust version (`agentdust version`), the macOS version, and the steps that reproduce the problem.

## Supported versions

Nothing is released yet. Reports against the `main` branch are welcome.

## Threat model

The adversaries AgentDust is designed against, the controls for each and the risk that remains are in the [threat model](docs/threat-model.md).

## What counts as a vulnerability

- AgentDust signals a process that its rules say it must never signal.
- Approval can be completed without the typed code.
- Data that the design says is never stored (commands, command output, process environments) reaches disk or a model.
