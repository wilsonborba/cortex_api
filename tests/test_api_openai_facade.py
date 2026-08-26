from __future__ import annotations

import json

import pytest
from fastapi.testclient import TestClient

from lib.core.settings import get_settings
from lib.engine.executor import ExecutionResult, StepResult, UnresolvedStrategyError
from lib.engine.prompt_normalizer import PromptNormalizationResult
from lib.engine.router import ExecutionPlan, ModelSelection, NoEligibleModelError, RoutingRequest
from lib.presentation.api.app import create_app
from lib.presentation.api.deps import get_executor, get_prompt_normalizer, get_router, get_security_shield
from lib.presentation.api.routes.openai_facade import _messages_to_prompt, _translate_model
from lib.presentation.api.schemas.openai_facade import ChatMessage


def _plan() -> ExecutionPlan:
    return ExecutionPlan(
        tier=3, task_type="general", strategy_id="general_t3_dynamic", prompt="hi",
        original_prompt="hi",
        selections=[ModelSelection(model_id="claude/facade-model", provider="claude", role="primary")],
        allow_multi_model=False, retrieval_mode="none", needs_web=False, use_memory=False, memory_topic=None,
        require_verification=False, max_latency_seconds=45, max_model_calls=1, source="dynamic", reason="test",
    )


class _FakeRouter:
    def __init__(self, plan=None, error=None):
        self._plan = plan
        self._error = error
        self.last_request: RoutingRequest | None = None

    def build_execution_plan(self, request: RoutingRequest) -> ExecutionPlan:
        self.last_request = request
        if self._error:
            raise self._error
        return self._plan


class _FakeExecutor:
    def __init__(self, result=None, error=None):
        self._result = result
        self._error = error

    async def execute(self, plan: ExecutionPlan) -> ExecutionResult:
        if self._error:
            raise self._error
        return self._result


class _FakePromptNormalizer:
    def __init__(self, prompt: str = "hi") -> None:
        self._prompt = prompt
        self.calls: list[str] = []

    def normalize(self, prompt: str) -> PromptNormalizationResult:
        self.calls.append(prompt)
        return PromptNormalizationResult(prompt=self._prompt, changed=self._prompt != prompt)


class _FakeSecurityShield:
    def evaluate(self, prompt: str):
        from lib.core.security import SecurityEvaluation
        return SecurityEvaluation(risk_flag_detected=False, flags=[], is_blocked=False)


def _result(**overrides) -> ExecutionResult:
    defaults = dict(
        request_id="req-facade-1", tier_requested=3, tier_executed=3, strategy_id="general_t3_dynamic",
        task_type="general", success=True, response_text="def validar_cpf(): pass",
        steps=[
            StepResult(
                role="primary", provider="claude", model_id="claude/facade-model", success=True,
                response_text="def validar_cpf(): pass", input_tokens=20, output_tokens=8, latency_ms=300,
                cost_usd=0.01, error_type=None, error_message=None, attempts=1,
            )
        ],
        total_input_tokens=20, total_output_tokens=8, total_cost_usd=0.01, latency_ms=300, error_type=None,
    )
    defaults.update(overrides)
    return ExecutionResult(**defaults)


@pytest.fixture
def app_():
    settings = get_settings().model_copy(
        update={"api_sync_models_on_startup": False, "api_background_tasks_enabled": False}
    )
    app = create_app(settings=settings)
    app.dependency_overrides[get_security_shield] = lambda: _FakeSecurityShield()
    return app


# --- model translation (pure functions) -------------------------------------------


@pytest.mark.parametrize(
    "model,expected_tier,expected_strategy",
    [
        ("cortex-auto", "auto", None),
        ("cortex-t0", "0", None),
        ("cortex-t3", "3", None),
        ("cortex-t5", "5", None),
        ("cortex-pin:coding_t3_v1", "auto", "coding_t3_v1"),
        ("gpt-4", "auto", None),  # unrecognized: falls back to auto, doesn't reject
    ],
)
def test_translate_model(model, expected_tier, expected_strategy):
    assert _translate_model(model) == (expected_tier, expected_strategy)


def test_messages_to_prompt_includes_every_role():
    messages = [
        ChatMessage(role="system", content="You are a senior engineer."),
        ChatMessage(role="user", content="Write a CPF validator."),
    ]
    prompt = _messages_to_prompt(messages)
    assert "System: You are a senior engineer." in prompt
    assert "User: Write a CPF validator." in prompt


# --- GET /v1/models -----------------------------------------------------------------


def test_list_virtual_models(app_):
    with TestClient(app_) as client:
        response = client.get("/v1/models")

    assert response.status_code == 200
    body = response.json()
    assert body["object"] == "list"
    ids = {m["id"] for m in body["data"]}
    assert ids == {"cortex-auto", "cortex-t0", "cortex-t1", "cortex-t2", "cortex-t3", "cortex-t4", "cortex-t5"}
    assert all(m["object"] == "model" for m in body["data"])


# --- POST /v1/chat/completions (non-streaming) --------------------------------------


def test_chat_completions_returns_openai_shaped_response(app_):
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    app_.dependency_overrides[get_router] = lambda: router
    app_.dependency_overrides[get_executor] = lambda: executor
    app_.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="User: hi")

    with TestClient(app_) as client:
        response = client.post(
            "/v1/chat/completions",
            json={"model": "cortex-t3", "messages": [{"role": "user", "content": "write a cpf validator"}]},
        )

    assert response.status_code == 200
    body = response.json()
    assert body["object"] == "chat.completion"
    assert body["model"] == "cortex-t3"
    assert body["choices"][0]["message"]["role"] == "assistant"
    assert body["choices"][0]["message"]["content"] == "def validar_cpf(): pass"
    assert body["choices"][0]["finish_reason"] == "stop"
    assert body["usage"]["prompt_tokens"] == 20
    assert body["usage"]["completion_tokens"] == 8
    assert body["usage"]["total_tokens"] == 28
    assert router.last_request.tier == "3"


def test_chat_completions_translates_pin_model_to_force_strategy(app_):
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    app_.dependency_overrides[get_router] = lambda: router
    app_.dependency_overrides[get_executor] = lambda: executor
    app_.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="User: hi")

    with TestClient(app_) as client:
        client.post(
            "/v1/chat/completions",
            json={"model": "cortex-pin:coding_t3_v1", "messages": [{"role": "user", "content": "hi"}]},
        )

    assert router.last_request.force_strategy == "coding_t3_v1"


def test_chat_completions_returns_openai_style_409_error(app_):
    app_.dependency_overrides[get_router] = lambda: _FakeRouter(error=NoEligibleModelError("nothing eligible"))
    app_.dependency_overrides[get_executor] = lambda: _FakeExecutor()
    app_.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="User: hi")

    with TestClient(app_) as client:
        response = client.post(
            "/v1/chat/completions", json={"model": "cortex-t5", "messages": [{"role": "user", "content": "hi"}]}
        )

    assert response.status_code == 409
    assert response.json()["error"]["type"] == "no_eligible_model"


def test_chat_completions_returns_openai_style_501_error(app_):
    app_.dependency_overrides[get_router] = lambda: _FakeRouter(plan=_plan())
    app_.dependency_overrides[get_executor] = lambda: _FakeExecutor(
        error=UnresolvedStrategyError("coding_t3_custom")
    )
    app_.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="User: hi")

    with TestClient(app_) as client:
        response = client.post(
            "/v1/chat/completions", json={"model": "cortex-t3", "messages": [{"role": "user", "content": "hi"}]}
        )

    assert response.status_code == 501
    assert response.json()["error"]["type"] == "unresolved_strategy"


# --- POST /v1/chat/completions (streaming) --------------------------------------------


def test_chat_completions_forwards_quality_controls(app_):
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    normalizer = _FakePromptNormalizer(prompt="structured prompt")
    app_.dependency_overrides[get_router] = lambda: router
    app_.dependency_overrides[get_executor] = lambda: executor
    app_.dependency_overrides[get_prompt_normalizer] = lambda: normalizer

    with TestClient(app_) as client:
        response = client.post(
            "/v1/chat/completions",
            json={
                "model": "cortex-t4",
                "messages": [{"role": "user", "content": "hi"}],
                "thinking": True,
                "auto_retrieval": True,
                "needs_web": True,
                "use_memory": True,
                "memory_topic": "ops",
            },
        )

    assert response.status_code == 200
    assert normalizer.calls == ["User: hi"]
    assert router.last_request.prompt == "structured prompt"
    assert router.last_request.thinking is True
    assert router.last_request.auto_retrieval is True
    assert router.last_request.needs_web is True
    assert router.last_request.use_memory is True
    assert router.last_request.memory_topic == "ops"


def test_chat_completions_can_forward_the_raw_prompt_without_normalization(app_):
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    normalizer = _FakePromptNormalizer(prompt="structured prompt")
    app_.dependency_overrides[get_router] = lambda: router
    app_.dependency_overrides[get_executor] = lambda: executor
    app_.dependency_overrides[get_prompt_normalizer] = lambda: normalizer

    with TestClient(app_) as client:
        response = client.post(
            "/v1/chat/completions",
            json={
                "model": "cortex-t3",
                "messages": [{"role": "user", "content": "messy prompt"}],
                "normalize_prompt": False,
            },
        )

    assert response.status_code == 200
    assert normalizer.calls == []
    assert router.last_request.prompt == "User: messy prompt"


def test_chat_completions_streaming_emits_valid_sse_frames_ending_in_done(app_):
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result(response_text="one two three four five six seven"))
    app_.dependency_overrides[get_router] = lambda: router
    app_.dependency_overrides[get_executor] = lambda: executor
    app_.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="User: hi")

    with TestClient(app_) as client:
        with client.stream(
            "POST", "/v1/chat/completions",
            json={"model": "cortex-t3", "messages": [{"role": "user", "content": "hi"}], "stream": True},
        ) as response:
            assert response.status_code == 200
            assert response.headers["content-type"].startswith("text/event-stream")
            raw = "".join(response.iter_text())

    lines = [line for line in raw.split("\n\n") if line.strip()]
    assert lines[-1] == "data: [DONE]"

    data_frames = [json.loads(line[len("data: "):]) for line in lines[:-1]]
    assert data_frames[0]["object"] == "chat.completion.chunk"
    assert data_frames[0]["choices"][0]["delta"] == {"role": "assistant"}

    full_text = "".join(
        frame["choices"][0]["delta"].get("content", "") for frame in data_frames
    )
    assert full_text == "one two three four five six seven"
    assert data_frames[-1]["choices"][0]["finish_reason"] == "stop"


def test_chat_completions_streaming_marks_failed_execution_as_error_finish_reason(app_):
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result(success=False, response_text="", error_type="rate_limit"))
    app_.dependency_overrides[get_router] = lambda: router
    app_.dependency_overrides[get_executor] = lambda: executor
    app_.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="User: hi")

    with TestClient(app_) as client:
        with client.stream(
            "POST", "/v1/chat/completions",
            json={"model": "cortex-t3", "messages": [{"role": "user", "content": "hi"}], "stream": True},
        ) as response:
            raw = "".join(response.iter_text())

    lines = [line for line in raw.split("\n\n") if line.strip() and line != "data: [DONE]"]
    frames = [json.loads(line[len("data: "):]) for line in lines]
    assert frames[-1]["choices"][0]["finish_reason"] == "error"
