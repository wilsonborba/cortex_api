# Cortex Configuration & Environment Reference

This document provides a comprehensive reference for all configuration options, environment variables, and tuning parameters supported by Cortex.

Configuration is loaded from the environment or from a local `.env` file created in the project root.

---

## 1. Core Environment & Database Settings

| Environment Variable | Default | Description |
| :--- | :--- | :--- |
| `CORTEX_ENVIRONMENT` | `development` | Runtime mode: `development`, `test`, or `production`. |
| `CORTEX_PROFILE` | `complete` | Installed capability profile: `light`, `medium`, or `complete`. |
| `CORTEX_DATABASE_URL` | `sqlite:///var/cortex.db` | SQLAlchemy database connection URI (SQLite default or PostgreSQL). |
| `CORTEX_LOG_LEVEL` | `INFO` | Logging verbosity: `DEBUG`, `INFO`, `WARNING`, `ERROR`. |
| `CORTEX_LOG_FILE` | `var/cortex.log` | File path for persistent structured log records. |

---

## 2. Server & Network Bindings

| Environment Variable | Default | Description |
| :--- | :--- | :--- |
| `CORTEX_API_HOST` | `0.0.0.0` | Bind IP address for the FastAPI / Uvicorn server (`0.0.0.0` for all interfaces). |
| `CORTEX_API_PORT` | `8003` | Listening TCP port for the Cortex API and Scalar docs. |
| `CORTEX_CORS_ALLOW_ORIGINS` | `["*"]` | Allowed origins list for CORS headers (JSON array). |

---

## 3. Quota Tracking & Sliding Window Tuning

| Environment Variable | Default | Description |
| :--- | :--- | :--- |
| `CORTEX_SLIDING_WINDOW_HOURS` | `5` | Duration in hours of the token sliding window. |
| `CORTEX_DEFAULT_QUOTA_WINDOW_TOKENS` | `1000000` | Default token ceiling per sliding window for unconfigured cloud providers. |
| `CORTEX_COOLDOWN_MINUTES` | `15` | Duration a provider stays cooling down after encountering an HTTP 429 rate limit. |
| `CORTEX_QUOTA_CRITICAL_THRESHOLD` | `0.15` | Quota factor threshold (15%) below which the router considers a provider quota-critical. |

---

## 4. Routing Engine Scoring Weights

The dynamic scoring formula determines model selection:
$$\text{Score}(M) = w_{\text{cap}} \cdot \text{Cap}(M) + w_q \cdot Q(M) - w_{\text{lat}} \cdot \text{LatencyNorm}(M) - w_{\text{cost}} \cdot \text{CostNorm}(M)$$

| Environment Variable | Default | Description |
| :--- | :--- | :--- |
| `CORTEX_ROUTING_WEIGHT_CAPABILITY` | `0.50` | Weight for model capability match to the task. |
| `CORTEX_ROUTING_WEIGHT_QUOTA` | `0.25` | Weight favoring providers with healthy remaining token quota. |
| `CORTEX_ROUTING_WEIGHT_LATENCY` | `0.15` | Weight penalizing slow or high-latency models. |
| `CORTEX_ROUTING_WEIGHT_COST` | `0.10` | Weight penalizing expensive paid endpoints. |
| `CORTEX_ROUTING_COST_CEILING_USD_PER_MILLION` | `20.0` | Normalization ceiling for token cost calculations. |

---

## 5. Execution Pipeline & Retry Limits

| Environment Variable | Default | Description |
| :--- | :--- | :--- |
| `CORTEX_EXECUTOR_MAX_RETRIES` | `1` | Number of immediate retries on transient connection hiccups. |
| `CORTEX_EXECUTOR_MAX_REROUTES` | `1` | Number of times the executor can reroute to an alternate model on a 429 rate limit. |
| `CORTEX_EXECUTOR_MAX_CRITIC_REVISIONS` | `1` | Maximum iterative revision loops when the Critic step returns `NEEDS_REVISION`. |
| `CORTEX_SANITIZE_PROVIDER_TEXT` | `true` | When true, strips provider-specific boilerplates and artifact noise. |

---

## 6. External Service Integration Endpoints

| Environment Variable | Default | Description |
| :--- | :--- | :--- |
| `CORTEX_OLLAMA_BASE_URL` | `http://localhost:11434` | Base URL for local or network Ollama daemon. |
| `CORTEX_HIPPOCAMPUS_URL` | `http://localhost:8001` | Base URL for Hippocampus vector memory service. |
| `CORTEX_SEARXNG_BASE_URL` | `None` | Optional self-hosted SearXNG search engine base URL. |
| `CORTEX_DISCOVERY_TIMEOUT_SECONDS` | `10.0` | Network timeout for probing provider model catalogs. |
| `CORTEX_DRIVER_TIMEOUT_SECONDS` | `180.0` | Execution timeout for individual model prompt completions. |
