#!/usr/bin/env bash
# Build Rust and prepare the existing system service. No database migration.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$(pwd)
mode=${1:---prepare}
case "$mode" in --prepare|--activate) ;; *) printf 'Use install.sh [--prepare|--activate]\n' >&2; exit 1;; esac
if [ "$mode" = --activate ] && [ "$(id -u)" != 0 ]; then
  printf 'Administrator rights are required to activate the system service.\n' >&2
  exit 1
fi
if [ "$mode" = --prepare ]; then
  cargo build --release --locked --bins -j2
  mkdir -p var/systemd
  cat > var/systemd/cortex-api.service <<UNIT
[Unit]
Description=Cortex Rust API
After=network-online.target hippocampus-api.service
Wants=network-online.target

[Service]
Type=simple
User=$(id -un)
Group=$(id -gn)
WorkingDirectory=$root
EnvironmentFile=$root/.env
ExecStart=$root/target/release/cortex-api --production
Restart=on-failure
RestartSec=3
NoNewPrivileges=yes
PrivateTmp=yes
UMask=027

[Install]
WantedBy=multi-user.target
UNIT
  printf 'Build and service file ready: var/systemd/cortex-api.service\n'
else
  test -f var/systemd/cortex-api.service
  test -x target/release/cortex-api
  install -m 644 var/systemd/cortex-api.service /etc/systemd/system/cortex-api.service
  systemctl daemon-reload
  systemctl enable cortex-api.service
  systemctl restart cortex-api.service
fi
