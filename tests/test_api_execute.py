from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

from lib.core.settings import get_settings
from lib.engine.executor import ExecutionResult, StepResult, UnresolvedStrategyError
from lib.engine.router import ExecutionPlan, ModelSelection, NoEligibleModelError, RoutingRequest
from lib.presentation.api.app import create_app
from lib.presentation.api.deps import get_executor, get_prompt_normalizer, get_router


def _plan(strategy_id: str = "general_t1_dynamic") -> ExecutionPlan:
    return ExecutionPlan(
        tier=1, task_type="general", strategy_id=strategy_id, prompt="hi",
        original_prompt="hi",
        selections=[ModelSelection(model_id="ollama/api-exec-model", provider="ollama", role="primary")],
        allow_multi_model=False, retrieval_mode="none", needs_web=False, use_memory=False, memory_topic=None,
        require_verification=False, max_latency_seconds=10, max_model_calls=1, source="dynamic", reason="test",
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
        self.last_plan: ExecutionPlan | None = None

    async def execute(self, plan: ExecutionPlan) -> ExecutionResult:
        self.last_plan = plan
        if self._error:
            raise self._error
        return self._result


class _FakePromptNormalizer:
    def __init__(self, prompt: str = "normalized hi") -> None:
        self._prompt = prompt
        self.calls: list[str] = []

    def normalize(self, prompt: str):
        from lib.engine.prompt_normalizer import PromptNormalizationResult

        self.calls.append(prompt)
        return PromptNormalizationResult(prompt=self._prompt, changed=self._prompt != prompt)


def _result(**overrides) -> ExecutionResult:
    defaults = dict(
        request_id="req-api-1", tier_requested=1, tier_executed=1, strategy_id="general_t1_dynamic",
        task_type="general", success=True, response_text="hello from cortex",
        steps=[
            StepResult(
                role="primary", provider="ollama", model_id="ollama/api-exec-model", success=True,
                response_text="hello from cortex", input_tokens=10, output_tokens=5, latency_ms=120,
                cost_usd=0.0, error_type=None, error_message=None, attempts=1,
            )
        ],
        total_input_tokens=10, total_output_tokens=5, total_cost_usd=0.0, latency_ms=120, error_type=None,
    )
    defaults.update(overrides)
    return ExecutionResult(**defaults)


@pytest.fixture
def app_and_overrides():
    settings = get_settings().model_copy(
        update={"api_sync_models_on_startup": False, "api_background_tasks_enabled": False}
    )
    app = create_app(settings=settings)
    return app


def test_execute_returns_the_executor_result(app_and_overrides):
    app = app_and_overrides
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    normalizer = _FakePromptNormalizer(prompt="hi")
    app.dependency_overrides[get_router] = lambda: router
    app.dependency_overrides[get_executor] = lambda: executor
    app.dependency_overrides[get_prompt_normalizer] = lambda: normalizer

    with TestClient(app) as client:
        response = client.post("/execute", json={"prompt": "hi", "tier": 1})

    assert response.status_code == 200
    body = response.json()
    assert body["success"] is True
    assert body["response"] == "hello from cortex"
    assert body["total_tokens"] == 15
    assert body["steps"][0]["role"] == "primary"


def test_execute_maps_override_strategy_to_force_strategy(app_and_overrides):
    app = app_and_overrides
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    normalizer = _FakePromptNormalizer(prompt="hi")
    app.dependency_overrides[get_router] = lambda: router
    app.dependency_overrides[get_executor] = lambda: executor
    app.dependency_overrides[get_prompt_normalizer] = lambda: normalizer

    with TestClient(app) as client:
        client.post("/execute", json={"prompt": "hi", "override_strategy": "coding_t3_v1"})

    assert router.last_request.force_strategy == "coding_t3_v1"


def test_execute_maps_force_context_format(app_and_overrides):
    app = app_and_overrides
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    normalizer = _FakePromptNormalizer(prompt="hi")
    app.dependency_overrides[get_router] = lambda: router
    app.dependency_overrides[get_executor] = lambda: executor
    app.dependency_overrides[get_prompt_normalizer] = lambda: normalizer

    with TestClient(app) as client:
        client.post("/execute", json={"prompt": "hi", "force_context_format": "json"})

    assert router.last_request.force_context_format == "json"


def test_execute_rejects_unknown_context_format(app_and_overrides):
    with TestClient(app_and_overrides) as client:
        response = client.post("/execute", json={"prompt": "hi", "force_context_format": "yaml"})

    assert response.status_code == 422


def test_execute_returns_409_when_no_eligible_model(app_and_overrides):
    app = app_and_overrides
    app.dependency_overrides[get_router] = lambda: _FakeRouter(error=NoEligibleModelError("nothing eligible"))
    app.dependency_overrides[get_executor] = lambda: _FakeExecutor()
    app.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="hi")

    with TestClient(app) as client:
        response = client.post("/execute", json={"prompt": "hi", "tier": 3})

    assert response.status_code == 409
    assert response.json()["error"] == "no_eligible_model"


def test_execute_returns_501_for_unresolved_strategy(app_and_overrides):
    app = app_and_overrides
    app.dependency_overrides[get_router] = lambda: _FakeRouter(plan=_plan())
    app.dependency_overrides[get_executor] = lambda: _FakeExecutor(
        error=UnresolvedStrategyError("coding_t3_custom")
    )
    app.dependency_overrides[get_prompt_normalizer] = lambda: _FakePromptNormalizer(prompt="hi")

    with TestClient(app) as client:
        response = client.post("/execute", json={"prompt": "hi"})

    assert response.status_code == 501
    assert response.json()["error"] == "unresolved_strategy"


def test_execute_returns_422_for_missing_prompt(app_and_overrides):
    with TestClient(app_and_overrides) as client:
        response = client.post("/execute", json={})

    assert response.status_code == 422


def test_execute_maps_thinking_and_auto_retrieval(app_and_overrides):
    app = app_and_overrides
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    normalizer = _FakePromptNormalizer(prompt="hi")
    app.dependency_overrides[get_router] = lambda: router
    app.dependency_overrides[get_executor] = lambda: executor
    app.dependency_overrides[get_prompt_normalizer] = lambda: normalizer

    with TestClient(app) as client:
        client.post("/execute", json={"prompt": "hi", "thinking": True, "auto_retrieval": True})

    assert router.last_request.thinking is True
    assert router.last_request.auto_retrieval is True


def test_execute_normalizes_prompt_before_routing(app_and_overrides):
    app = app_and_overrides
    router = _FakeRouter(plan=_plan())
    executor = _FakeExecutor(result=_result())
    normalizer = _FakePromptNormalizer(prompt="structured prompt")
    app.dependency_overrides[get_router] = lambda: router
    app.dependency_overrides[get_executor] = lambda: executor
    app.dependency_overrides[get_prompt_normalizer] = lambda: normalizer

    with TestClient(app) as client:
        client.post("/execute", json={"prompt": "messy prompt"})

    assert normalizer.calls == ["messy prompt"]
    assert router.last_request.prompt == "structured prompt"
