#!/usr/bin/env bash
# Remove only this service registration. Data, model files and backups remain.
set -euo pipefail
if [ "$(id -u)" != 0 ]; then
  printf 'Administrator rights are required to remove the system service.\n' >&2
  exit 1
fi
systemctl stop cortex-api.service
systemctl disable cortex-api.service
rm -f /etc/systemd/system/cortex-api.service
systemctl daemon-reload
