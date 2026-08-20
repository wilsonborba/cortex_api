# Cortex Documentation

Welcome to the comprehensive operational and configuration documentation for **Cortex**.

This directory contains detailed technical guides to help developers, system administrators, and AI coding agents install, configure, operate, and debug Cortex in production and development environments.

---

## Documentation Map

* 🚀 **[Installation & Profiles Guide (`installation.md`)](./installation.md)**
  Detailed guide covering automated installation, hardware profiling, `Light`, `Medium`, and `Complete` profile selection, platform support (Debian/Ubuntu/Kali Linux, macOS, WSL2), service lifecycle (Systemd & Launchd), and smart port allocation.

* ⚙️ **[Configuration & Environment Variables (`configuration.md`)](./configuration.md)**
  Complete reference for all `.env` and environment variables, database backends (SQLite & PostgreSQL), server network bindings, quota window tuning, scoring weights, and logging targets.

* 🌐 **[AI Providers Setup Guide (`providers.md`)](./providers.md)**
  Exhaustive setup instructions for all 14+ supported AI providers (Groq, Google AI Studio, OpenRouter, Mistral, Cohere, Cloudflare, NVIDIA NIM, SambaNova, HuggingFace, Ollama, SiliconFlow, Zai, Aion Labs, Inference.net). Includes official portal URLs, API key acquisition steps, exact environment variable names, and verification commands.

* 🛠️ **[External AI Tools & CLI Setup (`external-tools.md`)](./external-tools.md)**
  Setup, authentication, and container configuration for external tools and optional local runners: Codex CLI, Google Antigravity CLI / AGY Docker, Claude Docker, local Ollama, and local Whisper.cpp.

* 🔍 **[Troubleshooting & Diagnostics (`troubleshooting.md`)](./troubleshooting.md)**
  Diagnostic procedures for common installation failures, missing system dependencies (`python3-venv`, `build-essential`, `ffmpeg`), port conflict resolutions, rate limit cooldowns, and runtime capability degradation.

---

## Interactive API Documentation

* **Interactive Scalar Documentation:** [`http://localhost:8003/docs`](http://localhost:8003/docs) (or [`http://localhost:8003/scalar`](http://localhost:8003/scalar))
* **OpenAPI 3.1 Specification:** [`http://localhost:8003/openapi.json`](http://localhost:8003/openapi.json)
* **Multilingual API Reference Specs:**
  * 🇬🇧 **[English API Reference](./scalar/api_reference_en.md)**
  * 🇧🇷 **[Referência da API em Português](./scalar/api_reference_pt.md)**
  * 🇹🇭 **[เอกสารอ้างอิง API ภาษาไทย](./scalar/api_reference_th.md)**
