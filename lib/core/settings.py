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
        default="127.0.0.1",
        validation_alias=AliasChoices("CORTEX_API_HOST", "API_HOST"),
    )
    api_port: int = Field(
        default=8000,
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


@lru_cache(maxsize=1)
def get_settings() -> Settings:
    return Settings()
