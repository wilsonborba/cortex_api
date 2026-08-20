from __future__ import annotations

import json
import subprocess
from typing import Any

import httpx
import pytest

from lib.engine.drivers.agy_docker import AgyDockerDriver
from lib.engine.drivers.claude_docker import ClaudeDockerDriver
from lib.engine.drivers.codex import CodexDriver
from lib.engine.drivers.ollama import OllamaDriver
from lib.engine.drivers.openai_compatible import OpenAICompatibleDriver


def _completed(stdout: str = "", stderr: str = "", returncode: int = 0) -> subprocess.CompletedProcess:
    return subprocess.CompletedProcess(args=["cmd"], returncode=returncode, stdout=stdout, stderr=stderr)


# --- OllamaDriver -------------------------------------------------------------


class _FakeResponse:
    def __init__(self, payload: Any, status_code: int = 200) -> None:
        self._payload = payload
        self.status_code = status_code

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            request = httpx.Request("POST", "http://fake")
            response = httpx.Response(self.status_code, request=request)
            raise httpx.HTTPStatusError("error", request=request, response=response)

    def json(self) -> Any:
        return self._payload


class _FakeOllamaClient:
    def __init__(self, payload: Any, status_code: int = 200) -> None:
        self._payload = payload
        self._status_code = status_code

    def __enter__(self) -> "_FakeOllamaClient":
        return self

    def __exit__(self, *exc_info: Any) -> None:
        return None

    def post(self, url: str, json: dict) -> _FakeResponse:  # noqa: A002
        return _FakeResponse(self._payload, self._status_code)


def test_ollama_driver_parses_real_generate_payload():
    # Exact payload shape from docs/quota-and-token-tracking.md 2.1.
    payload = {
        "model": "dolphin3:8b",
        "response": "hi",
        "prompt_eval_count": 39,
        "eval_count": 2,
        "total_duration": 11691741700,
    }
    driver = OllamaDriver(base_url="http://localhost:11434", client_factory=lambda: _FakeOllamaClient(payload))

    result = driver.run("dolphin3:8b", "hello")

    assert result.success is True
    assert result.input_tokens == 39
    assert result.output_tokens == 2
    assert result.latency_ms == 11691  # total_duration (ns) / 1_000_000


class _CapturingClient:
    """Records the JSON body of the last POST and replies with `payload`."""

    def __init__(self, payload: Any, status_code: int = 200) -> None:
        self._payload = payload
        self._status_code = status_code
        self.last_json: Any = None

    def __enter__(self) -> "_CapturingClient":
        return self

    def __exit__(self, *exc_info: Any) -> None:
        return None

    def post(self, url: str, json: dict | None = None, headers: dict | None = None, **kwargs: Any) -> _FakeResponse:  # noqa: A002
        self.last_json = json
        return _FakeResponse(self._payload, self._status_code)


def test_ollama_driver_includes_images_field_when_given():
    client = _CapturingClient({"response": "ok"})
    driver = OllamaDriver(base_url="http://localhost:11434", client_factory=lambda: client)

    driver.run("qwen2.5vl:7b", "describe this", images=["data:image/png;base64,AAAA"])

    assert client.last_json["images"] == ["AAAA"]  # data: prefix stripped for Ollama's native field


def test_ollama_driver_omits_images_field_when_none_given():
    client = _CapturingClient({"response": "ok"})
    driver = OllamaDriver(base_url="http://localhost:11434", client_factory=lambda: client)

    driver.run("dolphin3:8b", "hello")

    assert "images" not in client.last_json


def test_openai_compatible_driver_sends_image_url_content_array():
    client = _CapturingClient({"choices": [{"message": {"content": "a red pixel"}}], "usage": {}})
    driver = OpenAICompatibleDriver(
        provider="test", base_url="http://fake", api_key="key", client_factory=lambda: client
    )

    driver.run("some-vision-model", "what is this?", images=["data:image/png;base64,AAAA"])

    content = client.last_json["messages"][0]["content"]
    assert content[0] == {"type": "text", "text": "what is this?"}
    assert content[1] == {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}


def test_openai_compatible_driver_sends_plain_string_content_without_images():
    client = _CapturingClient({"choices": [{"message": {"content": "hi"}}], "usage": {}})
    driver = OpenAICompatibleDriver(
        provider="test", base_url="http://fake", api_key="key", client_factory=lambda: client
    )

    driver.run("some-model", "hello")

    assert client.last_json["messages"][0]["content"] == "hello"


def test_ollama_driver_maps_429_to_rate_limit():
    driver = OllamaDriver(
        base_url="http://localhost:11434",
        client_factory=lambda: _FakeOllamaClient({"error": "rate limited"}, status_code=429),
    )

    result = driver.run("dolphin3:8b", "hello")

    assert result.success is False
    assert result.error_type == "rate_limit"


def test_ollama_driver_reports_unreachable_on_connection_error():
    def _broken_factory() -> Any:
        raise httpx.ConnectError("connection refused")

    driver = OllamaDriver(base_url="http://localhost:11434", client_factory=_broken_factory)

    result = driver.run("dolphin3:8b", "hello")

    assert result.success is False
    assert result.error_type == "unreachable"


# --- ClaudeDockerDriver -------------------------------------------------------


def test_claude_docker_driver_parses_real_json_payload():
    # Exact payload shape from docs/quota-and-token-tracking.md 2.2.
    payload = {
        "result": "hi",
        "duration_ms": 1683,
        "total_cost_usd": 0.035571,
        "usage": {
            "input_tokens": 2,
            "output_tokens": 3,
            "cache_read_input_tokens": 8170,
            "cache_creation_input_tokens": 5411,
        },
        "subtype": "success",
        "is_error": False,
    }

    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout=json.dumps(payload))

    driver = ClaudeDockerDriver(runner=runner)
    result = driver.run("claude-sonnet-5", "hello")

    assert result.success is True
    assert result.input_tokens == 2
    assert result.output_tokens == 3
    assert result.latency_ms == 1683
    assert result.cost_usd == pytest.approx(0.035571)


def test_claude_docker_driver_detects_is_error_rate_limit():
    payload = {"is_error": True, "subtype": "error_rate_limit", "result": ""}

    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout=json.dumps(payload), returncode=1)

    driver = ClaudeDockerDriver(runner=runner)
    result = driver.run("claude-sonnet-5", "hello")

    assert result.success is False
    assert result.error_type == "rate_limit"


def test_claude_docker_driver_handles_non_json_output():
    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout="", stderr="command not found", returncode=127)

    driver = ClaudeDockerDriver(runner=runner)
    result = driver.run("claude-sonnet-5", "hello")

    assert result.success is False
    assert result.error_type == "cli_error"


# --- AgyDockerDriver -----------------------------------------------------------


def test_agy_docker_driver_parses_real_json_payload():
    # Exact payload shape from docs/quota-and-token-tracking.md 2.3.
    payload = {
        "response": "hi",
        "status": "SUCCESS",
        "duration_seconds": 2.94,
        "usage": {"input_tokens": 15155, "output_tokens": 81, "thinking_tokens": 71, "total_tokens": 15236},
    }

    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout=json.dumps(payload))

    driver = AgyDockerDriver(runner=runner)
    result = driver.run("gemini-3.1-pro-high", "hello")

    assert result.success is True
    assert result.input_tokens == 15155
    assert result.output_tokens == 81
    assert result.latency_ms == 2940


def test_agy_docker_driver_non_success_status_is_failure():
    payload = {"response": "", "status": "RATE_LIMITED", "duration_seconds": 0.1}

    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout=json.dumps(payload), returncode=1)

    driver = AgyDockerDriver(runner=runner)
    result = driver.run("gemini-3.1-pro-high", "hello")

    assert result.success is False
    assert result.error_type == "rate_limit"


# --- CodexDriver --------------------------------------------------------------


def test_codex_driver_parses_real_jsonl_payload():
    # Exact payload shape from docs/quota-and-token-tracking.md 2.4.
    lines = [
        json.dumps({"type": "item.completed", "item": {"type": "agent_message", "text": "hi"}}),
        json.dumps(
            {"type": "turn.completed", "usage": {"input_tokens": 8835, "output_tokens": 16, "cached_input_tokens": 7808}}
        ),
    ]

    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout="\n".join(lines))

    driver = CodexDriver(runner=runner)
    result = driver.run("o3", "hello")

    assert result.success is True
    assert result.response_text == "hi"
    assert result.input_tokens == 8835
    assert result.output_tokens == 16


def test_codex_driver_missing_turn_completed_is_failure():
    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        return _completed(stdout="", stderr="Error: usage limit reached, try again later", returncode=1)

    driver = CodexDriver(runner=runner)
    result = driver.run("o3", "hello")

    assert result.success is False
    assert result.error_type == "rate_limit"


def test_codex_driver_reports_unreachable_when_binary_missing():
    def runner(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess:
        raise FileNotFoundError("codex not found")

    driver = CodexDriver(runner=runner)
    result = driver.run("o3", "hello")

    assert result.success is False
    assert result.error_type == "unreachable"
