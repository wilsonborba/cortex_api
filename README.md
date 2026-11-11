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
  * Installs core orchestration and cloud/free-tier API drivers. Omits the optional CUDA-backed local audio transcription stack and its model downloads.
* **`medium` (Balanced Workstation):**
  * Recommended for systems with $8$ to $16$ GB RAM and standard multi-core processors.
  * Includes core orchestration plus database connectors and web crawler utilities.
* **`complete` (Full Local Stack):**
  * Recommended for systems with $\ge 16$ GB RAM, NVIDIA CUDA GPU ($\ge 6$ GB VRAM), or Apple Silicon with Unified Memory.
  * Installs the CUDA-backed local audio transcription stack, developer testing suite, and advanced local models.

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
Edit `.env` only for provider credentials, machine-specific paths, or explicit deployment overrides. General defaults already live in `lib/core/settings.py`. For cloud providers, add only the keys you want to activate:
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

---

## Documentation

Comprehensive operational and technical guides are available in the [`documentation/`](documentation/) directory:

* 🚀 **[Installation & Profiles Guide](documentation/installation.md):** Hardware profiling, `Light`/`Medium`/`Complete` profiles, Linux/macOS/WSL setup, and smart port allocation.
* ⚙️ **[Configuration Reference](documentation/configuration.md):** Environment variables, database connection strings, sliding-window quotas, and scoring weights.
* 🌐 **[AI Providers Setup Guide](documentation/providers.md):** Setup instructions, API key generation links, and `.env` variables for all 14+ AI providers.
* 🛠️ **[External AI Tools & CLI Setup](documentation/external-tools.md):** Configuration for Codex CLI, Antigravity CLI, Claude Docker, local Ollama, and local Whisper.
* 🔍 **[Troubleshooting & Diagnostics](documentation/troubleshooting.md):** Solutions for installation errors, missing libraries, port conflicts, and capability degradation.

---

## Interactive Documentation & Multilingual Specs

* **Interactive Scalar Documentation:** [`http://localhost:8003/docs`](http://localhost:8003/docs) (or [`http://localhost:8003/scalar`](http://localhost:8003/scalar))
* **OpenAPI 3.1 Specification:** [`http://localhost:8003/openapi.json`](http://localhost:8003/openapi.json)

### Multilingual API References:
* 🇬🇧 **[English API Reference](documentation/scalar/api_reference_en.md)**
* 🇧🇷 **[Referência da API em Português](documentation/scalar/api_reference_pt.md)**
* 🇹🇭 **[เอกสารอ้างอิง API ภาษาไทย](documentation/scalar/api_reference_th.md)**

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
  Set `CORTEX_API_PORT` in `.env` only when overriding the built-in default `8003`, or run `./scripts/install.sh --port 8005`.
* **Checking Hardware Detection:**
  ```bash
  ./scripts/detect_hardware.sh
  # or JSON output:
  ./scripts/detect_hardware.sh --json
  ```
* **Provider Cooldowns & Rate Limits:**
  Cortex automatically clears cooldowns after the cooldown interval (default 15 minutes). You can trigger an immediate probe with `cortex models sync` or `POST /models/sync`.

## Rust Redis foundation

Cortex uses Redis directly through `dal::remote::cache::RedisCache`.
`CORTEX_REDIS_URL` is a secret in the shared .env file. Key constructors
always use cortex: for history, temporary chats, output events, hourly usage
and timing. A write requires a positive Lifetime. SET and PX run as one
command, so a written key always has an expiry. There is no permanent write
method. Values and connection secrets are not logged.

The operation timeout is in core/config.rs. A missing, down or stalled store
returns CacheError::Unavailable. Consumer issues apply the aligned fallback:
arrival order, direct stream, PostgreSQL history, or a temporary-chat error.
Run-record writes remain independent of Redis. Issue #99 stays open until
those consumers and their outage tests are complete.

The real Valkey test writes only unique fixture keys and checks the prefix
and PTTL of every written key. It removes the fixtures and checks expiry.

## Rust chat storage

The approved PostgreSQL role is cortex_api_app. It owns cortex_api and
cortex_api_test and has no superuser, createdb or createrole rights.
CORTEX_DATABASE_URL and CORTEX_TEST_DATABASE_URL are private .env keys.
The development worktree uses the canonical .env symlink.

Run `cargo run --bin migrate -- test` for the test database. Run
`cargo run --bin migrate -- cortex` for the new Cortex database. The tool
checks the database name and role. It does not import v1 data or start an app.

The six tables are chats, chat_inputs, chat_input_files, chat_outputs,
chat_output_sources and chat_output_memories. List entries use rows with
positions. There are no JSONB or array columns. Session ids are UUID v7.
Foreign keys keep each output with its input and chat. Cascading deletes
remove chat rows in one transaction after the domain cleanup succeeds.
Temporary chats have no rows here.

Repository methods check app and user scope for child writes and all reads.
The storage integration test uses a unique schema in cortex_api_test. It
checks each table, scope refusal, one output per input, UUID v7, scalar
columns and cascading delete. It reverses all migrations, runs them again,
then removes that test schema.

## Rust run records

CouchDB holds only run records, in cortex_runs. Each document id is its
output_id. The public write method accepts only RunRecord. The root and
nested serde structs reject unknown fields. They contain ids, app and user,
mode, timings, stages, model-call counts and file/web/audio counts. They have
no prompt, answer or file content, no schema_version and no registry.
Future shape changes must add only optional fields with a default.

Startup creates the database, the user/created and created indexes, and the
coded stats design document. Setup is safe to repeat. A store failure writes
a fixed warning. Credentials and records are not logged.

The stats view groups by web_mode and thinking. Its fixed numeric values are
latency, input tokens, output tokens, total tokens and success. The built-in
_stats reduce returns each metric's count, sum, min, max and sum of squares.
See the [CouchDB reduce reference](https://docs.couchdb.org/en/stable/ddocs/ddocs.html#stats).
Telemetry routes and retention are implemented in their area 12 issue.

The real document test uses cortex_runs_test. It checks startup setup,
indexes, repeat writes, typed reads, grouped statistics and fixture removal.
Primary setup creates only the database, indexes and design document.

## Rust app and user attribution

Config lists the nine slugs read from defaultdb_app in a read-only query.
Only those app ids are accepted. Unknown app ids return process number 1009.
The app comes from app_id at chat creation, then from the stored chat. Routes
without a chat use their aligned query or form app_id.

CallerContext reads users only from x-uuid and admins only from
x-access-level = admin. A required-user route rejects a missing header.
The no-user path requires a private or loopback socket peer. Forwarded
headers do not select that path. This does not add app authentication.

Caller builds the Hippo scope with workspace cortex, the checked app and an
explicit user filter. A2A uses null for Hippo, an empty SQL user and balancer
identity app:{app_id}. A user call uses its header user identity. There is
no user database sync, client secret or app proof header.

Issue #102 stays open until the chat, file, graph and persona handlers use
this layer and their integration checks pass.

## Rust usage queue

The scheduler gives one local GPU job a permit. Each call acquires a new
permit, so a long output rejoins the queue between calls. Pending users are
ordered by tokens divided by weight, with arrival order for ties. There is
no usage quota or admin exception. Missing Redis uses arrival order. A call
can wait 300 seconds; process 1010 is heavy load and 1011 is a queue fault.
Cancelled waiters cannot keep a permit.

Redis keeps 24 UTC hour buckets, the base weight snapshot and a wait-queue
snapshot. The window advances each hour. It includes the current hour and
the previous 23 hours, as required by the hour-bucket design. Each bucket
expires when its hour is 24 hours old. Every queue and snapshot key also has
a TTL. Atomic Lua updates keep token and item increments with their expiry.

On missing buckets, the ledger reads the CouchDB run records and rebuilds
the window. Calls not yet saved by the output remain in bounded local memory
for that rebuild. Saved output ids prevent double counting those calls.
A failed cache write marks the actor for repair. This adds no permanent store.
Image and audio counts are optional defaulted call fields, so older run
records still read. Supabase loyalty is blocked; runtime weight is 1.

Tests cover three users, weighted order, per-call requeue, no overlap,
Redis-down FIFO, cancelled waiters and a simulated five-minute wait. The
real Redis/CouchDB test checks full and partial cache loss, pending calls,
no double count after Save, more than 200 source records, and prefix/TTL for
every fixture key. It removes its own records and keys. Issue #67 remains
open for the Ollama driver and Save-stage integration.

## Rust local model driver

There is one ModelDriver implementation, Ollama. Config holds the approved
text and vision model ids, the MSI URL, and num_ctx = 8192. OutputModel picks
the model once for an output. Image outputs use the vision model for every
role, including roles that need no image payload.

Every model call acquires the shared queue permit. The driver checks the
loaded model and context, unloads the previous model if needed, then loads
the selected model. Model-loading events go to the progress channel. Native
Ollama NDJSON text is sent to the token channel as each complete frame arrives.
The parser handles split UTF-8 data and multiple frames in a network chunk.
It sends no OpenAI routes, response-format request or other provider call.

Completed calls use Ollama token and timing counts. A cancelled or broken
stream can lack final counts; its partial usage uses the shared conservative
UTF-8-byte estimate in Config. The future input guard uses that same rule.
The text is not logged. Cancellation is control flow, not a numbered fault.
Model failures use process 1012 and release the queue. Local transcription
can unload models while holding the same permit.

The local HTTP double tests cover text and vision, switching, one model per
output, immediate tokens, cancellation, failure and queue release. The ignored
ollama_msi test measures cold/warm loads and token rate through the real queue.
Run it only in an owner-approved MSI GPU window. Its stores and keys are tests.
The measurement report stays in ignored docs/ollama-8192-night.md.

## Rust input limit and models route

GET /models returns one active T0 entry, max_input_tokens and estimate_rule.
The input estimate is UTF-8 bytes divided by bytes_per_token, rounded up,
then multiplied by copies. The
current rule uses 1 byte per estimated token and two question copies, as
required by area 6.8. This conservative rule works
with the same UTF-8 encoding on Flutter web and the API.

Config computes the limit once from 8192 minus the 1024-token system and
chat-template allowance, 1024 answer tokens and 1024 thinking-note tokens.
The system allowance is at least the configured instruction estimate plus
the chat-template reserve.
The same safe limit applies with thinking on or off. Only user text enters
the guard. Inputs above it return error_number 1201. Extra files, memories
and web text are trimmed by Assemble; they do not reject a valid input.

Route and guard tests cover the exact boundary, over-limit text, Thai,
emoji, one T0 model and matching published values. Issue #71 stays open
until the chat Accept stage calls this guard.

## Rust usage route

GET /telemetry/usage requires x-uuid and returns only that user's 24-hour
buckets. Query user ids and admin headers cannot select another user. The
response has window_hours, input/output/total tokens, image/audio items,
loyalty_weight and its parts, and the formula. While #68 is blocked, the
weight is 1 and age, frequency, account_days and sign_in_days_90 are null.
The frontend can show Not available yet. Missing x-uuid uses process 1005;
unavailable usage uses process 1013.

The route is registered in the development API. Its run-record source is
cortex_runs_test until cutover, so development checks do not write primary
run records. Tests cover two users, attempted query/admin overrides, missing
identity, a failed ledger and values read from real Valkey buckets.

### Rust v2 input security

The v2 shield uses normalized Rust rules and the ProtectAI DeBERTa prompt
injection classifier on CPU.
There is no skip flag. Missing classifier files reject the security check with
process 1015. A blocked input returns process 1014 and the aligned block message.
File and web checks drop failed chunks. The persona check scans saved text fields.
Chat, source chunks and persona routes use the same shield.

The model, tokenizer and CPU runtime paths and threshold are in `src/core/config.rs`.
The ONNX export must expose int64 `input_ids`, `attention_mask` and optionally
`token_type_ids`, plus float32 `logits` with shape `[1, 2]`, ordered SAFE,
INJECTION. Each window has at most 512 tokens, with 64 tokens of overlap.
The adapter adds the tokenizer's special tokens to each window. Runtime 1.24 is
loaded from the configured local path. Model files and runtime files stay in the
ignored `models/` directory.

The model is [protectai/deberta-v3-base-prompt-injection-v2](https://huggingface.co/protectai/deberta-v3-base-prompt-injection-v2),
under Apache-2.0. The publisher ONNX export and tokenizer are used directly.
The publisher supports English prompt injection only and has archived the project.
It does not claim non-English or jailbreak detection. The deterministic rules still
cover English, Portuguese and Thai. Synthetic smoke tests report scores and misses;
they do not establish detection quality for all attacks.

Run `cargo test --test security measure_real_classifier_cpu -- --ignored`
with the real files installed. Reports go to ignored
`docs/injection-classifier-cpu.md` and `.json`. The local model manifest records
the pinned publisher revision and SHA256 hashes. There is no gated download.
The tiny ONNX fixture in `tests/fixtures/` is only a transport test double.
It is not a replacement classifier.

Rule sources: [Vigil](https://github.com/deadbits/vigil-llm/tree/main/data/yara)
and the [ProtectAI model card](https://huggingface.co/protectai/deberta-v3-base-prompt-injection-v2).
The English Vigil patterns were adapted. The Portuguese and Thai patterns were
added for the aligned languages. The Vigil license is in `LICENSES/`.

### Rust v2 context formats

Plain labelled text is the default selected by the real MSI context experiment
in issue #85. JSON and TOON remain available in `Config::context_format`.
Only the context sent to the model uses this setting. Service bodies stay JSON.
The ignored `context_format_benchmark` runner uses the real chat router, CPU
shield and MSI Ollama with fixed synthetic retrieval fixtures. Run it only in
an owner-approved GPU window. Raw trials and the report stay in ignored docs.
