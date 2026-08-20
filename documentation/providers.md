# Cortex AI Providers Setup Guide

This guide covers all **14+ AI providers** natively supported by Cortex. For each provider, you will find the official registration portal, steps to generate an API key, the exact environment variable required in `.env`, and verification commands.

---

## Provider Overview Table

| Provider | Primary Strengths | Official Portal | Environment Variable |
| :--- | :--- | :--- | :--- |
| **Groq** | Ultra-fast inference (Llama 3.3, Whisper) | [console.groq.com](https://console.groq.com) | `CORTEX_GROQ_API_KEY` |
| **Google AI Studio** | Gemini 2.0 / 1.5 Flash & Pro | [aistudio.google.com](https://aistudio.google.com) | `CORTEX_GOOGLE_AI_STUDIO_API_KEY` |
| **OpenRouter** | Multi-vendor aggregator (DeepSeek, Claude, Llama) | [openrouter.ai](https://openrouter.ai) | `CORTEX_OPENROUTER_API_KEY` |
| **Mistral AI** | Mistral Small, Large, Codestral, Pixtral | [console.mistral.ai](https://console.mistral.ai) | `CORTEX_MISTRAL_API_KEY` |
| **Cohere** | Command-R, Command-R+, Embeddings | [dashboard.cohere.com](https://dashboard.cohere.com) | `CORTEX_COHERE_API_KEY` |
| **Cloudflare Workers AI** | Serverless Llama, Mistral, Qwen | [dash.cloudflare.com](https://dash.cloudflare.com) | `CORTEX_CLOUDFLARE_API_KEY`<br>`CORTEX_CLOUDFLARE_ACCOUNT_ID` |
| **NVIDIA NIM** | Llama 3.3 70B, Nemotron, DeepSeek R1 | [build.nvidia.com](https://build.nvidia.com) | `CORTEX_NVIDIA_API_KEY` |
| **SambaNova** | High-throughput Llama 3.3 70B & 405B | [cloud.sambanova.ai](https://cloud.sambanova.ai) | `CORTEX_SAMBANOVA_API_KEY` |
| **Hugging Face** | Open-source serverless models | [huggingface.co/settings/tokens](https://huggingface.co/settings/tokens) | `CORTEX_HUGGINGFACE_API_KEY` |
| **Ollama Cloud** | Managed cloud Ollama instances | [ollama.com](https://ollama.com) | `CORTEX_OLLAMA_CLOUD_API_KEY` |
| **SiliconFlow** | DeepSeek V3/R1, Qwen 2.5 | [siliconflow.cn](https://cloud.siliconflow.cn) | `CORTEX_SILICONFLOW_API_KEY` |
| **Zai / Zhipu AI** | GLM-4, GLM-4V multi-modal | [open.bigmodel.cn](https://open.bigmodel.cn) | `CORTEX_ZAI_API_KEY` |
| **Aion Labs** | Specialized code and reasoning models | [aionlabs.ai](https://aionlabs.ai) | `CORTEX_AION_LABS_API_KEY` |
| **Inference.net** | High-speed open models | [inference.net](https://inference.net) | `CORTEX_INFERENCE_NET_API_KEY` |
| **Requesty** | Multi-model routing gateway | [requesty.ai](https://requesty.ai) | `CORTEX_REQUESTY_API_KEY` |
| **Ollama (Local)** | Offline privacy-first local LLMs | [ollama.com](https://ollama.com) | `CORTEX_OLLAMA_BASE_URL` |

---

## Detailed Setup Instructions

### 1. Groq
* **Usage:** Instantaneous low-latency generation (T0-T3) and cloud audio transcription.
* **Get Key:** Go to [Groq Console](https://console.groq.com/keys) $\rightarrow$ Create API Key.
* **Configure in `.env`:**
  ```env
  CORTEX_GROQ_API_KEY=gsk_...
  ```
* **Verify:**
  ```bash
  cortex models list --provider groq
  ```

### 2. Google AI Studio
* **Usage:** High-context reasoning, coding, and multi-modal comprehension.
* **Get Key:** Go to [Google AI Studio](https://aistudio.google.com/app/apikey) $\rightarrow$ Get API Key.
* **Configure in `.env`:**
  ```env
  CORTEX_GOOGLE_AI_STUDIO_API_KEY=AIzaSy...
  ```

### 3. OpenRouter
* **Usage:** Broad catalog of open and proprietary models with unified free/paid quotas.
* **Get Key:** Go to [OpenRouter Keys](https://openrouter.ai/keys) $\rightarrow$ Create Key.
* **Configure in `.env`:**
  ```env
  CORTEX_OPENROUTER_API_KEY=sk-or-v1-...
  ```

### 4. Mistral AI
* **Usage:** Advanced multilingual reasoning and Codestral programming pipelines.
* **Get Key:** Go to [Mistral Console](https://console.mistral.ai/api-keys/) $\rightarrow$ Create new key.
* **Configure in `.env`:**
  ```env
  CORTEX_MISTRAL_API_KEY=...
  ```

### 5. Cloudflare Workers AI
* **Usage:** Global edge inference with zero cold starts.
* **Get Key:** Go to [Cloudflare Dashboard](https://dash.cloudflare.com/) $\rightarrow$ AI $\rightarrow$ Workers AI $\rightarrow$ Manage API Tokens.
* **Configure in `.env`:**
  ```env
  CORTEX_CLOUDFLARE_API_KEY=...
  CORTEX_CLOUDFLARE_ACCOUNT_ID=...
  ```

### 6. NVIDIA NIM
* **Usage:** High-performance Llama 3.3 70B and Nemotron 70B models.
* **Get Key:** Go to [NVIDIA Build](https://build.nvidia.com/) $\rightarrow$ Generate API Key.
* **Configure in `.env`:**
  ```env
  CORTEX_NVIDIA_API_KEY=nvapi-...
  ```

### 7. SambaNova Systems
* **Usage:** Ultra-fast full precision Llama 3.3 70B and 405B models.
* **Get Key:** Go to [SambaNova Cloud](https://cloud.sambanova.ai/) $\rightarrow$ API Keys.
* **Configure in `.env`:**
  ```env
  CORTEX_SAMBANOVA_API_KEY=...
  ```

---

## Verifying Provider Health

To trigger a live sync across all configured providers and verify that credentials are authenticated:

```bash
# Via Cortex CLI:
cortex models sync

# Or via HTTP API:
curl -X POST http://localhost:8003/models/sync
```

Models with valid credentials will transition to `AVAILABLE` status immediately. Any unconfigured provider models will remain gracefully in `OFFLINE` status without affecting other providers.
