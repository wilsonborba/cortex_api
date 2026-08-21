from __future__ import annotations

import os
from pathlib import Path

import pytest

from lib.core.settings import Settings, get_settings


def test_default_settings(monkeypatch):
    monkeypatch.delenv("CORTEX_DATABASE_URL", raising=False)
    monkeypatch.delenv("DATABASE_URL", raising=False)
    settings = Settings()
    assert settings.environment == "development"
    assert settings.database_url == "sqlite:///var/cortex.db"
    assert settings.ollama_base_url == "http://localhost:11434"
    assert settings.hippocampus_url == "http://localhost:8001"
    assert settings.log_level == "INFO"
    assert settings.api_host == "0.0.0.0"
    assert settings.api_port == 8003
    assert settings.sliding_window_hours == 5


def test_settings_environment_overrides(monkeypatch):
    monkeypatch.setenv("CORTEX_ENVIRONMENT", "production")
    monkeypatch.setenv("CORTEX_DATABASE_URL", "postgresql+psycopg://user:pass@localhost:5432/cortex_prod")
    monkeypatch.setenv("CORTEX_OLLAMA_BASE_URL", "http://ollama-host:11434")
    monkeypatch.setenv("CORTEX_HIPPOCAMPUS_URL", "http://hippocampus-host:8001")
    monkeypatch.setenv("CORTEX_LOG_LEVEL", "DEBUG")
    monkeypatch.setenv("CORTEX_LOG_FILE", "var/custom.log")
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-ant-test-123")
    monkeypatch.setenv("OPENAI_API_KEY", "sk-proj-test-123")
    monkeypatch.setenv("GOOGLE_API_KEY", "AIzaSyTest-123")

    settings = Settings()
    assert settings.environment == "production"
    assert settings.database_url == "postgresql+psycopg://user:pass@localhost:5432/cortex_prod"
    assert settings.ollama_base_url == "http://ollama-host:11434"
    assert settings.hippocampus_url == "http://hippocampus-host:8001"
    assert settings.log_level == "DEBUG"
    assert settings.log_file == Path("var/custom.log")
    assert settings.anthropic_api_key == "sk-ant-test-123"
    assert settings.openai_api_key == "sk-proj-test-123"
    assert settings.google_api_key == "AIzaSyTest-123"


def test_get_settings_singleton():
    s1 = get_settings()
    s2 = get_settings()
    assert s1 is s2


def test_settings_support_binary_alias_envs_and_expand_user_paths(monkeypatch):
    monkeypatch.setenv("CORTEX_AGY_BIN", "~/bin/agy")
    monkeypatch.setenv("CORTEX_CLAUDE_BIN", "~/bin/claude")
    monkeypatch.setenv("CORTEX_CODEX_BIN", "~/bin/codex")
    monkeypatch.setenv("CORTEX_CLAUDE_CREDENTIALS_PATH", "~/.claude/.credentials.json")
    monkeypatch.setenv("CORTEX_CODEX_AUTH_PATH", "~/.codex/auth.json")

    settings = Settings()

    assert settings.agy_command == str(Path("~/bin/agy").expanduser())
    assert settings.claude_command == str(Path("~/bin/claude").expanduser())
    assert settings.codex_command == str(Path("~/bin/codex").expanduser())
    assert settings.claude_credentials_path == Path("~/.claude/.credentials.json").expanduser()
    assert settings.codex_auth_path == Path("~/.codex/auth.json").expanduser()


def test_settings_parse_judge_command_map_and_disabled_lists():
    settings = Settings(
        disabled_providers="claude,codex",
        calibration_judge_commands='{"agy:work":"/opt/agy-work","claude:service":"~/bin/claude-service"}',
        calibration_disabled_judge_ids="claude,claude:docker",
    )

    assert settings.disabled_providers == ["claude", "codex"]
    assert settings.calibration_judge_commands == {
        "agy:work": "/opt/agy-work",
        "claude:service": str(Path("~/bin/claude-service").expanduser()),
    }
    assert settings.calibration_disabled_judge_ids == ["claude", "claude:docker"]
