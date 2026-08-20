# Cortex Troubleshooting & Diagnostics Guide

This guide provides solutions for common installation issues, environment conflicts, service errors, and capability limitations.

---

## 1. Installation Issues

### `Missing virtualenv support on Debian/Ubuntu/Kali`
* **Symptom:** Installer fails with `Failed to create virtualenv using 'python3 -m venv'`.
* **Fix:** Install the missing Debian package:
  ```bash
  sudo apt update && sudo apt install -y python3-venv python3-pip build-essential
  ./scripts/install.sh
  ```

### `Python version < 3.11 detected`
* **Symptom:** Installer reports that Python 3.11 or newer is required.
* **Fix:** Install Python 3.11 or 3.12:
  * **Ubuntu/Debian:** `sudo apt install -y python3.11 python3.11-venv`
  * **macOS:** `brew install python@3.12`

---

## 2. Port Conflicts & Network Binding

### `Port 8003 is occupied`
* The installer automatically probes for an available port (`8004`, `8005`, ...) if `8003` is occupied by an unrelated service, updating `CORTEX_API_PORT` in `.env`.
* To check which process is using a port:
  ```bash
  sudo lsof -i :8003 || sudo ss -tulpn | grep 8003
  ```
* To specify a custom port explicitly:
  ```bash
  ./scripts/install.sh --port 8005
  ```

---

## 3. Service Management & Log Diagnostics

### Checking Service Health
```bash
# Check service status
./scripts/service.sh status

# Follow live logs
./scripts/service.sh logs

# Systemd system journal
sudo journalctl -u cortex-api -f -n 100
```

---

## 4. Capability Degradation & Missing Components

### `Audio transcription is unavailable under profile 'light'`
* **Cause:** The `light` profile omits local C++ Whisper compilation (`pywhispercpp`).
* **Fix:**
  1. Either configure a Groq API key in `.env` for cloud transcription:
     ```env
     CORTEX_GROQ_API_KEY=gsk_...
     ```
  2. Or reinstall with the complete profile:
     ```bash
     ./scripts/install.sh --profile complete
     ```

### `Video processing requires 'ffmpeg' binary in system PATH`
* **Cause:** FFmpeg is not installed on the operating system.
* **Fix:**
  * **Debian/Ubuntu:** `sudo apt install -y ffmpeg`
  * **macOS:** `brew install ffmpeg`

---

## 5. Quota & Rate Limit Cooldowns

### `No eligible model matches tier` / Model in `COOLING_DOWN`
* **Cause:** When a provider returns an HTTP 429 rate limit, Cortex automatically puts that model into cooldown (default 15 minutes) to protect your account and reroutes other tasks.
* **Fix:**
  * View active quotas and cooldown status:
    ```bash
    cortex quota
    ```
  * Trigger an immediate live probe to clear expired cooldowns:
    ```bash
    cortex models sync
    ```
