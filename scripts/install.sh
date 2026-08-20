#!/usr/bin/env bash
# ==============================================================================
# Cortex Complete End-to-End Installer
# Supports Debian/Ubuntu/Kali/Debian-derivatives, macOS, and Windows via WSL.
# ==============================================================================
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VENV_DIR="$ROOT_DIR/.venv"
ENV_FILE="$ROOT_DIR/.env"
ENV_EXAMPLE="$ROOT_DIR/.env.example"
SCRIPTS_DIR="$ROOT_DIR/scripts"

PYTHON_BIN=${PYTHON_BIN:-python3}
SERVICE_NAME="cortex-api"
SYSTEMD_SYSTEM_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
SYSTEMD_USER_FILE="${SYSTEMD_USER_DIR}/${SERVICE_NAME}.service"
LAUNCHD_PLIST="${HOME}/Library/LaunchAgents/com.cortex.api.plist"

DEFAULT_HOST="0.0.0.0"
DEFAULT_PORT="8003"
PROFILE="auto"
INSTALL_SERVICE="true"
CONFIGURE_FIREWALL="true"
SKIP_HEALTHCHECK="false"

log_info() {
  printf '[cortex-install] \033[0;32mINFO:\033[0m %s\n' "$1"
}

log_warn() {
  printf '[cortex-install] \033[0;33mWARN:\033[0m %s\n' "$1"
}

log_err() {
  printf '[cortex-install] \033[0;31mERROR:\033[0m %s\n' "$1" >&2
}

show_help() {
  cat <<EOF
Usage: ./scripts/install.sh [OPTIONS]

Options:
  --profile <auto|light|medium|complete>  Installation profile (default: auto)
  --host <ip>                             API Host bind address (default: 0.0.0.0)
  --port <port>                           API Port (default: 8003)
  --no-service                            Do not install system/user background service
  --no-ufw                                Skip UFW firewall rules configuration
  --skip-healthcheck                      Skip post-install API connectivity healthcheck
  -h, --help                              Show this help message

Profiles:
  light     Minimal dependencies for low-resource machines (< 8GB RAM, no GPU). Cloud-first.
  medium    Balanced setup with crawler/database tooling for standard workstations.
  complete  Full local media/audio bindings and developer toolchain for high-end systems.
EOF
}

# Parse command line options
while [ $# -gt 0 ]; do
  case "$1" in
    --profile)
      PROFILE="$2"
      shift 2
      ;;
    --host)
      DEFAULT_HOST="$2"
      shift 2
      ;;
    --port)
      DEFAULT_PORT="$2"
      shift 2
      ;;
    --no-service)
      INSTALL_SERVICE="false"
      shift
      ;;
    --no-ufw)
      CONFIGURE_FIREWALL="false"
      shift
      ;;
    --skip-healthcheck)
      SKIP_HEALTHCHECK="true"
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      log_err "Unknown argument: $1"
      show_help
      exit 1
      ;;
  esac
done

env_value() {
  local key="$1" default="$2"
  local value
  value=$(grep -E "^${key}=" "$ENV_FILE" 2>/dev/null | tail -n1 | cut -d= -f2- || true)
  printf '%s' "${value:-$default}"
}

check_python_environment() {
  log_info "Validating Python runtime environment..."

  if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
    log_err "Python 3 is not installed or not in PATH."
    log_err "Installation command:"
    log_err "  - Debian/Ubuntu/Kali: sudo apt update && sudo apt install -y python3 python3-pip python3-venv build-essential"
    log_err "  - macOS: brew install python@3.12"
    exit 1
  fi

  local py_ver
  py_ver=$("$PYTHON_BIN" -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")' 2>/dev/null || echo "0.0")
  local py_major
  py_major=$(echo "$py_ver" | cut -d. -f1)
  local py_minor
  py_minor=$(echo "$py_ver" | cut -d. -f2)

  if [ "$py_major" -lt 3 ] || { [ "$py_major" -eq 3 ] && [ "$py_minor" -lt 11 ]; }; then
    log_err "Python version detected: $py_ver. Cortex requires Python >= 3.11."
    log_err "Please upgrade Python:"
    log_err "  - Debian/Ubuntu: sudo apt install -y python3.11 python3.11-venv || sudo apt install -y python3.12 python3.12-venv"
    log_err "  - macOS: brew install python@3.12"
    exit 1
  fi

  log_info "Python runtime OK: $($PYTHON_BIN --version 2>/dev/null)"
}

detect_and_select_profile() {
  local detector="$SCRIPTS_DIR/detect_hardware.sh"
  if [ ! -f "$detector" ]; then
    log_warn "detect_hardware.sh not found; falling back to profile 'medium'."
    SELECTED_PROFILE="medium"
    return
  fi

  chmod +x "$detector"
  local auto_profile
  auto_profile=$("$detector" --profile-only 2>/dev/null || echo "medium")

  if [ "$PROFILE" = "auto" ]; then
    SELECTED_PROFILE="$auto_profile"
    log_info "Auto-detected system capabilities. Selected profile: '${SELECTED_PROFILE}'"
  else
    case "$PROFILE" in
      light|medium|complete)
        SELECTED_PROFILE="$PROFILE"
        log_info "Explicit profile selected: '${SELECTED_PROFILE}'"
        ;;
      *)
        log_err "Invalid profile '$PROFILE'. Allowed values: auto, light, medium, complete."
        exit 1
        ;;
    esac
  fi
}

create_virtualenv() {
  log_info "Setting up Python virtual environment at ${VENV_DIR}..."

  if [ ! -d "$VENV_DIR" ]; then
    if ! "$PYTHON_BIN" -m venv "$VENV_DIR" 2>/dev/null; then
      log_err "Failed to create virtualenv using '$PYTHON_BIN -m venv'."
      log_err "Missing virtualenv support on Debian/Ubuntu/Kali. Fix with:"
      log_err "  sudo apt update && sudo apt install -y python3-venv"
      exit 1
    fi
  fi

  if [ ! -f "$VENV_DIR/bin/pip" ]; then
    log_err "Virtual environment is missing pip. Recreating..."
    rm -rf "$VENV_DIR"
    "$PYTHON_BIN" -m venv "$VENV_DIR"
  fi

  log_info "Upgrading pip and wheel tooling..."
  "$VENV_DIR/bin/pip" install --upgrade pip setuptools wheel >/dev/null
}

install_project_dependencies() {
  log_info "Installing Cortex package and dependencies for profile '${SELECTED_PROFILE}'..."

  case "$SELECTED_PROFILE" in
    light)
      log_info "Installing core package dependencies (cloud-first, light footprint)..."
      "$VENV_DIR/bin/pip" install -e "$ROOT_DIR"
      ;;
    medium)
      log_info "Installing package with crawler and postgres extensions..."
      if ! "$VENV_DIR/bin/pip" install -e "$ROOT_DIR[crawler,postgres]"; then
        log_warn "Optional medium dependencies failed to install; falling back to core dependencies."
        "$VENV_DIR/bin/pip" install -e "$ROOT_DIR"
      fi
      ;;
    complete)
      log_info "Installing full stack (media, crawler, postgres, dev)..."
      if ! "$VENV_DIR/bin/pip" install -e "$ROOT_DIR[crawler,postgres,media,dev]"; then
        log_warn "Media/C++ compilation extension failed (e.g. missing build tools); installing core + dev stack..."
        "$VENV_DIR/bin/pip" install -e "$ROOT_DIR[crawler,postgres,dev]"
      fi
      ;;
  esac

  log_info "Dependencies successfully installed."
}

configure_environment() {
  mkdir -p "$ROOT_DIR/var"

  if [ ! -f "$ENV_FILE" ]; then
    if [ -f "$ENV_EXAMPLE" ]; then
      cp "$ENV_EXAMPLE" "$ENV_FILE"
      log_info "Created .env from .env.example"
    else
      cat <<EOF > "$ENV_FILE"
CORTEX_DATABASE_URL=sqlite:///var/cortex.db
CORTEX_API_HOST=${DEFAULT_HOST}
CORTEX_API_PORT=${DEFAULT_PORT}
CORTEX_ENVIRONMENT=development
CORTEX_LOG_LEVEL=INFO
CORTEX_LOG_FILE=var/cortex.log
EOF
      log_info "Created minimal .env file"
    fi
  else
    log_info "Using existing .env configuration file."
  fi

  # Ensure CORTEX_API_HOST and CORTEX_API_PORT exist in .env
  if ! grep -q "^CORTEX_API_HOST=" "$ENV_FILE" 2>/dev/null; then
    printf '\nCORTEX_API_HOST=%s\n' "$DEFAULT_HOST" >> "$ENV_FILE"
  fi
  if ! grep -q "^CORTEX_API_PORT=" "$ENV_FILE" 2>/dev/null; then
    printf 'CORTEX_API_PORT=%s\n' "$DEFAULT_PORT" >> "$ENV_FILE"
  fi
}

initialize_database() {
  log_info "Initializing database schema and running migrations..."
  "$VENV_DIR/bin/python" -c "from lib.dal.migrations import upgrade_db; from lib.core.settings import get_settings; upgrade_db(get_settings().database_url)" || {
    log_warn "Database migration check encountered a warning; continuing."
  }
}

setup_systemd_linux() {
  local host port
  host=$(env_value CORTEX_API_HOST "$DEFAULT_HOST")
  port=$(env_value CORTEX_API_PORT "$DEFAULT_PORT")
  local current_user current_group
  current_user=$(id -un)
  current_group=$(id -gn)

  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null || [ "$(id -u)" -eq 0 ]; then
    log_info "Installing systemd system service (${SERVICE_NAME})..."
    sudo tee "$SYSTEMD_SYSTEM_FILE" >/dev/null <<EOF
[Unit]
Description=Cortex Multi-Model AI Orchestrator API
After=network.target

[Service]
Type=simple
User=${current_user}
Group=${current_group}
WorkingDirectory=${ROOT_DIR}
EnvironmentFile=${ENV_FILE}
ExecStart=${VENV_DIR}/bin/python -m uvicorn lib.presentation.api.app:create_app --factory --host ${host} --port ${port}
Restart=always
RestartSec=3
NoNewPrivileges=yes
PrivateTmp=yes
UMask=027

[Install]
WantedBy=multi-user.target
EOF
    sudo systemctl daemon-reload
    sudo systemctl enable "$SERVICE_NAME" >/dev/null 2>&1 || true
    sudo systemctl restart "$SERVICE_NAME"
    log_info "Systemd system service active and started."
  elif command -v systemctl >/dev/null 2>&1; then
    log_info "Installing user systemd service (~/.config/systemd/user/${SERVICE_NAME}.service)..."
    mkdir -p "$SYSTEMD_USER_DIR"
    cat <<EOF > "$SYSTEMD_USER_FILE"
[Unit]
Description=Cortex Multi-Model AI Orchestrator API
After=network.target

[Service]
Type=simple
WorkingDirectory=${ROOT_DIR}
EnvironmentFile=${ENV_FILE}
ExecStart=${VENV_DIR}/bin/python -m uvicorn lib.presentation.api.app:create_app --factory --host ${host} --port ${port}
Restart=always
RestartSec=3

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable "$SERVICE_NAME" >/dev/null 2>&1 || true
    systemctl --user restart "$SERVICE_NAME"
    log_info "User systemd service active and started."
  else
    log_warn "systemctl not available in this environment; use ./scripts/service.sh start to run as daemon."
    "$SCRIPTS_DIR/service.sh" start
  fi
}

setup_launchd_macos() {
  local host port
  host=$(env_value CORTEX_API_HOST "$DEFAULT_HOST")
  port=$(env_value CORTEX_API_PORT "$DEFAULT_PORT")

  log_info "Installing macOS launchd agent (${LAUNCHD_PLIST})..."
  mkdir -p "$(dirname "$LAUNCHD_PLIST")"

  cat <<EOF > "$LAUNCHD_PLIST"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.cortex.api</string>
    <key>ProgramArguments</key>
    <array>
        <string>${VENV_DIR}/bin/python</string>
        <string>-m</string>
        <string>uvicorn</string>
        <string>lib.presentation.api.app:create_app</string>
        <string>--factory</string>
        <string>--host</string>
        <string>${host}</string>
        <string>--port</string>
        <string>${port}</string>
    </array>
    <key>WorkingDirectory</key>
    <string>${ROOT_DIR}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${ROOT_DIR}/var/cortex-service.log</string>
    <key>StandardErrorPath</key>
    <string>${ROOT_DIR}/var/cortex-service.log</string>
</dict>
</plist>
EOF

  launchctl unload "$LAUNCHD_PLIST" 2>/dev/null || true
  launchctl load -w "$LAUNCHD_PLIST"
  log_info "macOS launchd agent active and started."
}

configure_ufw() {
  if [ "$CONFIGURE_FIREWALL" != "true" ]; then
    return
  fi

  if ! command -v ufw >/dev/null 2>&1; then
    return
  fi

  if ! sudo -n ufw status 2>/dev/null | grep -q 'Status: active'; then
    return
  fi

  local port
  port=$(env_value CORTEX_API_PORT "$DEFAULT_PORT")
  sudo ufw allow proto tcp from 127.0.0.1 to any port "$port" comment 'cortex-api-local' >/dev/null 2>&1 || true
  sudo ufw allow proto tcp from 10.0.0.0/8 to any port "$port" comment 'cortex-api-lan-10' >/dev/null 2>&1 || true
  sudo ufw allow proto tcp from 172.16.0.0/12 to any port "$port" comment 'cortex-api-lan-172' >/dev/null 2>&1 || true
  sudo ufw allow proto tcp from 192.168.0.0/16 to any port "$port" comment 'cortex-api-lan-192' >/dev/null 2>&1 || true
  log_info "UFW firewall rules configured for LAN access on port ${port}."
}

run_healthcheck() {
  if [ "$SKIP_HEALTHCHECK" = "true" ]; then
    return
  fi

  local port
  port=$(env_value CORTEX_API_PORT "$DEFAULT_PORT")
  log_info "Verifying API server health at http://127.0.0.1:${port}..."

  "$VENV_DIR/bin/python" - <<PY
import time
from urllib.error import URLError
from urllib.request import urlopen

port = ${port}
last_error = None
for attempt in range(15):
    try:
        with urlopen(f"http://127.0.0.1:{port}/models", timeout=3) as response:
            if response.status == 200:
                print(f"[cortex-install] API health check passed (HTTP {response.status}).")
                break
    except Exception as exc:
        last_error = exc
        time.sleep(1)
else:
    print(f"[cortex-install] Warning: Health check did not connect immediately: {last_error}")
    print("[cortex-install] Service may still be initializing or starting in background.")
PY
}

main() {
  printf "========================================================\n"
  printf "       Cortex Automated Installation & Setup\n"
  printf "========================================================\n"

  check_python_environment
  detect_and_select_profile
  create_virtualenv
  install_project_dependencies
  configure_environment
  initialize_database

  if [ "$INSTALL_SERVICE" = "true" ]; then
    local kernel
    kernel=$(uname -s 2>/dev/null || echo "Linux")
    if [ "$kernel" = "Darwin" ]; then
      setup_launchd_macos
    else
      setup_systemd_linux
    fi
    configure_ufw
    run_healthcheck
  else
    log_info "Skipping background service installation (--no-service)."
    log_info "To start manually: ./scripts/service.sh start (or 'cortex-api')"
  fi

  local host port
  host=$(env_value CORTEX_API_HOST "$DEFAULT_HOST")
  port=$(env_value CORTEX_API_PORT "$DEFAULT_PORT")

  printf "\n========================================================\n"
  printf " \033[0;32mCortex is ready to use!\033[0m\n"
  printf "========================================================\n"
  printf " Installed Profile:      %s\n" "$SELECTED_PROFILE"
  printf " API Address:            http://%s:%s\n" "$host" "$port"
  printf " Interactive Swagger:    http://localhost:%s/docs\n" "$port"
  printf " Scalar API Docs:        http://localhost:%s/scalar\n" "$port"
  printf " OpenAI-Compatible API:  http://localhost:%s/v1/chat/completions\n" "$port"
  printf "--------------------------------------------------------\n"
  printf " Useful Commands:\n"
  printf "   cortex --help                # CLI command line interface\n"
  printf "   ./scripts/service.sh status  # Check service status\n"
  printf "   ./scripts/service.sh logs    # View live API logs\n"
  printf "   ./scripts/uninstall.sh       # Uninstall Cortex and service\n"
  printf "========================================================\n\n"
}

main "$@"
