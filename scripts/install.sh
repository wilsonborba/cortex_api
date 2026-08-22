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
HOST_WAS_EXPLICIT="false"
PORT_WAS_EXPLICIT="false"
NON_INTERACTIVE="false"
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
  --port <port>                           API Port (default: 8003 or existing .env)
  --non-interactive, -y                   Accept all recommendations without prompting
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
      HOST_WAS_EXPLICIT="true"
      shift 2
      ;;
    --port)
      DEFAULT_PORT="$2"
      PORT_WAS_EXPLICIT="true"
      shift 2
      ;;
    --non-interactive|-y)
      NON_INTERACTIVE="true"
      shift
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

upsert_env_value() {
  local key="$1"
  local value="$2"
  if grep -q "^${key}=" "$ENV_FILE" 2>/dev/null; then
    sed -i "s|^${key}=.*|${key}=${value}|" "$ENV_FILE" 2>/dev/null || true
  else
    printf '%s=%s\n' "$key" "$value" >> "$ENV_FILE"
  fi
}

env_has_key() {
  local key="$1"
  grep -q "^${key}=" "$ENV_FILE" 2>/dev/null
}

service_account_user() {
  if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    printf '%s' "$SUDO_USER"
    return
  fi
  id -un
}

service_account_group() {
  local user
  user=$(service_account_user)
  id -gn "$user" 2>/dev/null || id -gn
}

service_account_home() {
  local user
  user=$(service_account_user)
  getent passwd "$user" 2>/dev/null | cut -d: -f6
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
  local auto_profile="medium"
  if [ -f "$detector" ]; then
    chmod +x "$detector"
    "$detector"
    auto_profile=$("$detector" --profile-only 2>/dev/null || echo "medium")
  fi

  if [ "$PROFILE" = "auto" ] && env_has_key CORTEX_PROFILE; then
    PROFILE=$(env_value CORTEX_PROFILE "$auto_profile")
    log_info "Using existing profile override from .env: '${PROFILE}'."
  fi

  if [ "$PROFILE" != "auto" ]; then
    case "$PROFILE" in
      light|medium|complete)
        SELECTED_PROFILE="$PROFILE"
        log_info "Profile selected via argument: '${SELECTED_PROFILE}'"
        ;;
      *)
        log_err "Invalid profile '$PROFILE'. Allowed values: auto, light, medium, complete."
        exit 1
        ;;
    esac
    return
  fi

  if [ "$NON_INTERACTIVE" = "true" ] || [ ! -t 0 ]; then
    SELECTED_PROFILE="$auto_profile"
    log_info "Using recommended profile '${SELECTED_PROFILE}'."
    return
  fi

  printf "\n%s\n" "========================================================"
  printf " Select Installation Profile:\n"
  printf "  [1] Light    - Low memory (< 8GB), cloud-first API, no local Whisper/models\n"
  printf "  [2] Medium   - Balanced setup (8-16GB RAM, crawler + database tools)\n"
  printf "  [3] Complete - Full local stack (16GB+ RAM / GPU, local audio transcription)\n"
  printf "%s\n" "--------------------------------------------------------"
  printf " Recommended Profile: [%s]\n" "$auto_profile"
  read -r -p " Press ENTER to accept recommendation or type choice [light/medium/complete]: " user_choice || user_choice=""

  case "$user_choice" in
    1|light|Light) SELECTED_PROFILE="light" ;;
    2|medium|Medium) SELECTED_PROFILE="medium" ;;
    3|complete|Complete) SELECTED_PROFILE="complete" ;;
    "") SELECTED_PROFILE="$auto_profile" ;;
    *)
      log_warn "Unrecognized choice '$user_choice'; proceeding with recommendation '${auto_profile}'."
      SELECTED_PROFILE="$auto_profile"
      ;;
  esac
  log_info "Effective installation profile: '${SELECTED_PROFILE}'"
}

is_port_in_use() {
  local port="$1"
  "$PYTHON_BIN" -c "import socket; s = socket.socket(socket.AF_INET, socket.SOCK_STREAM); s.settimeout(1); res = s.connect_ex(('127.0.0.1', int('$port'))); s.close(); exit(0 if res == 0 else 1)" 2>/dev/null
}

is_port_owned_by_cortex() {
  local port="$1"
  "$PYTHON_BIN" -c "
import urllib.request
try:
    with urllib.request.urlopen('http://127.0.0.1:$port/models', timeout=2) as r:
        exit(0 if r.status == 200 else 1)
except Exception:
    exit(1)
" 2>/dev/null
}

resolve_effective_port() {
  local desired_port
  desired_port=$(env_value CORTEX_API_PORT "$DEFAULT_PORT")

  if ! is_port_in_use "$desired_port"; then
    EFFECTIVE_PORT="$desired_port"
    log_info "Port $EFFECTIVE_PORT is available."
    return
  fi

  if is_port_owned_by_cortex "$desired_port"; then
    EFFECTIVE_PORT="$desired_port"
    log_info "Port $EFFECTIVE_PORT is currently in use by Cortex; preserving existing port configuration."
    return
  fi

  log_warn "Port $desired_port is occupied by another service. Searching for open port..."
  local candidate=$(( desired_port + 1 ))
  while [ "$candidate" -le 65535 ]; do
    if ! is_port_in_use "$candidate"; then
      EFFECTIVE_PORT="$candidate"
      log_info "Found available port: $EFFECTIVE_PORT."
      break
    fi
    candidate=$(( candidate + 1 ))
  done
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
  mkdir -p "$ROOT_DIR/var" "$ROOT_DIR/lib/dal/var" "$ROOT_DIR/lib/dal/seeds"
  chown -R "$(service_account_user):$(service_account_group)" "$ROOT_DIR/var" "$ROOT_DIR/lib/dal/var" "$ROOT_DIR/lib/dal/seeds" 2>/dev/null || true

  if [ "$HOST_WAS_EXPLICIT" != "true" ] && env_has_key CORTEX_API_HOST; then
    DEFAULT_HOST=$(env_value CORTEX_API_HOST "$DEFAULT_HOST")
  fi

  if [ ! -f "$ENV_FILE" ]; then
    if [ -f "$ENV_EXAMPLE" ]; then
      cp "$ENV_EXAMPLE" "$ENV_FILE"
      log_info "Created .env from .env.example"
    else
      cat <<EOF > "$ENV_FILE"
# Optional provider credentials
ANTHROPIC_API_KEY=
OPENAI_API_KEY=
GOOGLE_API_KEY=

# Optional Cortex provider credentials
CORTEX_GROQ_API_KEY=
CORTEX_GOOGLE_AI_STUDIO_API_KEY=
CORTEX_OPENROUTER_API_KEY=
EOF
      log_info "Created minimal .env file"
    fi
  else
    log_info "Using existing .env configuration file."
  fi

  if env_has_key CORTEX_PROFILE || [ "$SELECTED_PROFILE" != "complete" ]; then
    upsert_env_value CORTEX_PROFILE "$SELECTED_PROFILE"
  fi

  if env_has_key CORTEX_API_HOST || [ "$HOST_WAS_EXPLICIT" = "true" ] || [ "$DEFAULT_HOST" != "0.0.0.0" ]; then
    upsert_env_value CORTEX_API_HOST "$DEFAULT_HOST"
  fi

  if env_has_key CORTEX_API_PORT || [ "$PORT_WAS_EXPLICIT" = "true" ] || [ "$EFFECTIVE_PORT" != "8003" ]; then
    upsert_env_value CORTEX_API_PORT "$EFFECTIVE_PORT"
  fi

  configure_external_cli_paths
}


configure_external_cli_paths() {
  local current_user current_home detected
  current_user=$(service_account_user)
  current_home=$(service_account_home)
  if [ -z "$current_home" ]; then
    current_home=$HOME
  fi

  detected=$(command -v agy 2>/dev/null || true)
  if [ -n "$detected" ] && [ "$(env_value CORTEX_AGY_COMMAND agy)" = "agy" ]; then
    upsert_env_value CORTEX_AGY_COMMAND "$detected"
  fi

  detected=$(command -v claude 2>/dev/null || true)
  if [ -n "$detected" ] && [ "$(env_value CORTEX_CLAUDE_COMMAND claude)" = "claude" ]; then
    upsert_env_value CORTEX_CLAUDE_COMMAND "$detected"
  fi

  detected=$(command -v codex 2>/dev/null || true)
  if [ -n "$detected" ] && [ "$(env_value CORTEX_CODEX_COMMAND codex)" = "codex" ]; then
    upsert_env_value CORTEX_CODEX_COMMAND "$detected"
  fi

  if [ "$(env_value CORTEX_CLAUDE_CREDENTIALS_PATH '~/.claude/.credentials.json')" = '~/.claude/.credentials.json' ]; then
    upsert_env_value CORTEX_CLAUDE_CREDENTIALS_PATH "$current_home/.claude/.credentials.json"
  fi
  if [ "$(env_value CORTEX_CODEX_AUTH_PATH '~/.codex/auth.json')" = '~/.codex/auth.json' ]; then
    upsert_env_value CORTEX_CODEX_AUTH_PATH "$current_home/.codex/auth.json"
  fi
}

initialize_database() {
  log_info "Initializing database schema and running migrations..."
  "$VENV_DIR/bin/python" -c "from lib.dal.migrations import upgrade_db; from lib.core.settings import get_settings; upgrade_db(get_settings().database_url)" || {
    log_warn "Database migration check encountered a warning; continuing."
  }
}

setup_systemd_linux() {
  local current_user current_group current_home service_path
  current_user=$(service_account_user)
  current_group=$(service_account_group)
  current_home=$(service_account_home)
  if [ -z "$current_home" ]; then
    current_home=$HOME
  fi
  service_path="$current_home/.local/bin:$VENV_DIR/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

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
Environment=HOME=${current_home}
Environment=PATH=${service_path}
Environment=XDG_CONFIG_HOME=${current_home}/.config
ExecStart=${VENV_DIR}/bin/python -c 'from lib.entrypoints import api_entrypoint; api_entrypoint()'
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
    log_info "Systemd system service active and started on port ${EFFECTIVE_PORT}."
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
Environment=HOME=${current_home}
Environment=PATH=${service_path}
Environment=XDG_CONFIG_HOME=${current_home}/.config
ExecStart=${VENV_DIR}/bin/python -c 'from lib.entrypoints import api_entrypoint; api_entrypoint()'
Restart=always
RestartSec=3

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable "$SERVICE_NAME" >/dev/null 2>&1 || true
    systemctl --user restart "$SERVICE_NAME"
    log_info "User systemd service active and started on port ${EFFECTIVE_PORT}."
  else
    log_warn "systemctl not available in this environment; starting standalone daemon."
    "$SCRIPTS_DIR/service.sh" start
  fi
}

setup_launchd_macos() {
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
        <string>-c</string>
        <string>from lib.entrypoints import api_entrypoint; api_entrypoint()</string>
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
  log_info "macOS launchd agent active and started on port ${EFFECTIVE_PORT}."
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

  sudo ufw allow proto tcp from 127.0.0.1 to any port "$EFFECTIVE_PORT" comment 'cortex-api-local' >/dev/null 2>&1 || true
  sudo ufw allow proto tcp from 10.0.0.0/8 to any port "$EFFECTIVE_PORT" comment 'cortex-api-lan-10' >/dev/null 2>&1 || true
  sudo ufw allow proto tcp from 172.16.0.0/12 to any port "$EFFECTIVE_PORT" comment 'cortex-api-lan-172' >/dev/null 2>&1 || true
  sudo ufw allow proto tcp from 192.168.0.0/16 to any port "$EFFECTIVE_PORT" comment 'cortex-api-lan-192' >/dev/null 2>&1 || true
  log_info "UFW firewall rules configured for LAN access on port ${EFFECTIVE_PORT}."
}


json_field() {
  local field_path="$1"
  local json_input
  json_input=$(cat)
  JSON_INPUT="$json_input" python3 - "$field_path" <<'JSONPY'
import json, os, sys
field = sys.argv[1]
raw = os.environ.get("JSON_INPUT", "")
if not raw.strip():
    print("")
    raise SystemExit(0)
data = json.loads(raw)
value = data
for part in field.split('.'):
    if part.isdigit():
        value = value[int(part)]
    elif isinstance(value, dict):
        value = value.get(part)
    else:
        value = None
        break
if isinstance(value, (dict, list)):
    print(json.dumps(value))
elif value is None:
    print("")
else:
    print(value)
JSONPY
}

run_healthcheck() {
  if [ "$SKIP_HEALTHCHECK" = "true" ]; then
    return
  fi

  log_info "Verifying API server health at http://127.0.0.1:${EFFECTIVE_PORT}..."

  "$VENV_DIR/bin/python" - <<PY
import time
from urllib.error import URLError
from urllib.request import urlopen

port = ${EFFECTIVE_PORT}
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
  resolve_effective_port
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

  printf "\n========================================================\n"
  printf " \033[0;32mCortex is ready to use!\033[0m\n"
  printf "========================================================\n"
  printf " Installed Profile:      %s\n" "$SELECTED_PROFILE"
  printf " Effective API Port:     %s\n" "$EFFECTIVE_PORT"
  printf " API Address:            http://%s:%s\n" "$DEFAULT_HOST" "$EFFECTIVE_PORT"
  printf " Scalar API Docs:        http://localhost:%s/docs\n" "$EFFECTIVE_PORT"
  printf " System Capabilities:    http://localhost:%s/system/capabilities\n" "$EFFECTIVE_PORT"
  printf " OpenAI-Compatible API:  http://localhost:%s/v1/chat/completions\n" "$EFFECTIVE_PORT"
  printf "%s\n" "--------------------------------------------------------"
  if [ "$SELECTED_PROFILE" = "light" ]; then
    printf " Note (Light Profile):\n"
    printf "   - Core orchestration & 14+ cloud providers are active.\n"
    printf "   - Local audio Whisper/media extensions are omitted.\n"
    printf "   - Cloud audio transcription is available via CORTEX_GROQ_API_KEY.\n"
    printf "%s\n" "--------------------------------------------------------"
  fi
  printf " Useful Commands:\n"
  printf "   cortex --help                # CLI command line interface\n"
  printf "   ./scripts/service.sh status  # Check service status\n"
  printf "   ./scripts/service.sh logs    # View live API logs\n"
  printf "   ./scripts/uninstall.sh       # Uninstall Cortex and service\n"
  printf "========================================================\n\n"
}

main "$@"
