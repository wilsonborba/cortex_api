#!/usr/bin/env bash
set -euo pipefail

# Installs Cortex's Ollama resource policy as a systemd drop-in. Run with:
#   sudo ./scripts/configure_ollama_queue_limits.sh
if [[ ${EUID} -ne 0 ]]; then
  echo "Run this script with sudo so it can update the system Ollama service." >&2
  exit 1
fi

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
source_file="${script_dir}/../deploy/ollama/cortex-queue-limits.conf"
target_dir="/etc/systemd/system/ollama.service.d"
target_file="${target_dir}/cortex-queue-limits.conf"

install -d -m 0755 "${target_dir}"
install -m 0644 "${source_file}" "${target_file}"
systemctl daemon-reload
systemctl restart ollama.service
systemctl is-active --quiet ollama.service
echo "Ollama queue limits installed and ollama.service is active."
