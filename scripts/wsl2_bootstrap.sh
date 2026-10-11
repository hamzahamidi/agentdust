#!/usr/bin/env bash
set -euo pipefail

source_dir=${1:?repository path required}
test -f "$source_dir/Cargo.toml"

apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y build-essential ca-certificates curl libssl-dev nodejs npm pkg-config python3

if ! id agentdust >/dev/null 2>&1; then
    useradd --create-home --shell /bin/bash agentdust
fi

install -d -o agentdust -g agentdust /home/agentdust/work /home/agentdust/tmp /home/agentdust/target
cp -a "$source_dir"/. /home/agentdust/work/agentdust/
chown -R agentdust:agentdust /home/agentdust/work/agentdust
printf '[boot]\nsystemd=true\n[user]\ndefault=agentdust\n' > /etc/wsl.conf
