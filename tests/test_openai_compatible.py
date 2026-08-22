from __future__ import annotations

from typing import Any

import httpx
import pytest

from lib.dal.models import AccessStatus
from lib.engine.discovery.base import ProviderDiscoveryError
from lib.engine.discovery.cloudflare import CloudflareDiscovery
from lib.engine.discovery.groq import GroqDiscovery
from lib.engine.discovery.huggingface import HuggingFaceDiscovery
from lib.engine.discovery.openai_compatible import OpenAICompatibleDiscovery
from lib.engine.drivers.cloudflare import CloudflareDriver
from lib.engine.drivers.groq import GroqDriver
from lib.engine.drivers.openai_compatible import OpenAICompatibleDriver


class _FakeResponse:
    def __init__(self, payload: Any, status_code: int = 200) -> None:
        self._payload = payload
        self.status_code = status_code

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            request = httpx.Request("GET", "http://fake")
            response = httpx.Response(self.status_code, request=request)
            raise httpx.HTTPStatusError("error", request=request, response=response)

    def json(self) -> Any:
        return self._payload


class _FakeClient:
    def __init__(self, get_payload: Any = None, post_payload: Any = None, status_code: int = 200) -> None:
        self._get_payload = get_payload
        self._post_payload = post_payload
        self._status_code = status_code
        self.last_headers: dict[str, str] = {}
        self.last_url: str = ""

    def __enter__(self) -> "_FakeClient":
        return self

    def __exit__(self, *exc_info: Any) -> None:
        return None

    def get(self, url: str, headers: dict | None = None) -> _FakeResponse:
        self.last_url = url
        self.last_headers = headers or {}
        return _FakeResponse(self._get_payload, self._status_code)

    def post(self, url: str, headers: dict | None = None, json: dict | None = None) -> _FakeResponse:  # noqa: A002
        self.last_url = url
        self.last_headers = headers or {}
        return _FakeResponse(self._post_payload, self._status_code)


# --- OpenAICompatibleDiscovery / OpenAICompatibleDriver (base) ---------------


def test_openai_compatible_discovery_parses_models_list():
    payload = {"data": [{"id": "llama-3.1-8b-instant"}, {"id": "llama-3.3-70b-versatile"}]}
    client = _FakeClient(get_payload=payload)
    discovery = OpenAICompatibleDiscovery(
        provider="groq", base_url="https://api.groq.com/openai/v1", api_key="fake-key",
        client_factory=lambda: client,
    )

    models = discovery.discover()

    assert [m.id for m in models] == ["groq/llama-3.1-8b-instant", "groq/llama-3.3-70b-versatile"]
    assert all(m.access_status == AccessStatus.AVAILABLE.value for m in models)
    assert models[0].tier_eligibility == [1, 2, 3]
    assert models[1].tier_eligibility == [3, 4, 5]
    assert models[1].capabilities["reasoning"] > models[0].capabilities["reasoning"]
    assert client.last_headers["Authorization"] == "Bearer fake-key"


def test_openai_compatible_discovery_raises_without_api_key():
    discovery = OpenAICompatibleDiscovery(
        provider="groq", base_url="https://api.groq.com/openai/v1", api_key=None,
    )

    with pytest.raises(ProviderDiscoveryError) as exc_info:
        discovery.discover()
    assert exc_info.value.provider == "groq"


def test_openai_compatible_driver_parses_chat_completion():
    payload = {
        "choices": [{"message": {"content": "hi"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 3},
    }
    client = _FakeClient(post_payload=payload)
    driver = OpenAICompatibleDriver(
        provider="groq", base_url="https://api.groq.com/openai/v1", api_key="fake-key",
        client_factory=lambda: client,
    )

    result = driver.run("llama-3.1-8b-instant", "hello")

    assert result.success is True
    assert result.response_text == "hi"
    assert result.input_tokens == 10
    assert result.output_tokens == 3
    assert client.last_headers["Authorization"] == "Bearer fake-key"


def test_openai_compatible_driver_maps_429_to_rate_limit():
    client = _FakeClient(post_payload={"error": "rate limited"}, status_code=429)
    driver = OpenAICompatibleDriver(
        provider="groq", base_url="https://api.groq.com/openai/v1", api_key="fake-key",
        client_factory=lambda: client,
    )

    result = driver.run("llama-3.1-8b-instant", "hello")

    assert result.success is False
    assert result.error_type == "rate_limit_exceeded"


def test_openai_compatible_driver_fails_without_api_key():
    driver = OpenAICompatibleDriver(provider="groq", base_url="https://api.groq.com/openai/v1", api_key=None)

    result = driver.run("llama-3.1-8b-instant", "hello")

    assert result.success is False
    assert result.error_type == "unreachable"


# --- GroqDiscovery / GroqDriver (representative "normal" subclass) ----------


def test_groq_discovery_uses_groq_base_url_and_provider():
    client = _FakeClient(get_payload={"data": [{"id": "llama-3.1-8b-instant"}]})
    discovery = GroqDiscovery(api_key="fake-key", client_factory=lambda: client)

    models = discovery.discover()

    assert discovery.provider == "groq"
    assert models[0].id == "groq/llama-3.1-8b-instant"
    assert client.last_url == "https://api.groq.com/openai/v1/models"


def test_groq_driver_uses_groq_base_url():
    client = _FakeClient(post_payload={"choices": [{"message": {"content": "hi"}}], "usage": {}})
    driver = GroqDriver(api_key="fake-key", client_factory=lambda: client)

    result = driver.run("llama-3.1-8b-instant", "hello")

    assert result.success is True
    assert client.last_url == "https://api.groq.com/openai/v1/chat/completions"


# --- CloudflareDiscovery / CloudflareDriver (account_id exception) -----------


def test_cloudflare_discovery_embeds_account_id_in_base_url():
    client = _FakeClient(get_payload={"data": [{"id": "@cf/meta/llama-3.1-8b-instruct"}]})
    discovery = CloudflareDiscovery(api_key="fake-key", account_id="acct123", client_factory=lambda: client)

    models = discovery.discover()

    assert "acct123" in client.last_url
    assert models[0].id == "cloudflare/@cf/meta/llama-3.1-8b-instruct"


def test_cloudflare_discovery_raises_without_account_id():
    discovery = CloudflareDiscovery(api_key="fake-key", account_id=None)

    with pytest.raises(ProviderDiscoveryError):
        discovery.discover()


def test_cloudflare_driver_fails_without_account_id():
    driver = CloudflareDriver(api_key="fake-key", account_id=None)

    result = driver.run("@cf/meta/llama-3.1-8b-instruct", "hello")

    assert result.success is False
    assert result.error_type == "unreachable"


# --- HuggingFaceDiscovery (whoami-v2 + router exception) ---------------------


def test_huggingface_discovery_checks_auth_then_lists_models():
    client = _FakeClient(get_payload={"data": [{"id": "meta-llama/Llama-3.1-8B-Instruct"}]})
    discovery = HuggingFaceDiscovery(api_key="fake-key", client_factory=lambda: client)

    models = discovery.discover()

    assert models[0].id == "huggingface/meta-llama/Llama-3.1-8B-Instruct"
    assert models[0].tier_eligibility == [2, 3, 4]


def test_huggingface_discovery_raises_without_api_key():
    discovery = HuggingFaceDiscovery(api_key=None)

    with pytest.raises(ProviderDiscoveryError):
        discovery.discover()


def test_codex_driver_uses_configured_binary_path():
    client = _FakeClient(post_payload=None)
    seen = {}

    def runner(args, **kwargs):
        seen["argv0"] = args[0]
        class _Proc:
            returncode = 0
            stdout = '{"type":"item.completed","item":{"type":"agent_message","text":"hi"}}\n{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}\n'
            stderr = ""
        return _Proc()

    driver = OpenAICompatibleDriver(
        provider="groq", base_url="https://api.groq.com/openai/v1", api_key="fake-key",
        client_factory=lambda: client,
    )
    assert driver.provider == "groq"

    from lib.engine.drivers.codex import CodexDriver
    result = CodexDriver(command="/opt/tools/codex", runner=runner).run("o3", "hello")

    assert result.success is True
    assert seen["argv0"] == "/opt/tools/codex"
