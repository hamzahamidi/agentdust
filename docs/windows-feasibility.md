# Native Windows feasibility

AgentDust does not support native Windows. WSL2 runs the Linux provider inside one Ubuntu distribution and does not inspect Windows host processes.

## Shutdown constraint

Windows exposes process creation time through `GetProcessTimes`, and `RM_UNIQUE_PROCESS` identifies a process with its PID and creation time. `GetExtendedTcpTable` can report TCP listener owner PIDs. These APIs can support identity checks and port diagnosis.

The current cleanup contract needs a fresh identity check followed by a graceful request to exactly one existing helper. `GenerateConsoleCtrlEvent` sends control events to console processes. `CTRL_C_EVENT` cannot be limited to a process group. `CTRL_BREAK_EVENT` targets a process group created at process launch and can reach its descendants. `TerminateProcess` ends a process without running its termination handlers, so it is not an acceptable fallback.

Restart Manager can register one `RM_UNIQUE_PROCESS` and request shutdown without the force flag. Microsoft documents console applications handling `CTRL_C_EVENT` for that shutdown path. A controlled Windows runner probe checks whether Restart Manager confines the request to the registered helper when a parent, sibling and descendant share its console.

## Controlled probe

The branch-scoped [M12 Windows job](../.github/workflows/ci.yml) runs isolated synthetic console helpers. It registers one process by PID and creation time, checks the Restart Manager process list, calls `RmShutdown` without force only when the list contains that exact identity, and records `CTRL_C_EVENT` delivery, surfaced as Python `SIGINT`, and exit states for the target, parent, sibling and descendant. It also checks an unresponsive target and a stale creation time paired with a live PID. The stale check verifies AgentDust's pre-action guard, not Restart Manager's own stale identity handling. Forceful termination is limited to cleaning up the probe's own helpers, including when a helper fails during startup.

The experiment does not use Claude Code or Codex, and it does not prove agent lifecycle attribution or an AgentDust provider. The job reports `follow_up_needed` only when every scope check passes; a completed `no_go` is also a successful experiment. `ERROR_FAIL_SHUTDOWN` is an expected refusal for a target that does not exit. Other Restart Manager errors, runtime failures or setup errors fail the job. A passing result only permits separate feasibility work. Native Windows remains unsupported until the exact target can be stopped gracefully without affecting any other process.

## Microsoft references

1. [Console process groups](https://learn.microsoft.com/en-us/windows/console/console-process-groups)
2. [GenerateConsoleCtrlEvent](https://learn.microsoft.com/en-us/windows/console/generateconsolectrlevent)
3. [Restart Manager and RM_UNIQUE_PROCESS](https://learn.microsoft.com/en-us/windows/win32/api/restartmanager/ns-restartmanager-rm_unique_process)
4. [RmShutdown](https://learn.microsoft.com/en-us/windows/win32/api/restartmanager/nf-restartmanager-rmshutdown)
5. [Restart Manager application guidelines](https://learn.microsoft.com/en-us/windows/win32/rstmgr/guidelines-for-applications)
6. [GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)
7. [GetExtendedTcpTable](https://learn.microsoft.com/en-us/windows/win32/api/iphlpapi/nf-iphlpapi-getextendedtcptable)
8. [Terminating a process](https://learn.microsoft.com/en-us/windows/desktop/ProcThread/terminating-a-process)
