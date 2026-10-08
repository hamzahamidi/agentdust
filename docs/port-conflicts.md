# TCP port conflicts

AgentDust 1.4.0 diagnoses IPv4/IPv6 TCP listeners on one local port and can recover proven leftovers under the existing user-enabled directory policy.

```bash
agentdust port 3000
agentdust port 3000 --resolve --json
```

The MCP tool is `agentdust_port` with `{ "port": 3000 }` for diagnosis, or `{ "port": 3000, "resolve": true }` for recovery. Diagnosis is the default. The port must be an integer from 1 through 65535. Recovery does not enable or broaden the directory policy.

## Agent workflow

1. Diagnose the requested port and explain each listener's class and evidence.
2. When recovery is requested, use `resolve: true`. Only a proven ended-session helper in an enabled project can receive automatic SIGTERM. Keeps, pause, exact process identity, all-owner liveness and existing attempt receipts still apply.
3. A suspect with `approval_required` needs the existing `agentdust_plan` and `agentdust_apply` typed approval flow. Select only the exact requested listener item from that fresh plan. Live, managed, unknown and likely-owned listeners cannot be cleaned through this feature.
4. Read the final scan. Retry the requested server start after `no_visible_listener`. Report a survivor or prior attempt without escalating or retrying a signal.

When a server start initiated for the user fails with `EADDRINUSE`, use the port reported by that failure or supplied by the user. If it is ambiguous, report that instead of guessing. The agent can call `agentdust_port` with `resolve: true` for the port. Recovery uses only the existing directory policy. If a listener needs approval, the user must enter the code through the typed approval flow. After any apply attempt, scan the port again without `resolve`. Retry that same server start once only after a fresh `no_visible_listener` result, then report whether it started. Do not repeat cleanup if the retry still fails. A remaining listener or unavailable inventory is reported as the blocker.

## Report

The JSON report has version `1`, the requested `port`, protocol `tcp`, visibility `current_user_visible`, initial listener results and `remaining_pids` from a final scan. Each listener includes its PID, optional sanitized process finding, result and reason. Raw commands, full paths, environments and listener addresses are not returned. `gone_or_no_longer_listening` means the executor could no longer find that exact listener; it does not claim the process terminated.

| State | Meaning |
| --- | --- |
| `listening` | At least one listener was observed in the final scan |
| `no_visible_listener` | No listener was observed within current-user visibility at that moment |
| `unavailable` | Inventory, classification or final verification failed; read `error` |

An empty visible scan is not a bind guarantee. Another user may hold a socket that this account cannot inspect. A process can start listening after the scan, and non-listening TCP sockets can also affect binding. UDP, cloud execution and remote hosts are outside this feature.

## Execution boundary

The macOS listener source uses `/usr/sbin/lsof` with TCP LISTEN and numeric output. It reads only PID and socket-name fields, bounds output and time, rejects diagnostics and malformed records, and compares exact kernel identity and executable path before and after the scan. At most 16 distinct listeners are accepted.

The normal full process survey and ownership classifier run before listener filtering. Before each automatic attempt, the existing executor takes another full survey through that wrapper and requires the same exact process to remain a listener. A port change can prevent cleanup; it cannot establish that an owner is gone. The executor sends at most one initial SIGTERM per recorded identity through the existing audit and receipt path. Socket membership and signaling are not one kernel transaction.

Controlled tests cover listener parsing and deduplication, malformed/partial inventory, changed identity or listener membership, protected classes, read-only defaults, MCP argument refusal, native TCP listeners, live owners, keep, pause, disabled scope, one cleanup and survivor receipts. These establish controlled behavior, not a claim of field incident recovery or measured time saved.
