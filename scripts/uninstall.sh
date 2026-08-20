#!/usr/bin/env bash
# ==============================================================================
# Cortex Clean Uninstaller
# Stops and removes background services (systemd / launchd), firewall rules,
# virtual environment, and optionally purges data and logs.
# ==============================================================================
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SERVICE_NAME="cortex-api"
SYSTEMD_SYSTEM_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
SYSTEMD_USER_FILE="${HOME}/.config/systemd/user/${SERVICE_NAME}.service"
LAUNCHD_PLIST="${HOME}/Library/LaunchAgents/com.cortex.api.plist"
PID_FILE="$ROOT_DIR/var/cortex-api.pid"
VENV_DIR="$ROOT_DIR/.venv"

PURGE_DATA=false
REMOVE_VENV=true

for arg in "$@"; do
  case "$arg" in
    --purge-data) PURGE_DATA=true ;;
    --keep-venv) REMOVE_VENV=false ;;
    --help|-h)
      cat <<EOF
Usage: ./scripts/uninstall.sh [OPTIONS]

Options:
  --purge-data   Remove database (var/cortex.db), logs, and configuration
  --keep-venv    Preserve .venv directory
  -h, --help     Show this help message
EOF
      exit 0
      ;;
    *)
      printf 'Unknown argument: %s\n' "$arg" >&2
      exit 1
      ;;
  esac
done

log() {
  printf '[cortex-uninstall] %s\n' "$1"
}

stop_and_remove_services() {
  # 1. Systemd system
  if command -v systemctl >/dev/null 2>&1; then
    if sudo -n systemctl list-unit-files "$SERVICE_NAME.service" 2>/dev/null | grep -q "$SERVICE_NAME.service" || [ "$(id -u)" -eq 0 ]; then
      log "Stopping and disabling systemd system unit..."
      sudo systemctl stop "$SERVICE_NAME" 2>/dev/null || true
      sudo systemctl disable "$SERVICE_NAME" 2>/dev/null || true
      if [ -f "$SYSTEMD_SYSTEM_FILE" ]; then
        sudo rm -f "$SYSTEMD_SYSTEM_FILE"
        sudo systemctl daemon-reload
      fi
    fi

    # 2. Systemd user
    if systemctl --user list-unit-files "$SERVICE_NAME.service" 2>/dev/null | grep -q "$SERVICE_NAME.service"; then
      log "Stopping and disabling systemd user unit..."
      systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true
      systemctl --user disable "$SERVICE_NAME" 2>/dev/null || true
      if [ -f "$SYSTEMD_USER_FILE" ]; then
        rm -f "$SYSTEMD_USER_FILE"
        systemctl --user daemon-reload
      fi
    fi
  fi

  # 3. macOS Launchd
  if [ -f "$LAUNCHD_PLIST" ] && command -v launchctl >/dev/null 2>&1; then
    log "Unloading macOS launchd agent..."
    launchctl unload "$LAUNCHD_PLIST" 2>/dev/null || true
    rm -f "$LAUNCHD_PLIST"
  fi

  # 4. Standalone PID
  if [ -f "$PID_FILE" ]; then
    local pid
    pid=$(cat "$PID_FILE" 2>/dev/null || echo "")
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      log "Stopping standalone daemon process PID $pid..."
      kill "$pid" 2>/dev/null || true
    fi
    rm -f "$PID_FILE"
  fi
}

remove_ufw_rules() {
  if ! command -v ufw >/dev/null 2>&1; then
    return
  fi

  if ! sudo -n ufw status 2>/dev/null | grep -q 'Status: active'; then
    return
  fi

  log "Removing Cortex firewall rules..."
  while read -r rule_number; do
    [ -n "$rule_number" ] && sudo ufw --force delete "$rule_number" >/dev/null 2>&1 || true
  done < <(sudo ufw status numbered 2>/dev/null | grep 'cortex-api' | sed -n 's/^\[ \{0,1\}\([0-9]\+\)\].*/\1/p' | sort -rn || true)
}

main() {
  log "Beginning Cortex uninstallation..."

  stop_and_remove_services
  remove_ufw_rules

  if [ "$REMOVE_VENV" = true ] && [ -d "$VENV_DIR" ]; then
    log "Removing Python virtual environment ($VENV_DIR)..."
    rm -rf "$VENV_DIR"
  fi

  if [ "$PURGE_DATA" = true ]; then
    log "Purging var/ directory (database, logs, temporary assets)..."
    rm -rf "$ROOT_DIR/var"
  fi

  log "Uninstallation completed successfully."
}

main
