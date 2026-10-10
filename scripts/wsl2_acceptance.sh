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
test "$(uname -m)" = x86_64
test "$(findmnt -n -o FSTYPE -T "$HOME")" = ext4
test "$(findmnt -n -o FSTYPE -T /mnt/c)" != ext4

if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.99.0
    export PATH="$HOME/.cargo/bin:$PATH"
fi

cd "$repo"
cargo test --workspace --locked
cargo build --locked -p agentdust -p agentdust-testkit --bins
cargo build --release --locked -p agentdust
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
python3 scripts/package.py --binary "$target/release/agentdust" --version "$version" --target linux-x64 --out-dir "$TMPDIR/release-dist"
mkdir -p "$TMPDIR/release-install"
tar -xzf "$TMPDIR/release-dist/agentdust-$version-linux-x64.tar.gz" -C "$TMPDIR/release-install"
release_binary="$TMPDIR/release-install/agentdust-$version-linux-x64/agentdust"
test "$("$release_binary" version)" = "agentdust $version"
cmp "$target/release/agentdust" "$release_binary"

if AGENTDUST_DATA_DIR=/mnt/c/agentdust-wsl2-ci-data "$target/release/agentdust" setup --check > "$TMPDIR/drvfs-check.out" 2> "$TMPDIR/drvfs-check.err"; then
    echo 'AgentDust accepted a data directory on /mnt/c' >&2
    exit 1
fi
grep -q 'requires writable ext4' "$TMPDIR/drvfs-check.err"
test ! -e /mnt/c/agentdust-wsl2-ci-data

cargo test --locked -p agentdust-core --test linux_journal_ext4 -- --ignored ext4_journal_append_rotation_and_torn_tail_recovery
cargo test --locked -p agentdust --test linux_setup_ext4 -- --ignored claude_codex_setup_hooks_and_mcp_work_on_ext4
cargo test --locked -p agentdust --test linux_port

systemd-run --user --scope env TMPDIR="$TMPDIR" AGENTDUST_EXT4_TEST_ROOT="$AGENTDUST_EXT4_TEST_ROOT" AGENTDUST_REQUIRE_SYSTEMD_PIDFD=1 cargo test --locked -p agentdust --test linux_pidfd
systemd-run --user --scope env TMPDIR="$TMPDIR" AGENTDUST_EXT4_TEST_ROOT="$AGENTDUST_EXT4_TEST_ROOT" AGENTDUST_FIXTURE_SLEEPER="$target/debug/fixture-sleeper" cargo test --locked -p agentdust --test linux_automatic -- --ignored --test-threads=1

worker_data="$home/worker-data"
worker_project="$home/worker-project"
install -d -m 700 "$worker_data" "$worker_project"
AGENTDUST_DATA_DIR="$worker_data" "$target/release/agentdust" auto enable --yes "$worker_project"
systemctl --user is-active --quiet agentdust-automatic.service
AGENTDUST_DATA_DIR="$worker_data" "$target/release/agentdust" auto status | python3 -c 'import json,sys; assert json.load(sys.stdin)["worker_running"]'
systemctl --user restart agentdust-automatic.service
systemctl --user is-active --quiet agentdust-automatic.service
AGENTDUST_DATA_DIR="$worker_data" "$target/release/agentdust" auto status | python3 -c 'import json,sys; assert json.load(sys.stdin)["worker_running"]'
systemctl --user disable --now agentdust-automatic.service
rm "$HOME/.config/systemd/user/agentdust-automatic.service"
systemctl --user daemon-reload

mkdir -p "$TMPDIR/npm-build/bin" "$TMPDIR/npm-build/vendor/linux-x64" "$TMPDIR/npm-dist" "$TMPDIR/npm-install"
cp npm/package.json LICENSE README.md "$TMPDIR/npm-build/"
install -m 755 npm/bin/agentdust.js "$TMPDIR/npm-build/bin/agentdust.js"
install -m 755 "$target/release/agentdust" "$TMPDIR/npm-build/vendor/linux-x64/agentdust"
npm --prefix "$TMPDIR/npm-build" version "$version" --no-git-tag-version --ignore-scripts --allow-same-version
npm pack "$TMPDIR/npm-build" --pack-destination "$TMPDIR/npm-dist" --ignore-scripts
npm install --global --prefix "$TMPDIR/npm-install" "$TMPDIR/npm-dist/agentdust-$version.tgz" --ignore-scripts --no-audit --no-fund
test "$("$TMPDIR/npm-install/bin/agentdust" version)" = "agentdust $version"
cmp "$target/release/agentdust" "$TMPDIR/npm-build/vendor/linux-x64/agentdust"

host_pid=${AGENTDUST_WINDOWS_HOST_PID:?Windows host process PID required}
if [ -e "/proc/$host_pid" ]; then
    host_comm=$(cat "/proc/$host_pid/comm")
    host_exe=$(readlink "/proc/$host_pid/exe" || true)
    case "$host_comm $host_exe" in
        *powershell.exe*|*pwsh.exe*)
            echo 'A Windows host process appeared in the WSL process inventory' >&2
            exit 1
            ;;
    esac
fi

printf 'PASS: WSL2 ext4, Claude Code and Codex setup, journal, PID identity, ports, automatic worker, npm package and host process isolation\n'
