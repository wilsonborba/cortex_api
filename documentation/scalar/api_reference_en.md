# Cortex API Reference (Scalar Technical Specification)

Cortex provides a unified REST, WebSocket, and OpenAI-compatible API for orchestrating multi-model pipelines across 14+ AI providers (local Ollama, Groq, Google AI Studio, OpenRouter, Mistral, Cohere, Cloudflare, NVIDIA, SambaNova, HuggingFace, etc.) with automatic quota management, latency envelopes (Tiers T0-T5), and evidence-based routing.

* **Base URL:** `http://localhost:8003` (or `http://0.0.0.0:8003`)
* **Scalar Interactive UI:** `http://localhost:8003/scalar`
* **OpenAPI 3.1 JSON:** `http://localhost:8003/openapi.json`

---

## Table of Contents

1. [Execution Engine (`POST /execute`)](#1-execution-engine)
2. [OpenAI-Compatible Facade (`/v1/chat/completions`, `/v1/models`)](#2-openai-compatible-facade)
3. [Model Registry (`/models`, `/models/sync`, `/models/{model_id}`)](#3-model-registry)
4. [Quota & Token Tracking (`/quota`, `/quota/{provider}`)](#4-quota--token-tracking)
5. [Tiers & Execution Envelopes (`/tiers`, `/tiers/{tier}`)](#5-tiers--execution-envelopes)
6. [Routing Pins (`/routing/pins`)](#6-routing-pins)
7. [Telemetry & Audit Trail (`/telemetry/stats`, `/telemetry/events`)](#7-telemetry--audit-trail)
8. [Live Logs WebSocket (`WS /logs/stream`)](#8-live-logs-websocket)
9. [Multi-Modal Video Ingestion (`/attachments/video`)](#9-multi-modal-video-ingestion)

---

## 1. Execution Engine

### `POST /execute`
Executes a prompt through Cortex's multi-tier orchestration engine. Evaluates complexity, selects optimal models based on live scores and quotas, runs multi-step pipelines (Primary $\rightarrow$ Refiner $\rightarrow$ Critic), and retrieves web/memory context if required.

#### Request Body (`application/json`)
| Field | Type | Required | Default | Description |
| :--- | :--- | :--- | :--- | :--- |
| `prompt` | string | **Yes** | — | The main user prompt or instruction to execute. |
| `tier` | integer \| string | No | `null` | Explicit tier (`0` to `5`) or `"auto"` for automatic heuristic complexity classification. |
| `task_type` | string | No | `"general"` | Task category: `"general"`, `"coding"`, `"reasoning"`, `"creative"`, `"retrieval"`. |
| `needs_web` | boolean | No | `false` | When `true`, queries SearXNG/DuckDuckGo and injects web search context. |
| `use_memory` | boolean | No | `false` | When `true`, searches Hippocampus vector memory for relevant topic chunks. |
| `memory_topic` | string | No | `null` | Specific topic key for Hippocampus memory search/storage. |
| `force_model` | string | No | `null` | Layer-3 override: Forces execution using a specific model ID (e.g. `groq/llama-3.3-70b-versatile`). |
| `force_provider`| string | No | `null` | Layer-3 override: Forces execution on any available model from this provider (e.g. `groq`, `openrouter`). |
| `override_strategy` | string | No | `null` | Layer-3 override: Names a custom multi-model strategy pipeline. |
| `force_context_format` | string | No | `null` | Forces RAG context serialization format: `"toon"` (Token-Optimized Object Notation) or `"json"`. |
| `attachments` | array[object] | No | `[]` | Multi-modal attachments. Each object contains `{ "filename": str, "mime_type": str, "data_base64": str }`. |
| `attachment_job_id` | string (UUID) | No | `null` | ID of a completed video processing job (`/attachments/video/{id}`) to inject transcript & visual context. |

#### Request Example
```json
{
  "prompt": "Explain the architectural differences between SQLite WAL mode and traditional rollback journal.",
  "tier": 3,
  "task_type": "coding",
  "needs_web": false,
  "use_memory": false,
  "force_context_format": "toon"
}
```

#### Response (`200 OK`)
```json
{
  "request_id": "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d",
  "tier_requested": 3,
  "tier_executed": 3,
  "strategy_id": "coding_t3_groq_dynamic",
  "task_type": "coding",
  "success": true,
  "response_text": "In SQLite, the Write-Ahead Logging (WAL) mode fundamentally changes how transactions are committed...",
  "steps": [
    {
      "role": "primary",
      "provider": "groq",
      "model_id": "groq/llama-3.3-70b-versatile",
      "success": true,
      "response_text": "In SQLite, WAL mode...",
      "input_tokens": 420,
      "output_tokens": 850,
      "latency_ms": 1120,
      "cost_usd": 0.0,
      "error_type": null,
      "error_message": null,
      "attempts": 1
    }
  ],
  "total_input_tokens": 420,
  "total_output_tokens": 850,
  "total_cost_usd": 0.0,
  "latency_ms": 1120,
  "error_type": null
}
```

#### Status Codes & Error Responses
* `200 OK`: Successful execution.
* `409 Conflict` (`NoEligibleModelError`): No `AVAILABLE` model matches the tier/force requirements (e.g. all candidates in cooldown or provider unconfigured).
* `422 Unprocessable Entity`: Invalid attachment encoding, unknown format name, or video job incomplete/failed.
* `500 Internal Server Error`: Unhandled system error.

---

## 2. OpenAI-Compatible Facade

Cortex exposes an OpenAI-compatible wire protocol on `/v1` to integrate with IDE extensions (OpenCode, Claude Code, Cursor, Continue, Roo Code, Aider) and standard OpenAI SDKs (`openai` Python/JS package).

### `GET /v1/models`
Returns virtual model identifiers mapped to Cortex effort tiers.

#### Response (`200 OK`)
```json
{
  "object": "list",
  "data": [
    { "id": "cortex-auto", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t0", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t1", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t2", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t3", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t4", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t5", "object": "model", "created": 1740000000, "owned_by": "cortex" }
  ]
}
```

### `POST /v1/chat/completions`
Standard chat completion interface supporting non-streaming and Server-Sent Events (SSE) streaming.

#### Request Body
```json
{
  "model": "cortex-t3",
  "messages": [
    { "role": "system", "content": "You are an expert systems programmer." },
    { "role": "user", "content": "Refactor this SQL query for performance." }
  ],
  "stream": false
}
```

#### Non-Streaming Response (`200 OK`)
```json
{
  "id": "chatcmpl-a1b2c3d4e5f6789012345678",
  "object": "chat.completion",
  "created": 1740000050,
  "model": "cortex-t3",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Here is the optimized query using CTEs..."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 120,
    "completion_tokens": 340,
    "total_tokens": 460
  }
}
```

#### Streaming Response (`stream: true`)
Emits `text/event-stream` SSE chunks:
```text
data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":1740000050,"model":"cortex-t3","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}

data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":1740000050,"model":"cortex-t3","choices":[{"index":0,"delta":{"content":"Here is "},"finish_reason":null}]}

data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":1740000050,"model":"cortex-t3","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

---

## 3. Model Registry

### `GET /models`
Lists all catalogued models from cache (SQLite) with sub-millisecond latency.
* **Query Parameters:**
  * `provider` (string, optional): Filter by provider name (`groq`, `ollama`, `openrouter`, etc.).
  * `tier` (int, optional): Filter models eligible for tier (`0` to `5`).
  * `status` (string, optional): Filter by status (`AVAILABLE`, `OFFLINE`, `COOLING_DOWN`, `DISABLED_MANUALLY`, `REQUIRES_SUBSCRIPTION`).

### `POST /models/sync`
Performs an active live probe across all configured provider endpoints and authentication credentials, updating the persisted catalog cache.

### `GET /models/{model_id}`
Retrieves detailed metadata for a single model (e.g. `/models/groq/llama-3.3-70b-versatile`).

### `PATCH /models/{model_id}`
Updates persistent configuration for a model.
```json
{
  "tier_eligibility": [2, 3, 4],
  "is_enabled": true,
  "context_format_pin": "toon",
  "context_format_pin_ttl_seconds": 3600
}
```

---

## 4. Quota & Token Tracking

### `GET /quota`
Returns a summary of token consumption, sliding window headroom, and health status for all providers.

#### Response (`200 OK`)
```json
[
  {
    "provider": "groq",
    "window_tokens": 1000000,
    "used_tokens": 142500,
    "remaining_tokens": 857500,
    "quota_factor": 0.8575,
    "status": "OK",
    "cooldown_until": null
  },
  {
    "provider": "mistral",
    "window_tokens": 1000000,
    "used_tokens": 980000,
    "remaining_tokens": 20000,
    "quota_factor": 0.02,
    "status": "CRITICAL",
    "cooldown_until": null
  }
]
```

### `GET /quota/{provider}`
Returns quota tracking details for a single provider.

---

## 5. Tiers & Execution Envelopes

### `GET /tiers`
Lists configuration envelopes for all tiers (**T0** to **T5**).

| Tier | Name | Target Latency | Multi-Model Pipeline | Verification / Critic | Web / Memory RAG |
| :---: | :--- | :---: | :---: | :---: | :---: |
| **T0** | Basic | $\le 5$s | No (Single model) | No | None |
| **T1** | Light | $\le 10$s | No (Single model) | No | Simple |
| **T2** | Standard | $\le 20$s | Optional (1-2 models) | No | Vector + Rerank |
| **T3** | Advanced | $\le 45$s | Yes (Generator $\rightarrow$ Refiner) | No | Full RAG |
| **T4** | High | $\le 90$s | Yes (Multi-model) | Optional | Deep RAG |
| **T5** | Ultra | $\le 180$s | Yes (Generator $\rightarrow$ Refiner $\rightarrow$ Reviser) | Required (Critic Loop) | Deep + Web RAG |

### `PATCH /tiers/{tier}`
Updates persistent policy parameters for a tier (e.g. max latency, allowed models, multi-model enablement).

---

## 6. Routing Pins

### `GET /routing/pins`
Lists all active persistent routing pins.

### `POST /routing/pins`
Pins a tier or task type to a specific model or strategy.
```json
{
  "tier": 3,
  "task": "coding",
  "model_id": "groq/llama-3.3-70b-versatile",
  "pinned_by": "lead-engineer"
}
```

### `DELETE /routing/pins?tier=3&task=coding`
Removes an active pin (`204 No Content`).

---

## 7. Telemetry & Audit Trail

### `GET /telemetry/stats`
Returns aggregated performance metrics:
* Query parameters: `tier` (optional), `task` (optional), `strategy_id` (optional).
* Metrics returned: `total_requests`, `success_rate`, `avg_latency_ms`, `total_tokens`, `total_cost_usd`.

### `GET /telemetry/events`
Returns raw execution audit records with pagination (`limit`, `offset`).

---

## 8. Live Logs WebSocket

### `WS /logs/stream`
Connects to live system log stream. Emits structured log lines in real-time as events occur inside the engine.

---

## 9. Multi-Modal Video Ingestion

### `POST /attachments/video`
Submits a base64 video file for asynchronous background multi-modal analysis (audio transcription via Whisper + visual keyframe analysis).
```json
{
  "filename": "architecture_demo.mp4",
  "mime_type": "video/mp4",
  "data_base64": "AAAAIGZ0eXBtcDQy..."
}
```

#### Response (`200 OK`)
```json
{
  "attachment_id": "3c8e4f1a-5b2d-4819-9182-3d7f1a8e9c2b",
  "status": "processing"
}
```

### `GET /attachments/video/{attachment_id}`
Polls status and retrieves final transcript + visual summary for inclusion in `POST /execute` via `attachment_job_id`.
