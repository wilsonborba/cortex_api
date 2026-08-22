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
7. Initialize database schema and migrations
8. Install & start background service (Systemd on Linux, Launchd on macOS)
9. Configure LAN firewall rules (UFW if active)
10. Run healthcheck probe & display completion summary
```

---

## 2. Configuration Architecture During Install

The installer follows the same configuration architecture as the application:
- non-secret defaults come from `lib/core/settings.py`;
- `.env` is for secrets, machine-specific values, and explicit overrides;
- existing legitimate overrides are preserved;
- generic defaults are not written into `.env` just to make Cortex run.

Examples:
- If `CORTEX_API_HOST` is absent, Cortex uses the built-in default `0.0.0.0`.
- If `CORTEX_API_HOST=127.0.0.1` already exists, the installer preserves it.
- If `CORTEX_API_PORT` must change because `8003` is occupied, the installer writes the resolved override into `.env`.
- If the default SQLite database is acceptable, you do not need `CORTEX_DATABASE_URL` in `.env`.

---

## 3. Installation Profiles

Cortex supports three runtime profiles to ensure reliable execution across both low-spec hardware and high-performance multi-GPU workstations. Every profile produces a functional Cortex installation.

| Profile | Hardware Target | Included Components | Omitted / Unavailable Capabilities |
| :--- | :--- | :--- | :--- |
| `light` | < 8 GB RAM, < 4 CPU cores, basic cloud VMs | Core orchestration, FastAPI, Uvicorn, SQLAlchemy, Typer, DuckDuckGo/Trafilatura, cloud providers | Local Whisper.cpp compilation and heavy local models |
| `medium` | 8 to 16 GB RAM, modern multi-core CPU | Core orchestration, crawler extensions, PostgreSQL connectors, standard Ollama integration | Heavy local Whisper.cpp compilation unless build tools are present |
| `complete` | >= 16 GB RAM, NVIDIA CUDA GPU, or Apple Silicon Metal | Core orchestration, crawler, PostgreSQL, local Whisper.cpp transcription, developer test suite | None |

---

## 4. Smart Port Allocation & Reinstallation Handling

- Built-in API defaults are `CORTEX_API_HOST=0.0.0.0` and `CORTEX_API_PORT=8003`.
- If port `8003` is occupied by another service, the installer finds the next available port and writes `CORTEX_API_PORT` to `.env` as an explicit override.
- If the configured port is already owned by Cortex, the installer preserves it and avoids drift.
- The generated service uses the Python entrypoint, so runtime host/port are resolved from `Settings` instead of being hard-coded into the unit file.

---

## 5. Service Lifecycle Management

```bash
./scripts/service.sh status
./scripts/service.sh start
./scripts/service.sh stop
./scripts/service.sh restart
./scripts/service.sh logs
```

---

## 6. Uninstallation

```bash
./scripts/uninstall.sh
./scripts/uninstall.sh --purge-data
```
