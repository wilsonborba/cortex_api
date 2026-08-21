from __future__ import annotations

from functools import lru_cache
from pathlib import Path
from typing import Optional

from pydantic import AliasChoices, Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    # Core environment & databases
    environment: str = Field(
        default="development",
        validation_alias=AliasChoices("CORTEX_ENVIRONMENT", "ENVIRONMENT", "ENV"),
    )
    profile: str = Field(
        default="complete",
        validation_alias=AliasChoices("CORTEX_PROFILE", "PROFILE"),
    )
    database_url: str = Field(
        default="sqlite:///var/cortex.db",
        validation_alias=AliasChoices("CORTEX_DATABASE_URL", "DATABASE_URL"),
    )

    # Service endpoints
    ollama_base_url: str = Field(
        default="http://localhost:11434",
        validation_alias=AliasChoices("CORTEX_OLLAMA_BASE_URL", "OLLAMA_BASE_URL"),
    )
    hippocampus_url: str = Field(
        default="http://localhost:8001",
        validation_alias=AliasChoices("CORTEX_HIPPOCAMPUS_URL", "HIPPOCAMPUS_URL"),
    )

    # Logging
    log_level: str = Field(
        default="INFO",
        validation_alias=AliasChoices("CORTEX_LOG_LEVEL", "LOG_LEVEL"),
    )
    log_file: Path = Field(
        default=Path("var/cortex.log"),
        validation_alias=AliasChoices("CORTEX_LOG_FILE", "LOG_FILE"),
    )

    # Server configuration
    api_host: str = Field(
        default="0.0.0.0",
        validation_alias=AliasChoices("CORTEX_API_HOST", "API_HOST"),
    )
    api_port: int = Field(
        default=8003,
        validation_alias=AliasChoices("CORTEX_API_PORT", "API_PORT"),
    )

    # Engine tuning & quota budget
    sliding_window_hours: int = Field(
        default=5,
        validation_alias=AliasChoices("CORTEX_SLIDING_WINDOW_HOURS", "SLIDING_WINDOW_HOURS"),
    )

    # Model Registry: provider discovery
    discovery_timeout_seconds: float = Field(
        default=10.0,
        validation_alias=AliasChoices("CORTEX_DISCOVERY_TIMEOUT_SECONDS", "DISCOVERY_TIMEOUT_SECONDS"),
    )
    agy_command: str = Field(
        default="agy",
        validation_alias=AliasChoices("CORTEX_AGY_COMMAND", "AGY_COMMAND"),
    )
    claude_credentials_path: Path = Field(
        default_factory=lambda: Path.home() / ".claude" / ".credentials.json",
        validation_alias=AliasChoices("CORTEX_CLAUDE_CREDENTIALS_PATH", "CLAUDE_CREDENTIALS_PATH"),
    )
    codex_auth_path: Path = Field(
        default_factory=lambda: Path.home() / ".codex" / "auth.json",
        validation_alias=AliasChoices("CORTEX_CODEX_AUTH_PATH", "CODEX_AUTH_PATH"),
    )
    claude_docker_command: str = Field(
        default="claude-docker",
        validation_alias=AliasChoices("CORTEX_CLAUDE_DOCKER_COMMAND", "CLAUDE_DOCKER_COMMAND"),
    )
    agy_docker_command: str = Field(
        default="agy-docker",
        validation_alias=AliasChoices("CORTEX_AGY_DOCKER_COMMAND", "AGY_DOCKER_COMMAND"),
    )
    codex_command: str = Field(
        default="codex",
        validation_alias=AliasChoices("CORTEX_CODEX_COMMAND", "CODEX_COMMAND"),
    )
    driver_timeout_seconds: float = Field(
        default=180.0,
        validation_alias=AliasChoices("CORTEX_DRIVER_TIMEOUT_SECONDS", "DRIVER_TIMEOUT_SECONDS"),
    )

    # Quota Tracker: sliding window token budget
    # `ollama` is intentionally excluded from these: it's local/free, so its
    # quota factor is always 1.0 rather than measured against a ceiling.
    default_quota_window_tokens: int = Field(
        default=1_000_000,
        validation_alias=AliasChoices(
            "CORTEX_DEFAULT_QUOTA_WINDOW_TOKENS", "DEFAULT_QUOTA_WINDOW_TOKENS"
        ),
    )
    quota_window_tokens_by_provider: dict[str, int] = Field(
        default_factory=dict,
        validation_alias=AliasChoices(
            "CORTEX_QUOTA_WINDOW_TOKENS_BY_PROVIDER", "QUOTA_WINDOW_TOKENS_BY_PROVIDER"
        ),
    )
    cooldown_minutes: int = Field(
        default=15,
        validation_alias=AliasChoices("CORTEX_COOLDOWN_MINUTES", "COOLDOWN_MINUTES"),
    )

    # Web Retrieval: search + scraping
    web_search_max_results: int = Field(
        default=5,
        validation_alias=AliasChoices("CORTEX_WEB_SEARCH_MAX_RESULTS", "WEB_SEARCH_MAX_RESULTS"),
    )
    web_retrieval_timeout_seconds: float = Field(
        default=15.0,
        validation_alias=AliasChoices(
            "CORTEX_WEB_RETRIEVAL_TIMEOUT_SECONDS", "WEB_RETRIEVAL_TIMEOUT_SECONDS"
        ),
    )
    searxng_base_url: Optional[str] = Field(
        default=None,
        validation_alias=AliasChoices("CORTEX_SEARXNG_BASE_URL", "SEARXNG_BASE_URL"),
    )

    # Hippocampus: external memory/document service client
    hippocampus_timeout_seconds: float = Field(
        default=10.0,
        validation_alias=AliasChoices(
            "CORTEX_HIPPOCAMPUS_TIMEOUT_SECONDS", "HIPPOCAMPUS_TIMEOUT_SECONDS"
        ),
    )
    hippocampus_api_key: Optional[str] = Field(
        default=None,
        validation_alias=AliasChoices("CORTEX_HIPPOCAMPUS_API_KEY", "HIPPOCAMPUS_API_KEY"),
    )

    # Routing Engine: dynamic scoring weights + quota cutoff
    # Score(M) = w_cap*Cap(M,task) + w_q*Q(M) - w_lat*LatencyNorm(M) - w_cost*CostNorm(M)
    quota_critical_threshold: float = Field(
        default=0.15,
        validation_alias=AliasChoices("CORTEX_QUOTA_CRITICAL_THRESHOLD", "QUOTA_CRITICAL_THRESHOLD"),
    )
    routing_weight_capability: float = Field(
        default=0.5,
        validation_alias=AliasChoices("CORTEX_ROUTING_WEIGHT_CAPABILITY", "ROUTING_WEIGHT_CAPABILITY"),
    )
    routing_weight_quota: float = Field(
        default=0.25,
        validation_alias=AliasChoices("CORTEX_ROUTING_WEIGHT_QUOTA", "ROUTING_WEIGHT_QUOTA"),
    )
    routing_weight_latency: float = Field(
        default=0.15,
        validation_alias=AliasChoices("CORTEX_ROUTING_WEIGHT_LATENCY", "ROUTING_WEIGHT_LATENCY"),
    )
    routing_weight_cost: float = Field(
        default=0.10,
        validation_alias=AliasChoices("CORTEX_ROUTING_WEIGHT_COST", "ROUTING_WEIGHT_COST"),
    )
    routing_weight_tier_fit: float = Field(
        default=0.20,
        validation_alias=AliasChoices("CORTEX_ROUTING_WEIGHT_TIER_FIT", "ROUTING_WEIGHT_TIER_FIT"),
    )
    routing_cost_ceiling_usd_per_million: float = Field(
        default=20.0,
        validation_alias=AliasChoices(
            "CORTEX_ROUTING_COST_CEILING_USD_PER_MILLION", "ROUTING_COST_CEILING_USD_PER_MILLION"
        ),
    )

    # Execution Engine
    executor_max_retries: int = Field(
        default=1,
        validation_alias=AliasChoices("CORTEX_EXECUTOR_MAX_RETRIES", "EXECUTOR_MAX_RETRIES"),
    )
    executor_max_reroutes: int = Field(
        default=1,
        validation_alias=AliasChoices("CORTEX_EXECUTOR_MAX_REROUTES", "EXECUTOR_MAX_REROUTES"),
    )
    executor_max_critic_revisions: int = Field(
        default=1,
        validation_alias=AliasChoices(
            "CORTEX_EXECUTOR_MAX_CRITIC_REVISIONS", "EXECUTOR_MAX_CRITIC_REVISIONS"
        ),
    )
    sanitize_provider_text: bool = Field(
        default=True,
        validation_alias=AliasChoices("CORTEX_SANITIZE_PROVIDER_TEXT", "SANITIZE_PROVIDER_TEXT"),
    )

    # Attachments: local audio transcription + local vision model
    whisper_local_enabled: bool = Field(
        default=True,
        validation_alias=AliasChoices("CORTEX_WHISPER_LOCAL_ENABLED", "WHISPER_LOCAL_ENABLED"),
    )
    whisper_local_model: str = Field(
        default="large-v3-turbo-q5_0",
        validation_alias=AliasChoices("CORTEX_WHISPER_LOCAL_MODEL", "WHISPER_LOCAL_MODEL"),
    )
    whisper_models_dir: str = Field(
        default="var/whisper-models",
        validation_alias=AliasChoices("CORTEX_WHISPER_MODELS_DIR", "WHISPER_MODELS_DIR"),
    )
    # Resolved once by the setup process (ollama pull qwen2.5vl:7b, falling
    # back to llava:7b) -- never two vision models installed/used at once.
    vision_model: str = Field(
        default="qwen2.5vl:7b",
        validation_alias=AliasChoices("CORTEX_VISION_MODEL", "VISION_MODEL"),
    )

    # REST API
    cors_allow_origins: list[str] = Field(
        default_factory=lambda: ["*"],
        validation_alias=AliasChoices("CORTEX_CORS_ALLOW_ORIGINS", "CORS_ALLOW_ORIGINS"),
    )
    api_sync_models_on_startup: bool = Field(
        default=True,
        validation_alias=AliasChoices("CORTEX_API_SYNC_MODELS_ON_STARTUP", "API_SYNC_MODELS_ON_STARTUP"),
    )
    api_background_tasks_enabled: bool = Field(
        default=True,
        validation_alias=AliasChoices("CORTEX_API_BACKGROUND_TASKS_ENABLED", "API_BACKGROUND_TASKS_ENABLED"),
    )
    api_cooldown_refresh_interval_seconds: float = Field(
        default=60.0,
        validation_alias=AliasChoices(
            "CORTEX_API_COOLDOWN_REFRESH_INTERVAL_SECONDS", "API_COOLDOWN_REFRESH_INTERVAL_SECONDS"
        ),
    )

    # Sensitive API Secrets (only read from .env / environment)
    anthropic_api_key: Optional[str] = Field(
        default=None,
        validation_alias=AliasChoices("ANTHROPIC_API_KEY", "CLAUDE_API_KEY"),
    )
    openai_api_key: Optional[str] = Field(
        default=None,
        validation_alias=AliasChoices("OPENAI_API_KEY"),
    )
    google_api_key: Optional[str] = Field(
        default=None,
        validation_alias=AliasChoices("GOOGLE_API_KEY", "GEMINI_API_KEY"),
    )

    # Free-tier providers (docs/free-tier-adapters-plan.md): each key is its
    # own `CORTEX_<PROVIDER>_API_KEY`, deliberately separate from the "API
    # Secrets" section above (those use the raw names official SDKs expect;
    # these are cortex-specific driver credentials, including
    # `CORTEX_GOOGLE_AI_STUDIO_API_KEY`, which is NOT the same thing as
    # `google_api_key` above).
    groq_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_GROQ_API_KEY")
    )
    google_ai_studio_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_GOOGLE_AI_STUDIO_API_KEY")
    )
    openrouter_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_OPENROUTER_API_KEY")
    )
    cloudflare_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_CLOUDFLARE_API_KEY")
    )
    cloudflare_account_id: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_CLOUDFLARE_ACCOUNT_ID")
    )
    cohere_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_COHERE_API_KEY")
    )
    mistral_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_MISTRAL_API_KEY")
    )
    nvidia_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_NVIDIA_API_KEY")
    )
    zai_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_ZAI_API_KEY")
    )
    requesty_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_REQUESTY_API_KEY")
    )
    huggingface_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_HUGGINGFACE_API_KEY")
    )
    ollama_cloud_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_OLLAMA_CLOUD_API_KEY")
    )
    aion_labs_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_AION_LABS_API_KEY")
    )
    siliconflow_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_SILICONFLOW_API_KEY")
    )
    inference_net_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_INFERENCE_NET_API_KEY")
    )
    sambanova_api_key: Optional[str] = Field(
        default=None, validation_alias=AliasChoices("CORTEX_SAMBANOVA_API_KEY")
    )


@lru_cache(maxsize=1)
def get_settings() -> Settings:
    return Settings()
