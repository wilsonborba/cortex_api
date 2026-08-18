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
