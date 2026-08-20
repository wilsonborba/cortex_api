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
* **Configure in `.env`:**
  ```env
  CORTEX_OLLAMA_BASE_URL=http://localhost:11434
  CORTEX_VISION_MODEL=qwen2.5vl:7b
  ```

---

## 2. OpenAI Codex CLI

Codex CLI allows routing reasoning and coding tasks through local Codex authentication.

* **Credentials Path:** `~/.codex/auth.json` (or configured via `CORTEX_CODEX_AUTH_PATH`).
* **Authentication:**
  ```bash
  codex login
  ```
* **Configure in `.env`:**
  ```env
  CORTEX_CODEX_AUTH_PATH=~/.codex/auth.json
  CORTEX_CODEX_COMMAND=codex
  ```

---

## 3. Google Antigravity CLI / AGY Docker

Antigravity CLI provides access to Google Gemini models (Gemini 2.0 Flash, Gemini 1.5 Pro).

* **Authentication:**
  ```bash
  agy auth login
  ```
* **Configure in `.env`:**
  ```env
  CORTEX_AGY_COMMAND=agy
  CORTEX_AGY_DOCKER_COMMAND=agy-docker
  ```

---

## 4. Claude Docker

Claude Docker runs Anthropic models inside an isolated container runner.

* **Credentials Path:** `~/.claude/.credentials.json` (or configured via `CORTEX_CLAUDE_CREDENTIALS_PATH`).
* **Configure in `.env`:**
  ```env
  CORTEX_CLAUDE_CREDENTIALS_PATH=~/.claude/.credentials.json
  CORTEX_CLAUDE_DOCKER_COMMAND=claude-docker
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
* **Configure in `.env`:**
  ```env
  CORTEX_WHISPER_LOCAL_ENABLED=true
  CORTEX_WHISPER_LOCAL_MODEL=large-v3-turbo-q5_0
  CORTEX_WHISPER_MODELS_DIR=var/whisper-models
  ```
