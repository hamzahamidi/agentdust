#!/usr/bin/env bash
set -euo pipefail

uid=$(id -u agentdust)
loginctl enable-linger agentdust
systemctl start "user@$uid.service"
systemctl is-active --quiet "user@$uid.service"
