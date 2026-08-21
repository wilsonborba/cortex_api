# Cortex Configuration & Environment Reference

Cortex uses a built-in settings layer in `lib/core/settings.py` for non-secret defaults.

Use `.env` only for:
- secrets and provider credentials;
- machine-specific paths or command locations;
- explicit deployment-specific overrides.

Configuration resolution follows this order:

```text
settings default
        ↓
explicit environment/.env override, if supplied
        ↓
runtime value
```

---

## 1. Defaults vs. Overrides

The application does not require a large `.env` full of repeated defaults.

Examples:
- `CORTEX_API_HOST` defaults to `0.0.0.0`.
- `CORTEX_API_PORT` defaults to `8003`.
- `CORTEX_DATABASE_URL` defaults to `sqlite:///var/cortex.db`.
- `CORTEX_OLLAMA_BASE_URL` defaults to `http://localhost:11434`.
- `CORTEX_ENVIRONMENT` defaults to `development`.

If you do not define those variables, Cortex starts with the built-in defaults above.

---

## 2. Deployment Overrides

| Environment Variable | Built-in Default | When to override |
| :--- | :--- | :--- |
| `CORTEX_API_HOST` | `0.0.0.0` | Bind to a specific interface such as `127.0.0.1`. |
| `CORTEX_API_PORT` | `8003` | Use a different port for your deployment. |
| `CORTEX_DATABASE_URL` | `sqlite:///var/cortex.db` | Point Cortex to PostgreSQL or a non-default database location. |
| `CORTEX_OLLAMA_BASE_URL` | `http://localhost:11434` | Use a remote or non-default Ollama endpoint. |
| `CORTEX_HIPPOCAMPUS_URL` | `http://localhost:8001` | Use a remote or non-default Hippocampus endpoint. |
| `CORTEX_SEARXNG_BASE_URL` | unset | Enable a self-hosted SearXNG instance. |
| `CORTEX_ENVIRONMENT` | `development` | Run under `test` or `production`. |
| `CORTEX_PROFILE` | `complete` | Keep a non-default install profile override. |

Example optional overrides:

```env
CORTEX_API_HOST=127.0.0.1
CORTEX_DATABASE_URL=postgresql+psycopg://user:pass@localhost:5432/cortex
```

---

## 3. Machine-Specific Values

These may belong in `.env` because they depend on the local machine or user account.

| Environment Variable | Built-in Default |
| :--- | :--- |
| `CORTEX_CLAUDE_CREDENTIALS_PATH` | `~/.claude/.credentials.json` |
| `CORTEX_CODEX_AUTH_PATH` | `~/.codex/auth.json` |
| `CORTEX_AGY_COMMAND` | `agy` |
| `CORTEX_AGY_DOCKER_COMMAND` | `agy-docker` |
| `CORTEX_CLAUDE_DOCKER_COMMAND` | `claude-docker` |
| `CORTEX_CODEX_COMMAND` | `codex` |

Only override them when your installation differs from the standard defaults or PATH lookup.

---

## 4. General Application Defaults

These live in `lib/core/settings.py` and are not required in `.env` for normal operation:
- quota window defaults and provider quota maps;
- cooldown thresholds and routing weights;
- discovery, retrieval, driver, and Hippocampus timeouts;
- API background-task flags and startup sync flags;
- CORS defaults, logging defaults, and executor retry limits;
- local Whisper and vision-model defaults (`CORTEX_WHISPER_*`, `CORTEX_VISION_MODEL`);
- sanitization and routing-tuning defaults such as `CORTEX_SANITIZE_PROVIDER_TEXT` and `CORTEX_ROUTING_WEIGHT_TIER_FIT`.

They may still be overridden through environment variables when needed, but Cortex does not require users to repeat them in `.env` just to run.

---

## 5. Secrets & Credentials

Keep secrets out of committed settings and only in `.env` or the process environment.

Examples:
- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- `GOOGLE_API_KEY`
- `CORTEX_GROQ_API_KEY`
- `CORTEX_GOOGLE_AI_STUDIO_API_KEY`
- `CORTEX_OPENROUTER_API_KEY`
- other provider API keys and tokens
- `CORTEX_HIPPOCAMPUS_API_KEY` when the target service requires auth

---

## 6. Runtime Bind Behavior

`CORTEX_API_HOST` is an optional override.

```text
if CORTEX_API_HOST is explicitly supplied:
    use it
else:
    use 0.0.0.0
```

The background service and standalone launcher both resolve host and port through `Settings`, so runtime behavior matches the same default-plus-override architecture used by the application itself.


## 5. Calibration Overrides

These are optional and usually do not need to be set:

| Environment Variable | Built-in Default | Purpose |
| :--- | :--- | :--- |
| `CORTEX_CALIBRATION_CANONICAL_DB_PATH` | `var/canonical_calibration.db` | Read-only distributed baseline path. |
| `CORTEX_CALIBRATION_PERSONAL_DB_PATH` | `var/personal_calibration.db` | Local writable override produced by calibration runs. |
| `CORTEX_CALIBRATION_MAX_MODELS_PER_TIER` | `6` | Cap candidate count per tier during calibration. |
| `CORTEX_CALIBRATION_JUDGE_TIMEOUT_SECONDS` | `180` | Timeout for judge CLI evaluation calls. |

Routing weights now default to a quality-dominant balance so slower but materially better models are less likely to be dominated by merely fast ones.
