from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any, Optional

import httpx
import pytest

from lib.dal.models import AccessStatus
from lib.engine.discovery.antigravity import AntigravityDiscovery
from lib.engine.discovery.base import ProviderDiscoveryError
from lib.engine.discovery.claude_docker import ClaudeDockerDiscovery
from lib.engine.discovery.codex import CodexDiscovery
from lib.engine.discovery.ollama import OllamaDiscovery


# --- OllamaDiscovery ---------------------------------------------------------


class _FakeResponse:
    def __init__(self, payload: Any, status_code: int = 200) -> None:
        self._payload = payload
        self.status_code = status_code

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            request = httpx.Request("GET", "http://fake")
            raise httpx.HTTPStatusError("error", request=request, response=None)

    def json(self) -> Any:
        return self._payload


class _FakeOllamaClient:
    def __init__(self, tags_payload: Any, show_payload: Optional[dict] = None) -> None:
        self._tags_payload = tags_payload
        self._show_payload = show_payload or {}

    def __enter__(self) -> "_FakeOllamaClient":
        return self

    def __exit__(self, *exc_info: Any) -> None:
        return None

    def get(self, url: str) -> _FakeResponse:
        return _FakeResponse(self._tags_payload)

    def post(self, url: str, json: dict) -> _FakeResponse:  # noqa: A002 - matches httpx signature
        name = json["name"]
        return _FakeResponse(self._show_payload.get(name, {}))


def test_ollama_discovery_parses_tags_and_context_window():
    tags_payload = {
        "models": [
            {"name": "dolphin3:8b", "details": {"parameter_size": "8B"}},
            {"name": "qwen3:8b", "details": {"parameter_size": "8B"}},
        ]
    }
    show_payload = {"dolphin3:8b": {"model_info": {"llama.context_length": 131072}}}
    client = _FakeOllamaClient(tags_payload, show_payload)
    discovery = OllamaDiscovery(base_url="http://localhost:11434", client_factory=lambda: client)

    models = discovery.discover()

    assert [m.id for m in models] == ["ollama/dolphin3:8b", "ollama/qwen3:8b"]
    assert models[0].context_window == 131072
    assert models[1].context_window == 8192  # no /api/show data -> conservative default
    assert all(m.access_status == AccessStatus.AVAILABLE.value for m in models)
    assert all(m.is_local for m in models)
    assert models[0].tier_eligibility == [0, 1, 2, 3]
    assert models[0].capabilities["general"] >= 0.6


def test_ollama_large_model_can_reach_higher_tiers():
    tags_payload = {"models": [{"name": "deepseek-r1:32b", "details": {"parameter_size": "32B"}}]}
    client = _FakeOllamaClient(tags_payload)
    discovery = OllamaDiscovery(base_url="http://localhost:11434", client_factory=lambda: client)

    models = discovery.discover()

    assert models[0].tier_eligibility == [2, 3, 4, 5]
    assert models[0].capabilities["reasoning"] >= 0.8


def test_ollama_discovery_raises_when_unreachable():
    def _broken_client_factory() -> Any:
        raise httpx.ConnectError("connection refused")

    discovery = OllamaDiscovery(base_url="http://localhost:11434", client_factory=_broken_client_factory)

    with pytest.raises(ProviderDiscoveryError) as exc_info:
        discovery.discover()
    assert exc_info.value.provider == "ollama"


# --- AntigravityDiscovery ----------------------------------------------------


def _completed(stdout: str = "", stderr: str = "", returncode: int = 0) -> subprocess.CompletedProcess:
    return subprocess.CompletedProcess(args=["agy", "models"], returncode=returncode, stdout=stdout, stderr=stderr)


def test_antigravity_discovery_parses_plain_model_list():
    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout="gemini-3.7-flash-high\ngemini-3.1-pro-high\n")

    discovery = AntigravityDiscovery(runner=runner)
    models = discovery.discover()

    assert [m.id for m in models] == ["agy/gemini-3.7-flash-high", "agy/gemini-3.1-pro-high"]
    assert all(m.access_status == AccessStatus.AVAILABLE.value for m in models)
    assert all(not m.is_local for m in models)


def test_antigravity_discovery_detects_sign_in_required():
    # Matches the real CLI's actual output when not authenticated.
    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(
            stdout="Fetching available models...\n",
            stderr="Error: Please sign in to view available models. Launch the CLI without arguments to sign in.\n",
            returncode=1,
        )

    discovery = AntigravityDiscovery(runner=runner)

    with pytest.raises(ProviderDiscoveryError) as exc_info:
        discovery.discover()
    assert exc_info.value.provider == "agy"


def test_antigravity_discovery_raises_when_binary_missing():
    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        raise FileNotFoundError("agy not found")

    discovery = AntigravityDiscovery(runner=runner)

    with pytest.raises(ProviderDiscoveryError):
        discovery.discover()


def test_antigravity_discovery_uses_configured_binary_path():
    seen = {}

    def runner(args, **kwargs):
        seen["argv0"] = args[0]
        return _completed(stdout="gemini-3.7-flash-high\n")

    discovery = AntigravityDiscovery(command="/opt/tools/agy", runner=runner)
    discovery.discover()

    assert seen["argv0"] == "/opt/tools/agy"


# --- ClaudeDockerDiscovery ----------------------------------------------------


def test_claude_docker_discovery_available_with_credentials(tmp_path: Path):
    creds = tmp_path / ".credentials.json"
    creds.write_text(json.dumps({"account": "wilsonborba"}), encoding="utf-8")

    models = ClaudeDockerDiscovery(credentials_path=creds).discover()

    assert len(models) >= 1
    assert all(m.access_status == AccessStatus.AVAILABLE.value for m in models)
    assert all(m.provider == "claude" for m in models)


def test_claude_docker_discovery_offline_without_credentials(tmp_path: Path):
    missing = tmp_path / "does-not-exist.json"

    models = ClaudeDockerDiscovery(credentials_path=missing).discover()

    assert len(models) >= 1
    assert all(m.access_status == AccessStatus.OFFLINE.value for m in models)


# --- CodexDiscovery ------------------------------------------------------------


def test_codex_discovery_requires_subscription_for_chatgpt_auth(tmp_path: Path):
    auth = tmp_path / "auth.json"
    auth.write_text(json.dumps({"auth_mode": "chatgpt"}), encoding="utf-8")

    models = CodexDiscovery(auth_path=auth).discover()

    assert len(models) >= 1
    assert all(m.access_status == AccessStatus.REQUIRES_SUBSCRIPTION.value for m in models)


def test_codex_discovery_available_for_apikey_auth(tmp_path: Path):
    auth = tmp_path / "auth.json"
    auth.write_text(json.dumps({"auth_mode": "apikey"}), encoding="utf-8")

    models = CodexDiscovery(auth_path=auth).discover()

    assert all(m.access_status == AccessStatus.AVAILABLE.value for m in models)


def test_codex_discovery_offline_without_auth_file(tmp_path: Path):
    missing = tmp_path / "does-not-exist.json"

    models = CodexDiscovery(auth_path=missing).discover()

    assert all(m.access_status == AccessStatus.OFFLINE.value for m in models)
