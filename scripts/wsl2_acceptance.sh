#!/usr/bin/env bash
set -euo pipefail

repo=/home/agentdust/work/agentdust
home=/home/agentdust
target=/home/agentdust/target
export HOME="$home"
export CARGO_TARGET_DIR="$target"
export TMPDIR="$home/tmp"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
export PATH="$HOME/.cargo/bin:$PATH"
export AGENTDUST_EXT4_TEST_ROOT="$home/tmp"

grep -qi 'microsoft-standard-WSL2' /proc/sys/kernel/osrelease
[[ $(uname -m) == x86_64 ]]
[[ $(findmnt -n -o FSTYPE -T "$HOME") == ext4 ]]
[[ $(findmnt -n -o FSTYPE -T /mnt/c) != ext4 ]]

if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.99.0
    export PATH="$HOME/.cargo/bin:$PATH"
fi

cd "$repo"
python3 - <<'PY'
from pathlib import Path
import subprocess

tracked_seeds = subprocess.check_output(
    ["git", "ls-tree", "-r", "--name-only", "HEAD", "--", "fuzz/seeds"], text=True
).splitlines()
for relative in tracked_seeds:
    path = Path(relative)
    if not path.is_file():
        raise SystemExit(f"tracked fuzz seed is missing from the working tree: {path}")
    expected = subprocess.check_output(
        ["git", "rev-parse", f"HEAD:{path.as_posix()}"], text=True
    ).strip()
    actual = subprocess.check_output(
        ["git", "hash-object", "--no-filters", str(path)], text=True
    ).strip()
    if actual != expected:
        raise SystemExit(f"fuzz seed bytes differ from Git: {path}")
PY
cargo test --workspace --exclude agentdust --locked
cargo test --locked -p agentdust --bin agentdust systemd_tests
cargo test --locked -p agentdust --test linux_cli
cargo build --locked -p agentdust -p agentdust-testkit --bins
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
release_dist=${AGENTDUST_RELEASE_DIST:-}
if [[ -n "$release_dist" ]]; then
    mkdir -p "$TMPDIR/release-dist"
    cp "$release_dist/agentdust-$version-linux-x64.tar.gz" "$TMPDIR/release-dist/"
    cp "$release_dist/agentdust-$version.tgz" "$TMPDIR/release-dist/"
    cp "$release_dist/SHA256SUMS" "$TMPDIR/release-dist/"
    (cd "$TMPDIR/release-dist" && sha256sum --check --ignore-missing SHA256SUMS)
else
    cargo build --release --locked -p agentdust
    python3 scripts/package.py --binary "$target/release/agentdust" --version "$version" --target linux-x64 --out-dir "$TMPDIR/release-dist"
fi
mkdir -p "$TMPDIR/release-install"
tar -xzf "$TMPDIR/release-dist/agentdust-$version-linux-x64.tar.gz" -C "$TMPDIR/release-install"
release_binary="$TMPDIR/release-install/agentdust-$version-linux-x64/agentdust"
[[ $("$release_binary" version) == "agentdust $version" ]]
if [[ -z "$release_dist" ]]; then
    cmp "$target/release/agentdust" "$release_binary"
fi

if AGENTDUST_DATA_DIR=/mnt/c/agentdust-wsl2-ci-data "$release_binary" setup --check > "$TMPDIR/drvfs-check.out" 2> "$TMPDIR/drvfs-check.err"; then
    echo 'AgentDust accepted a data directory on /mnt/c' >&2
    exit 1
fi
grep -q 'requires writable ext4' "$TMPDIR/drvfs-check.err"
[[ ! -e /mnt/c/agentdust-wsl2-ci-data ]]

cargo test --locked -p agentdust-core --test linux_journal_ext4 -- --ignored ext4_journal_append_rotation_and_torn_tail_recovery
cargo test --locked -p agentdust --test linux_setup_ext4 -- --ignored claude_codex_setup_hooks_and_mcp_work_on_ext4
cargo test --locked -p agentdust --test linux_port

systemd-run --user --scope env TMPDIR="$TMPDIR" AGENTDUST_EXT4_TEST_ROOT="$AGENTDUST_EXT4_TEST_ROOT" AGENTDUST_REQUIRE_SYSTEMD_PIDFD=1 cargo test --locked -p agentdust --test linux_pidfd
systemd-run --user --scope env TMPDIR="$TMPDIR" AGENTDUST_EXT4_TEST_ROOT="$AGENTDUST_EXT4_TEST_ROOT" AGENTDUST_FIXTURE_SLEEPER="$target/debug/fixture-sleeper" cargo test --locked -p agentdust --test linux_automatic -- --ignored --test-threads=1
systemd-run --user --scope env TMPDIR="$TMPDIR" AGENTDUST_EXT4_TEST_ROOT="$AGENTDUST_EXT4_TEST_ROOT" AGENTDUST_FIXTURE_LISTENER="$target/debug/fixture-listener" cargo test --locked -p agentdust --test linux_port -- --ignored --test-threads=1

worker_data="$home/worker-data"
worker_project="$home/worker-project"
wait_for_worker() {
    local attempt
    for attempt in {1..50}; do
        if AGENTDUST_DATA_DIR="$worker_data" "$release_binary" auto status | python3 -c 'import json,sys; assert json.load(sys.stdin)["worker_running"]'; then
            return 0
        fi
        sleep 0.1
    done
    echo 'automatic worker did not report ready' >&2
    AGENTDUST_DATA_DIR="$worker_data" "$release_binary" auto status >&2
    return 1
}
install -d -m 700 "$worker_data" "$worker_project"
AGENTDUST_DATA_DIR="$worker_data" "$release_binary" auto enable --yes "$worker_project"
systemctl --user is-active --quiet agentdust-automatic.service
wait_for_worker
systemctl --user restart agentdust-automatic.service
systemctl --user is-active --quiet agentdust-automatic.service
wait_for_worker
systemctl --user disable --now agentdust-automatic.service
rm "$HOME/.config/systemd/user/agentdust-automatic.service"
systemctl --user daemon-reload

mkdir -p "$TMPDIR/npm-install"
if [[ -n "$release_dist" ]]; then
    npm install --global --prefix "$TMPDIR/npm-install" "$TMPDIR/release-dist/agentdust-$version.tgz" --ignore-scripts --no-audit --no-fund
else
    mkdir -p "$TMPDIR/npm-build/bin" "$TMPDIR/npm-build/vendor/linux-x64" "$TMPDIR/npm-dist"
    cp npm/package.json LICENSE README.md "$TMPDIR/npm-build/"
    install -m 755 npm/bin/agentdust.js "$TMPDIR/npm-build/bin/agentdust.js"
    install -m 755 "$target/release/agentdust" "$TMPDIR/npm-build/vendor/linux-x64/agentdust"
    npm --prefix "$TMPDIR/npm-build" version "$version" --no-git-tag-version --ignore-scripts --allow-same-version
    npm pack "$TMPDIR/npm-build" --pack-destination "$TMPDIR/npm-dist" --ignore-scripts
    npm install --global --prefix "$TMPDIR/npm-install" "$TMPDIR/npm-dist/agentdust-$version.tgz" --ignore-scripts --no-audit --no-fund
fi
[[ $("$TMPDIR/npm-install/bin/agentdust" version) == "agentdust $version" ]]
cmp "$release_binary" "$TMPDIR/npm-install/lib/node_modules/agentdust/vendor/linux-x64/agentdust"
if [[ -z "$release_dist" ]]; then
    cmp "$target/release/agentdust" "$TMPDIR/npm-build/vendor/linux-x64/agentdust"
fi

host_pid=$(powershell.exe -NoProfile -NonInteractive -Command '$p = Start-Process -FilePath "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" -ArgumentList @("-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 900") -PassThru; $p.Id' | tr -d '\r')
case "$host_pid" in
    ''|0|*[!0-9]*) echo 'WSL interop did not return a Windows host PID' >&2; exit 1 ;;
    [1-9]*) ;;
    *) echo 'WSL interop returned an unexpected Windows host PID' >&2; exit 1 ;;
esac
assert_host_alive() {
    powershell.exe -NoProfile -NonInteractive -Command "if (-not (Get-Process -Id $host_pid -ErrorAction SilentlyContinue)) { exit 1 }" >/dev/null
}
cleanup_host() {
    powershell.exe -NoProfile -NonInteractive -Command "Stop-Process -Id $host_pid -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1 || true
}
trap cleanup_host EXIT
assert_host_alive
windows_process_seen=0
for proc_exe in /proc/[0-9]*/exe; do
    exe=$(readlink "$proc_exe" 2>/dev/null || true)
    case "${exe,,}" in
        *powershell.exe|*pwsh.exe) windows_process_seen=1 ;;
        *) : ;;
    esac
done
[[ $windows_process_seen == 0 ]]
"$release_binary" doctor --json > "$TMPDIR/windows-host-doctor.json"
python3 - "$TMPDIR/windows-host-doctor.json" <<'PY'
import json
import sys

findings = json.load(open(sys.argv[1], encoding="utf-8"))["findings"]
windows_names = {"powershell.exe", "pwsh.exe"}
assert all((finding.get("exe_base") or "").lower() not in windows_names for finding in findings)
PY
assert_host_alive

printf 'PASS: WSL2 ext4, Claude Code and Codex setup, journal, PID identity, ports, automatic worker, npm package and host process isolation\n'
