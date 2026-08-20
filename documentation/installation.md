# Cortex Installation & Profiles Guide

This document details the end-to-end installation process, hardware capability detection, installation profiles, service configuration, and platform-specific instructions for Cortex.

---

## 1. Installation Workflow Overview

Cortex uses a unified installer (`./scripts/install.sh`) that orchestrates the entire setup from cloning to service activation:

```text
git clone https://github.com/wilsonborba/cortex.git
cd cortex
        ↓
run ./scripts/install.sh
        ↓
1. Validate Python >= 3.11 & venv tooling
2. Profile hardware (CPU cores, RAM, GPU/CUDA/Metal)
3. Recommend profile (Light, Medium, Complete)
4. Allow user confirmation or manual selection
5. Probe & resolve API port (defaults to 8003, reuses existing Cortex port on update)
6. Create virtualenv & install profile-specific Python dependencies
7. Initialize SQLite database & run migrations
8. Install & start background service (Systemd on Linux, Launchd on macOS)
9. Configure LAN firewall rules (UFW if active)
10. Run healthcheck probe & display completion summary
```

---

## 2. Installation Profiles

Cortex supports three runtime profiles to ensure reliable execution across both low-spec hardware and high-performance multi-GPU workstations. **Every profile produces a 100% operational Cortex system.**

### Comparison Table

| Profile | Hardware Target | Included Components | Omitted / Unavailable Capabilities |
| :--- | :--- | :--- | :--- |
| **`light`** | $< 8$ GB RAM, $< 4$ CPU cores, basic cloud VMs | Core orchestration, FastAPI, Uvicorn, SQLAlchemy, Typer, DuckDuckGo/Trafilatura, all 14+ cloud providers | Local C++ Whisper compilation (`pywhispercpp`), heavy local vision models. *(Cloud audio transcription via Groq is supported)* |
| **`medium`** | $8$ to $16$ GB RAM, modern multi-core CPU | Core orchestration, Web crawler extensions (`crawl4ai`), PostgreSQL connectors (`psycopg`), standard Ollama integration | Heavy local C++ Whisper compilation (unless build tools present) |
| **`complete`** | $\ge 16$ GB RAM, NVIDIA CUDA GPU ($\ge 6$ GB VRAM), or Apple Silicon Metal | Core orchestration, Web crawler, PostgreSQL, local Whisper.cpp C++ audio transcription, developer test suite (`pytest`) | None (all capabilities enabled) |

---

## 3. Platform Setup Instructions

### 3.1 Debian, Ubuntu, Kali Linux & Derivatives
Ensure standard build and virtualenv packages are present before running the installer:
```bash
sudo apt update
sudo apt install -y python3 python3-pip python3-venv build-essential curl git
./scripts/install.sh
```

### 3.2 macOS (Apple Silicon M-Series & Intel)
Ensure Homebrew and Python 3.11+ are installed:
```bash
brew install python@3.12 git
./scripts/install.sh
```
*Note: The installer automatically configures a Launchd user agent (`~/Library/LaunchAgents/com.cortex.api.plist`) and leverages Apple Silicon Unified Memory.*

### 3.3 Windows via WSL2 (Recommended)
Native Windows is not supported. Use Windows Subsystem for Linux (WSL2):
1. In Windows PowerShell: `wsl --install -d Ubuntu`
2. Inside the Ubuntu WSL shell:
   ```bash
   sudo apt update && sudo apt install -y python3 python3-venv python3-pip build-essential
   git clone https://github.com/wilsonborba/cortex.git
   cd cortex
   ./scripts/install.sh
   ```

---

## 4. Smart Port Allocation & Reinstallation Handling

* **Default Port:** `8003` (binds to `0.0.0.0` by default).
* **Port Conflict Resolution:** If port `8003` is occupied by an unrelated third-party service, the installer automatically detects the next available port (`8004`, `8005`, ...) and persists `CORTEX_API_PORT` into `.env`.
* **Reinstall / Update Idempotence:** If port `8003` is currently occupied by an existing Cortex instance, the installer recognizes the existing instance, preserves the port, and updates the service in-place **without port drift**.

---

## 5. Service Lifecycle Management

```bash
# Check status of the background service
./scripts/service.sh status

# Start, stop, or restart the API
./scripts/service.sh start
./scripts/service.sh stop
./scripts/service.sh restart

# Follow real-time API logs
./scripts/service.sh logs
```

---

## 6. Uninstallation

```bash
# Standard uninstall (stops service, removes systemd/launchd, cleans .venv):
./scripts/uninstall.sh

# Complete purge (removes service, .venv, and purges SQLite database and log files):
./scripts/uninstall.sh --purge-data
```
