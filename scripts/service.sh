#!/usr/bin/env bash
# ==============================================================================
# Cortex Service Manager (start | stop | restart | status | logs)
# Supports Linux systemd (system & user), macOS launchd, and standalone daemon.
# ==============================================================================
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VENV_DIR="$ROOT_DIR/.venv"
ENV_FILE="$ROOT_DIR/.env"
PID_FILE="$ROOT_DIR/var/cortex-api.pid"
LOG_FILE="$ROOT_DIR/var/cortex.log"
SERVICE_NAME="cortex-api"
LAUNCHD_PLIST="${HOME}/Library/LaunchAgents/com.cortex.api.plist"

env_value() {
  local key="$1" default="$2"
  local value
  value=$(grep -E "^${key}=" "$ENV_FILE" 2>/dev/null | tail -n1 | cut -d= -f2- || true)
  printf '%s' "${value:-$default}"
}

HOST=$(env_value CORTEX_API_HOST "0.0.0.0")
PORT=$(env_value CORTEX_API_PORT "8003")

can_sudo() {
  [ "$(id -u)" -eq 0 ] || (command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null)
}

is_systemd_system() {
  can_sudo && command -v systemctl >/dev/null 2>&1 && systemctl list-unit-files "$SERVICE_NAME.service" 2>/dev/null | grep -q "$SERVICE_NAME.service"
}

is_systemd_user() {
  command -v systemctl >/dev/null 2>&1 && systemctl --user list-unit-files "$SERVICE_NAME.service" 2>/dev/null | grep -q "$SERVICE_NAME.service"
}

is_launchd() {
  [ -f "$LAUNCHD_PLIST" ] && command -v launchctl >/dev/null 2>&1
}

service_start() {
  mkdir -p "$ROOT_DIR/var"
  if is_systemd_system; then
    printf "Starting via systemd (system)...\n"
    sudo systemctl start "$SERVICE_NAME"
  elif is_systemd_user; then
    printf "Starting via systemd (user)...\n"
    systemctl --user start "$SERVICE_NAME"
  elif is_launchd; then
    printf "Starting via macOS launchctl...\n"
    launchctl load -w "$LAUNCHD_PLIST" 2>/dev/null || true
  else
    printf "Starting Cortex API standalone daemon on %s:%s...\n" "$HOST" "$PORT"
    if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
      printf "Cortex API is already running (PID: %s).\n" "$(cat "$PID_FILE")"
      return 0
    fi
    nohup "$VENV_DIR/bin/python" -c 'from lib.entrypoints import api_entrypoint; api_entrypoint()' >> "$LOG_FILE" 2>&1 &
    echo $! > "$PID_FILE"
    printf "Started standalone daemon with PID %s\n" "$(cat "$PID_FILE")"
  fi
}

service_stop() {
  if is_systemd_system; then
    printf "Stopping via systemd (system)...\n"
    sudo systemctl stop "$SERVICE_NAME" || true
  elif is_systemd_user; then
    printf "Stopping via systemd (user)...\n"
    systemctl --user stop "$SERVICE_NAME" || true
  elif is_launchd; then
    printf "Stopping via macOS launchctl...\n"
    launchctl unload "$LAUNCHD_PLIST" 2>/dev/null || true
  else
    if [ -f "$PID_FILE" ]; then
      local pid
      pid=$(cat "$PID_FILE")
      printf "Stopping standalone daemon PID %s...\n" "$pid"
      kill "$pid" 2>/dev/null || true
      rm -f "$PID_FILE"
    else
      printf "No PID file found at %s.\n" "$PID_FILE"
    fi
  fi
}

service_restart() {
  service_stop
  sleep 1
  service_start
}

service_status() {
  if is_systemd_system; then
    systemctl status "$SERVICE_NAME" --no-pager
  elif is_systemd_user; then
    systemctl --user status "$SERVICE_NAME" --no-pager
  elif is_launchd; then
    launchctl list | grep "com.cortex.api" || printf "Launchd service not running.\n"
  else
    if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
      printf "Cortex API standalone daemon is running (PID: %s).\n" "$(cat "$PID_FILE")"
    else
      printf "Cortex API standalone daemon is NOT running.\n"
    fi
  fi
}

service_logs() {
  if is_systemd_system; then
    journalctl -u "$SERVICE_NAME" -f -n 50
  elif is_systemd_user; then
    journalctl --user -u "$SERVICE_NAME" -f -n 50
  else
    if [ -f "$LOG_FILE" ]; then
      tail -f -n 50 "$LOG_FILE"
    elif [ -f "$ROOT_DIR/var/cortex-service.log" ]; then
      tail -f -n 50 "$ROOT_DIR/var/cortex-service.log"
    else
      printf "Log file not found at %s\n" "$LOG_FILE"
    fi
  fi
}

case "${1:-status}" in
  start) service_start ;;
  stop) service_stop ;;
  restart) service_restart ;;
  status) service_status ;;
  logs) service_logs ;;
  help|-h|--help)
    printf "Usage: %s {start|stop|restart|status|logs}\n" "$0"
    exit 0
    ;;
  *)
    printf "Usage: %s {start|stop|restart|status|logs}\n" "$0" >&2
    exit 1
    ;;
esac
