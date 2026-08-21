# Cortex External AI Tools & CLI Setup Guide

Cortex can route tasks to local AI CLI runners, containerized runners, and local inference daemons. This guide explains how to configure each optional tool.

> [!NOTE]
> All external tools described below are **strictly optional**. If none of them are installed, Cortex operates normally using configured cloud providers.

---

## 1. Local Ollama Daemon

Ollama runs open-weight models (Llama 3, Qwen 2.5, DeepSeek) locally on CPU, NVIDIA CUDA, or Apple Silicon GPU.

* **Installation:**
  ```bash
  # Linux:
  curl -fsSL https://ollama.com/install.sh | sh

  # macOS:
  brew install ollama
  ```
* **Recommended Models:**
  ```bash
  ollama pull qwen2.5:7b
  ollama pull dolphin3:8b
  ollama pull qwen2.5vl:7b   # Local vision model
  ```
* **Defaults and optional overrides:**
  `CORTEX_OLLAMA_BASE_URL` already defaults to `http://localhost:11434`. Add it to `.env` only when your Ollama endpoint differs.
  ```env
  # Optional override
  CORTEX_OLLAMA_BASE_URL=http://remote-host:11434
  ```

---

## 2. OpenAI Codex CLI

Codex CLI allows routing reasoning and coding tasks through local Codex authentication.

* **Credentials Path:** `~/.codex/auth.json` (or configured via `CORTEX_CODEX_AUTH_PATH`).
* **Authentication:**
  ```bash
  codex login
  ```
* **Optional `.env` overrides:**
  `CORTEX_CODEX_AUTH_PATH` and `CORTEX_CODEX_COMMAND` only belong in `.env` when your local setup differs from the built-in defaults.
  ```env
  CORTEX_CODEX_AUTH_PATH=/home/your-user/.codex/auth.json
  CORTEX_CODEX_COMMAND=/home/your-user/.local/bin/codex
  ```

---

## 3. Google Antigravity CLI / AGY

Antigravity CLI provides access to Google Gemini models (Gemini 2.0 Flash, Gemini 1.5 Pro). The default auto-detected command is `agy`; optional calibration-only custom judges can be configured separately.

* **Authentication:**
  ```bash
  agy auth login
  ```
* **Optional `.env` overrides:**
  Add these only when PATH resolution is not enough on your machine.
  ```env
  CORTEX_AGY_COMMAND=/home/your-user/.local/bin/agy
  CORTEX_CALIBRATION_JUDGE_COMMANDS={"agy:work":"/opt/tools/agy-work"}
  ```

---

## 4. Claude CLI

Claude CLI is auto-detected via the standard `claude` command. Optional calibration-only custom judges can be configured manually when you want a different local runner.

* **Credentials Path:** `~/.claude/.credentials.json` (or configured via `CORTEX_CLAUDE_CREDENTIALS_PATH`).
* **Optional `.env` overrides:**
  Use these only when your credentials path or binary path differs from the built-in defaults.
  ```env
  CORTEX_CLAUDE_CREDENTIALS_PATH=/home/your-user/.claude/.credentials.json
  CORTEX_CLAUDE_COMMAND=/home/your-user/.local/bin/claude
  CORTEX_DISABLED_PROVIDERS=claude
  CORTEX_CALIBRATION_JUDGE_COMMANDS={"claude:service":"/home/your-user/.local/bin/claude-service","claude:team":"/opt/tools/claude-team"}
  ```

---

## 5. Local Whisper.cpp Audio Transcription

High-performance offline audio transcription using quantized Whisper models.

* **Installation (included in `complete` profile):**
  ```bash
  pip install pywhispercpp
  ```
* **Model Download:**
  Cortex automatically downloads the model weights (`large-v3-turbo-q5_0`) into `var/whisper-models/` on first use.
* **Defaults and optional overrides:**
  Local Whisper already defaults to enabled with model `large-v3-turbo-q5_0` in `var/whisper-models`. Only add overrides when you intentionally want different behavior.
  ```env
  # Optional overrides
  CORTEX_WHISPER_LOCAL_ENABLED=false
  CORTEX_WHISPER_LOCAL_MODEL=large-v3-turbo-q5_0
  CORTEX_WHISPER_MODELS_DIR=var/whisper-models
  ```
