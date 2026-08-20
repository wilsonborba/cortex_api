# Cortex — Multi-Model AI Orchestration Engine

[![Python 3.11+](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![FastAPI](https://img.shields.io/badge/API-FastAPI-009688.svg)](https://fastapi.tiangolo.com)
[![Scalar Docs](https://img.shields.io/badge/Docs-Scalar-purple.svg)](http://localhost:8003/scalar)

**Cortex** is an enterprise-grade AI orchestration and routing engine. Instead of binding your application to a single vendor or static model endpoint, Cortex routes prompts dynamically across **14+ AI providers** based on **Quality/Effort Tiers (T0–T5)**, live latency envelopes, quota availability, and evidence-based performance telemetry.

---

## Key Capabilities

* **Dynamic Effort Tiers (T0 to T5):** Direct local execution for simple queries up to multi-step reasoning pipelines (Generator $\rightarrow$ Refiner $\rightarrow$ Critic) for complex programming and analysis.
* **Provider Independence & Fail-Soft Isolation:** Native support for local Ollama, Groq, Google AI Studio, OpenRouter, Mistral, Cohere, Cloudflare Workers AI, NVIDIA NIM, SambaNova, HuggingFace, SiliconFlow, Zai, Aion Labs, Inference.net, Claude Docker, Antigravity CLI, and Codex CLI. **The absence of any optional local tool or CLI never blocks Cortex**; it seamlessly degrades to active cloud/free-tier providers.
* **OpenAI-Compatible Facade (`/v1`):** Drop-in compatibility for AI coding assistants and tools (**OpenCode**, **Claude Code**, **Cursor**, **Continue**, **Roo Code**, **Aider**, standard `openai` SDKs).
* **Sliding-Window Quota & Cooldown Tracker:** Automatically tracks token budgets and handles HTTP 429 rate limits by putting affected providers into temporary cooldown while rerouting requests in-flight.
* **Hardware-Aware Auto-Profiling:** Inspects CPU cores, RAM, NVIDIA CUDA, and Apple Silicon Metal to recommend and install the optimal configuration profile (**Light**, **Medium**, or **Complete**).
* **Multi-Modal & Video Processing:** Ingests images and audio (local Whisper transcription) along with asynchronous multi-modal video analysis.
* **Interactive Scalar Documentation:** Rich API documentation served at `/scalar` and documented across English, Portuguese, and Thai.

---

## Quality & Effort Tiers (T0–T5)

| Tier | Name | Target Latency | Pipeline Architecture | Verification / Critic | RAG / Context |
| :---: | :--- | :---: | :---: | :---: | :---: |
| **T0** | **Basic** | $\le 5$s | Single fast/local model | None | None |
| **T1** | **Light** | $\le 10$s | Single fast model | None | Lightweight Context |
| **T2** | **Standard** | $\le 20$s | Single/Dual Model | None | Vector + Rerank |
| **T3** | **Advanced** | $\le 45$s | Multi-model (Generator $\rightarrow$ Refiner) | None | Full Web & Memory |
| **T4** | **High** | $\le 90$s | Multi-model (Research $\rightarrow$ Primary $\rightarrow$ Refiner) | Optional | Deep RAG |
| **T5** | **Ultra** | $\le 180$s | Decomposed Pipeline (Primary $\rightarrow$ Refiner $\rightarrow$ Reviser) | **Mandatory** (Critic Loop) | Deep + Web RAG |

---

## Supported Operating Systems

* **Linux:** Debian, Ubuntu, Xubuntu, Kali Linux, and other Debian-based distributions.
* **macOS:** Apple Silicon (M1/M2/M3/M4) with Metal acceleration and Intel x86_64.
* **Windows (via WSL2):** Fully supported under Windows Subsystem for Linux (Debian or Ubuntu on WSL2).

---

## Installation Profiles & Hardware Detection

When installing, Cortex runs `./scripts/detect_hardware.sh` to profile your machine and select the most appropriate installation profile:

* **`light` (Cloud-First / Low Footprint):**
  * Recommended for systems with $< 8$ GB RAM, $< 4$ CPU cores, or basic cloud VMs.
  * Installs core orchestration and cloud/free-tier API drivers. Omits heavy local C++ compilation (`pywhispercpp`) and large local model downloads.
* **`medium` (Balanced Workstation):**
  * Recommended for systems with $8$ to $16$ GB RAM and standard multi-core processors.
  * Includes core orchestration plus database connectors and web crawler utilities.
* **`complete` (Full Local Stack):**
  * Recommended for systems with $\ge 16$ GB RAM, NVIDIA CUDA GPU ($\ge 6$ GB VRAM), or Apple Silicon with Unified Memory.
  * Installs full local media audio transcription bindings, developer testing suite, and advanced local models.

---

## Quickstart

### 1. Clone & Run the Automated Installer
```bash
git clone https://github.com/wilsonborba/cortex.git
cd cortex

# Run the installer (auto-detects hardware profile and configures background service)
./scripts/install.sh
```

### 2. Explicit Profile or Non-Service Installation
```bash
# Force a lightweight cloud-first installation without installing background systemd service:
./scripts/install.sh --profile light --no-service

# Custom bind host and port:
./scripts/install.sh --host 0.0.0.0 --port 8003
```

### 3. Configure Provider API Keys
Edit `.env` to provide keys for any cloud providers you wish to activate (e.g. Groq, Google AI Studio, OpenRouter, Mistral, SambaNova):
```bash
# Example in .env
CORTEX_GROQ_API_KEY=gsk_...
CORTEX_GOOGLE_AI_STUDIO_API_KEY=...
CORTEX_OPENROUTER_API_KEY=sk-or-v1-...
```

---

## Service Lifecycle Management

Manage the Cortex API background service using the cross-platform `./scripts/service.sh` utility:

```bash
# Check status
./scripts/service.sh status

# Start, stop, or restart
./scripts/service.sh start
./scripts/service.sh stop
./scripts/service.sh restart

# View live API logs
./scripts/service.sh logs
```

---

## OpenAI-Compatible API (`/v1`)

Cortex provides an OpenAI-compatible facade on `http://localhost:8003/v1` for instant integration with tools like **OpenCode**, **Claude Code**, **Cursor**, **Continue**, and standard Python/Node SDKs.

### Virtual Model Names
* `cortex-auto`: Heuristic auto-classification based on prompt complexity.
* `cortex-t0` to `cortex-t5`: Explicit effort tier targeting.

### Example: Using with `curl`
```bash
curl http://localhost:8003/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "cortex-t3",
    "messages": [
      {"role": "system", "content": "You are a software architect."},
      {"role": "user", "content": "Explain the advantages of event sourcing."}
    ],
    "stream": false
  }'
```

### Example: Using with Python OpenAI SDK
```python
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:8003/v1",
    api_key="not-needed",  # Cortex handles provider authentication internally
)

response = client.chat.completions.create(
    model="cortex-t3",
    messages=[
        {"role": "user", "content": "Write an async Python client for SQLite WAL mode."}
    ],
)
print(response.choices[0].message.content)
```

### Example: Using with OpenCode / Claude Code / Cursor
Set the custom OpenAI API endpoint in your tool configuration:
* **Base URL:** `http://localhost:8003/v1` (or `http://<lan-ip>:8003/v1` when running on server)
* **Model:** `cortex-t3` (or `cortex-auto`)
* **API Key:** Any arbitrary string (e.g. `cortex`)

---

## Native API Endpoints

The Cortex native REST API listens on `0.0.0.0:8003` by default:

* `POST /execute` — Multi-tier execution with optional RAG and multi-model pipeline.
* `GET /models` & `POST /models/sync` — Model catalog and live provider discovery.
* `GET /quota` — Sliding-window token tracking and cooldown monitoring.
* `GET /tiers` & `PATCH /tiers/{tier}` — Tier latency and policy envelope configuration.
* `GET /routing/pins` & `POST /routing/pins` — Pin models/strategies to tiers.
* `GET /telemetry/stats` — Latency, token usage, cost, and success metrics.
* `WS /logs/stream` — Real-time live log streaming WebSocket.
* `POST /attachments/video` — Asynchronous multi-modal video analysis.

---

## Interactive Documentation & Multilingual Specs

* **Interactive Scalar Documentation:** [`http://localhost:8003/scalar`](http://localhost:8003/scalar)
* **Interactive Swagger UI:** [`http://localhost:8003/docs`](http://localhost:8003/docs)
* **OpenAPI 3.1 Specification:** [`http://localhost:8003/openapi.json`](http://localhost:8003/openapi.json)

### Multilingual Technical References:
* 🇬🇧 **[English API Reference](docs/scalar/api_reference_en.md)**
* 🇧🇷 **[Referência da API em Português](docs/scalar/api_reference_pt.md)**
* 🇹🇭 **[เอกสารอ้างอิง API ภาษาไทย](docs/scalar/api_reference_th.md)**

---

## Uninstallation

To cleanly stop all background services, remove firewall rules, virtual environments, and optionally purge data:

```bash
# Standard uninstall (keeps database and logs):
./scripts/uninstall.sh

# Complete purge (removes services, venv, SQLite database, and logs):
./scripts/uninstall.sh --purge-data
```

---

## Troubleshooting & Diagnostics

* **Missing Python Virtualenv Package on Debian/Ubuntu/Kali:**
  ```bash
  sudo apt update && sudo apt install -y python3-venv python3-pip build-essential
  ```
* **Port Conflict on 8003:**
  Update `CORTEX_API_PORT` in `.env` or run `./scripts/install.sh --port 8005`.
* **Checking Hardware Detection:**
  ```bash
  ./scripts/detect_hardware.sh
  # or JSON output:
  ./scripts/detect_hardware.sh --json
  ```
* **Provider Cooldowns & Rate Limits:**
  Cortex automatically clears cooldowns after the cooldown interval (default 15 minutes). You can trigger an immediate probe with `cortex models sync` or `POST /models/sync`.
