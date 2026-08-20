#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SERVICE_NAME="cortex-api"
SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
PURGE_DATA=false

for arg in "$@"; do
  case "$arg" in
    --purge-data) PURGE_DATA=true ;;
    *)
      printf 'Unknown argument: %s\n' "$arg" >&2
      exit 1
      ;;
  esac
done

log() {
  printf '[cortex-uninstall] %s\n' "$1"
}

remove_ufw_rules() {
  if ! command -v ufw >/dev/null 2>&1; then
    return
  fi

  if ! sudo ufw status | grep -q 'Status: active'; then
    return
  fi

  while read -r rule_number; do
    sudo ufw --force delete "$rule_number"
  done < <(sudo ufw status numbered | grep 'cortex-api-' | sed -n 's/^\[ \{0,1\}\([0-9]\+\)\].*/\1/p' | sort -rn)
}

main() {
  if sudo systemctl list-unit-files | grep -q "^${SERVICE_NAME}.service"; then
    sudo systemctl stop "$SERVICE_NAME" || true
    sudo systemctl disable "$SERVICE_NAME" || true
  fi

  if [ -f "$SERVICE_FILE" ]; then
    sudo rm -f "$SERVICE_FILE"
    sudo systemctl daemon-reload
  fi

  remove_ufw_rules

  if [ "$PURGE_DATA" = true ]; then
    rm -rf "$ROOT_DIR/var"
    log "Removed var (SQLite db + logs)."
  fi

  log "Uninstall complete."
}

main
