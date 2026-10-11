import ctypes
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import uuid

ERROR_MORE_DATA = 234
ERROR_FAIL_SHUTDOWN = 351
STILL_ACTIVE = 259
PROCESS_TERMINATE = 0x0001
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
SYNCHRONIZE = 0x00100000

DWORD = ctypes.c_uint32
UINT = ctypes.c_uint32
BOOL = ctypes.c_int
HANDLE = ctypes.c_void_p
WCHAR = ctypes.c_wchar


class FILETIME(ctypes.Structure):
    _fields_ = [("dwLowDateTime", DWORD), ("dwHighDateTime", DWORD)]


class RM_UNIQUE_PROCESS(ctypes.Structure):
    _fields_ = [("dwProcessId", DWORD), ("ProcessStartTime", FILETIME)]


class RM_PROCESS_INFO(ctypes.Structure):
    _fields_ = [
        ("Process", RM_UNIQUE_PROCESS),
        ("strAppName", WCHAR * 256),
        ("strServiceShortName", WCHAR * 64),
        ("ApplicationType", ctypes.c_int),
        ("AppStatus", DWORD),
        ("TSSessionId", DWORD),
        ("bRestartable", BOOL),
    ]


kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
rstrtmgr = ctypes.WinDLL("rstrtmgr", use_last_error=True)

kernel32.GetCurrentProcess.restype = HANDLE
kernel32.GetProcessTimes.argtypes = [HANDLE, ctypes.POINTER(FILETIME), ctypes.POINTER(FILETIME), ctypes.POINTER(FILETIME), ctypes.POINTER(FILETIME)]
kernel32.GetProcessTimes.restype = BOOL
kernel32.OpenProcess.argtypes = [DWORD, BOOL, DWORD]
kernel32.OpenProcess.restype = HANDLE
kernel32.GetExitCodeProcess.argtypes = [HANDLE, ctypes.POINTER(DWORD)]
kernel32.GetExitCodeProcess.restype = BOOL
kernel32.TerminateProcess.argtypes = [HANDLE, UINT]
kernel32.TerminateProcess.restype = BOOL
kernel32.CloseHandle.argtypes = [HANDLE]
kernel32.CloseHandle.restype = BOOL
kernel32.WaitForSingleObject.argtypes = [HANDLE, DWORD]
kernel32.WaitForSingleObject.restype = DWORD
kernel32.AllocConsole.restype = BOOL
kernel32.FreeConsole.restype = BOOL
kernel32.GetConsoleWindow.restype = HANDLE

rstrtmgr.RmStartSession.argtypes = [ctypes.POINTER(DWORD), DWORD, ctypes.c_wchar_p]
rstrtmgr.RmStartSession.restype = DWORD
rstrtmgr.RmRegisterResources.argtypes = [DWORD, UINT, ctypes.c_void_p, UINT, ctypes.POINTER(RM_UNIQUE_PROCESS), UINT, ctypes.c_void_p]
rstrtmgr.RmRegisterResources.restype = DWORD
rstrtmgr.RmGetList.argtypes = [DWORD, ctypes.POINTER(UINT), ctypes.POINTER(UINT), ctypes.POINTER(RM_PROCESS_INFO), ctypes.POINTER(DWORD)]
rstrtmgr.RmGetList.restype = DWORD
rstrtmgr.RmShutdown.argtypes = [DWORD, UINT, ctypes.c_void_p]
rstrtmgr.RmShutdown.restype = DWORD
rstrtmgr.RmEndSession.argtypes = [DWORD]
rstrtmgr.RmEndSession.restype = DWORD


def filetime_value(value):
    return (int(value.dwHighDateTime) << 32) | int(value.dwLowDateTime)


def process_creation_time(handle):
    created, exited, kernel, user = FILETIME(), FILETIME(), FILETIME(), FILETIME()
    native_handle = handle if isinstance(handle, HANDLE) else HANDLE(handle)
    if not kernel32.GetProcessTimes(native_handle, ctypes.byref(created), ctypes.byref(exited), ctypes.byref(kernel), ctypes.byref(user)):
        raise ctypes.WinError(ctypes.get_last_error())
    return filetime_value(created)


def current_creation_time():
    return process_creation_time(kernel32.GetCurrentProcess())


def write_json(path, value):
    path = Path(path)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value), encoding="utf-8")
    os.replace(temporary, path)


def append_json(path, value):
    with open(path, "a", encoding="utf-8") as output:
        output.write(json.dumps(value) + "\n")


def worker(role, directory, mode, spawn_child):
    directory = Path(directory)
    event_path = directory / f"events-{role}.jsonl"
    def on_control(signum, frame):
        append_json(event_path, {"signal": "CTRL_C_EVENT", "pid": os.getpid(), "time": time.time()})
        if mode == "exit":
            raise SystemExit(0)
    signal.signal(signal.SIGINT, on_control)

    child = None
    try:
        if spawn_child:
            child = subprocess.Popen([
                sys.executable, str(Path(__file__).resolve()), "--worker", "descendant", str(directory), "stay", "0"
            ], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=0)
            wait_ready(directory / "ready-descendant.json", 15)

        write_json(directory / f"ready-{role}.json", {
            "role": role,
            "pid": os.getpid(),
            "created": current_creation_time(),
            "child_pid": child.pid if child else None,
        })
    except BaseException:
        if child is not None:
            child.terminate()
            child.wait(timeout=5)
        raise

    while True:
        time.sleep(0.1)


def wait_ready(path, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if Path(path).exists():
            return json.loads(Path(path).read_text(encoding="utf-8"))
        time.sleep(0.05)
    raise TimeoutError(f"worker did not become ready: {path}")


def launch_worker(directory, role, mode, spawn_child=False):
    process = subprocess.Popen([
        sys.executable, str(Path(__file__).resolve()), "--worker", role, str(directory), mode,
        "1" if spawn_child else "0",
    ], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=0)
    try:
        ready = wait_ready(Path(directory) / f"ready-{role}.json", 15)
    except BaseException:
        process.terminate()
        process.wait(timeout=5)
        raise
    return process, ready


def event_count(directory, role):
    path = Path(directory) / f"events-{role}.jsonl"
    if not path.exists():
        return 0
    return len(path.read_text(encoding="utf-8").splitlines())


def rm_session_for(pid, created):
    session = DWORD()
    key = ctypes.create_unicode_buffer(33)
    code = rstrtmgr.RmStartSession(ctypes.byref(session), 0, key)
    if code != 0:
        raise OSError(f"RmStartSession returned {code}")
    unique = RM_UNIQUE_PROCESS()
    unique.dwProcessId = pid
    unique.ProcessStartTime.dwLowDateTime = created & 0xFFFFFFFF
    unique.ProcessStartTime.dwHighDateTime = (created >> 32) & 0xFFFFFFFF
    code = rstrtmgr.RmRegisterResources(session, 0, None, 1, ctypes.byref(unique), 0, None)
    if code != 0:
        rstrtmgr.RmEndSession(session)
        raise OSError(f"RmRegisterResources returned {code}")
    return session


def rm_list(session):
    needed, count, reboot = UINT(0), UINT(0), DWORD(0)
    code = rstrtmgr.RmGetList(session, ctypes.byref(needed), ctypes.byref(count), None, ctypes.byref(reboot))
    if code not in (0, ERROR_MORE_DATA):
        raise OSError(f"RmGetList sizing call returned {code}")
    if needed.value == 0:
        return []
    count = UINT(needed.value)
    processes = (RM_PROCESS_INFO * needed.value)()
    code = rstrtmgr.RmGetList(session, ctypes.byref(needed), ctypes.byref(count), processes, ctypes.byref(reboot))
    if code != 0:
        raise OSError(f"RmGetList returned {code}")
    return [
        (processes[index].Process.dwProcessId, filetime_value(processes[index].Process.ProcessStartTime))
        for index in range(count.value)
    ]


def rm_shutdown(pid, created):
    identity_state = pid_state(pid, created)
    if identity_state != "alive":
        guard = "identity_mismatch" if identity_state == "pid_reused" else "identity_not_live"
        return [], None, guard
    session = rm_session_for(pid, created)
    try:
        processes = rm_list(session)
        if processes != [(pid, created)]:
            return processes, None, "affected_process_list_mismatch"
        if pid_state(pid, created) != "alive":
            return processes, None, "identity_changed"
        code = int(rstrtmgr.RmShutdown(session, 0, None))
        if code not in (0, ERROR_FAIL_SHUTDOWN):
            raise OSError(f"RmShutdown returned unexpected status {code}")
        return processes, code, "attempted"
    finally:
        rstrtmgr.RmEndSession(session)


def pid_state(pid, expected_created):
    handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, False, pid)
    if not handle:
        return "exited_or_inaccessible"
    try:
        if process_creation_time(handle) != expected_created:
            return "pid_reused"
        code = DWORD()
        if not kernel32.GetExitCodeProcess(handle, ctypes.byref(code)):
            raise ctypes.WinError(ctypes.get_last_error())
        return "alive" if code.value == STILL_ACTIVE else f"exited:{code.value}"
    finally:
        kernel32.CloseHandle(handle)


def terminate_owned(pid, expected_created):
    handle = kernel32.OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, False, pid)
    if not handle:
        return
    try:
        if process_creation_time(handle) == expected_created:
            code = DWORD()
            if kernel32.GetExitCodeProcess(handle, ctypes.byref(code)) and code.value == STILL_ACTIVE:
                kernel32.TerminateProcess(handle, 137)
                kernel32.WaitForSingleObject(handle, 5000)
    finally:
        kernel32.CloseHandle(handle)


def run_probe(directory):
    if not kernel32.FreeConsole():
        pass
    if not kernel32.AllocConsole():
        raise ctypes.WinError(ctypes.get_last_error())
    parent_events = Path(directory) / "events-parent.jsonl"
    def parent_control(signum, frame):
        append_json(parent_events, {"signal": "CTRL_C_EVENT", "pid": os.getpid(), "time": time.time()})
    signal.signal(signal.SIGINT, parent_control)

    owned = []
    summary = {"targeted_shutdown": {}, "unresponsive_shutdown": {}, "stale_identity": {}}
    try:
        target, target_record = launch_worker(directory, "target", "exit", spawn_child=True)
        owned.append((target.pid, target_record["created"]))
        descendant_record = wait_ready(Path(directory) / "ready-descendant.json", 15)
        owned.append((descendant_record["pid"], descendant_record["created"]))
        sibling, sibling_record = launch_worker(directory, "sibling", "stay")
        owned.append((sibling.pid, sibling_record["created"]))

        before = {role: event_count(directory, role) for role in ("parent", "sibling", "descendant")}
        processes, shutdown_code, shutdown_guard = rm_shutdown(target.pid, target_record["created"])
        try:
            target.wait(timeout=12)
            target_exit_code = target.returncode
        except subprocess.TimeoutExpired:
            target_exit_code = None
        time.sleep(0.5)
        after = {role: event_count(directory, role) for role in ("parent", "sibling", "descendant")}
        summary["targeted_shutdown"] = {
            "registered_list": processes,
            "target_pid": target.pid,
            "target_created": target_record["created"],
            "shutdown_attempted": shutdown_guard == "attempted",
            "shutdown_guard": shutdown_guard,
            "shutdown_code": shutdown_code,
            "target_exit_code": target_exit_code,
            "target_ctrl_c_count": event_count(directory, "target"),
            "other_ctrl_c_counts_before": before,
            "other_ctrl_c_counts_after": after,
            "sibling_state": pid_state(sibling.pid, sibling_record["created"]),
            "descendant_state": pid_state(descendant_record["pid"], descendant_record["created"]),
        }

        unresponsive, unresponsive_record = launch_worker(directory, "unresponsive", "stay")
        owned.append((unresponsive.pid, unresponsive_record["created"]))
        before = {role: event_count(directory, role) for role in ("parent", "sibling", "descendant")}
        processes, shutdown_code, shutdown_guard = rm_shutdown(unresponsive.pid, unresponsive_record["created"])
        time.sleep(0.5)
        after = {role: event_count(directory, role) for role in ("parent", "sibling", "descendant")}
        summary["unresponsive_shutdown"] = {
            "registered_list": processes,
            "target_pid": unresponsive.pid,
            "shutdown_attempted": shutdown_guard == "attempted",
            "shutdown_guard": shutdown_guard,
            "shutdown_code": shutdown_code,
            "target_state": pid_state(unresponsive.pid, unresponsive_record["created"]),
            "target_ctrl_c_count": event_count(directory, "unresponsive"),
            "other_ctrl_c_counts_before": before,
            "other_ctrl_c_counts_after": after,
        }

        stale_created = target_record["created"]
        if stale_created == sibling_record["created"]:
            stale_created += 1
        before = {role: event_count(directory, role) for role in ("parent", "sibling", "descendant")}
        stale_list, stale_code, stale_guard = rm_shutdown(sibling.pid, stale_created)
        time.sleep(0.5)
        after = {role: event_count(directory, role) for role in ("parent", "sibling", "descendant")}
        summary["stale_identity"] = {
            "pid": sibling.pid,
            "requested_created": stale_created,
            "live_created": sibling_record["created"],
            "state": pid_state(sibling.pid, sibling_record["created"]),
            "affected_process_list": stale_list,
            "shutdown_attempted": stale_guard == "attempted",
            "shutdown_guard": stale_guard,
            "shutdown_code": stale_code,
            "other_ctrl_c_counts_before": before,
            "other_ctrl_c_counts_after": after,
        }
    finally:
        for pid, created in owned:
            terminate_owned(pid, created)

    target = summary["targeted_shutdown"]
    unresponsive = summary["unresponsive_shutdown"]
    stale = summary["stale_identity"]
    expected_target = (target.get("registered_list") == [(target.get("target_pid"), target.get("target_created"))]
                       and target.get("shutdown_attempted") is True
                       and target.get("shutdown_code") == 0
                       and target.get("target_exit_code") == 0
                       and target.get("target_ctrl_c_count", 0) > 0
                       and target.get("sibling_state") == "alive"
                       and target.get("descendant_state") == "alive"
                       and target.get("other_ctrl_c_counts_before") == target.get("other_ctrl_c_counts_after"))
    expected_unresponsive = (unresponsive.get("registered_list") == [(unresponsive.get("target_pid"), unresponsive_record["created"])]
                             and unresponsive.get("shutdown_attempted") is True
                             and unresponsive.get("target_state") == "alive"
                             and unresponsive.get("target_ctrl_c_count", 0) > 0
                             and unresponsive.get("other_ctrl_c_counts_before") == unresponsive.get("other_ctrl_c_counts_after"))
    expected_stale = (stale.get("state") == "alive"
                       and stale.get("requested_created") != stale.get("live_created")
                       and stale.get("affected_process_list") == []
                       and stale.get("shutdown_attempted") is False
                       and stale.get("shutdown_guard") == "identity_mismatch"
                       and stale.get("other_ctrl_c_counts_before") == stale.get("other_ctrl_c_counts_after"))
    summary["checks"] = {
        "target_only_shutdown": bool(expected_target),
        "unresponsive_target_survives_without_collateral": bool(expected_unresponsive),
        "stale_identity_is_not_listed": bool(expected_stale),
    }
    summary["probe_completed"] = True
    summary["decision"] = "follow_up_needed" if all(summary["checks"].values()) else "no_go"
    print(json.dumps(summary, indent=2, sort_keys=True))


def main():
    if os.name != "nt":
        raise SystemExit("This probe runs only on Windows.")
    if len(sys.argv) >= 2 and sys.argv[1] == "--worker":
        _, _, role, directory, mode, spawn_child = sys.argv
        worker(role, directory, mode, spawn_child == "1")
        return
    with tempfile.TemporaryDirectory(prefix="agentdust-m12-") as directory:
        run_probe(directory)


if __name__ == "__main__":
    main()
