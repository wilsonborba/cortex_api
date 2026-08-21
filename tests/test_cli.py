from __future__ import annotations

import json

import pytest
from typer.testing import CliRunner

import lib.presentation.cli.main as cli_main
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.pin_repository import RoutingPinRepository
from lib.engine.executor import ExecutionResult, StepResult, UnresolvedStrategyError
from lib.engine.prompt_normalizer import PromptNormalizationResult
from lib.engine.router import ExecutionPlan, ModelSelection, NoEligibleModelError

runner = CliRunner()


@pytest.fixture
def model_repo_() -> ModelRepository:
    return ModelRepository()


@pytest.fixture
def pin_repo_() -> RoutingPinRepository:
    return RoutingPinRepository()


def _seed_model(repo: ModelRepository, **overrides) -> ModelCatalogEntry:
    defaults = dict(
        provider="cli-test-provider", display_name="model", access_status=AccessStatus.AVAILABLE.value,
        context_window=8192, is_local=True, tier_eligibility=[0, 1, 2], capabilities={},
        cost_per_million_tokens=0.0, is_enabled=True,
    )
    defaults.update(overrides)
    return repo.upsert(ModelCatalogEntry(**defaults))


def _invoke(*args: str):
    return runner.invoke(cli_main.app, list(args))


# --- root ------------------------------------------------------------------------


def test_version_flag_prints_a_version_and_exits_zero():
    result = _invoke("--version")
    assert result.exit_code == 0
    assert "cortex" in result.output


def test_no_args_shows_help():
    result = _invoke()
    # Typer's no_args_is_help treats a bare invocation as a usage error (exit 2),
    # not success -- it still prints the full command list to guide the user.
    assert result.exit_code == 2
    assert "run" in result.output


# --- models ------------------------------------------------------------------------


def test_models_list_returns_seeded_model_as_json(model_repo_):
    _seed_model(model_repo_, id="cli-test-provider/cli-list-model", display_name="CLI List Model")

    result = _invoke("models", "list")

    assert result.exit_code == 0
    ids = {m["id"] for m in json.loads(result.output)}
    assert "cli-test-provider/cli-list-model" in ids


def test_models_get_returns_the_model(model_repo_):
    _seed_model(model_repo_, id="cli-test-provider/cli-get-model", display_name="CLI Get Model")

    result = _invoke("models", "get", "cli-test-provider/cli-get-model")

    assert result.exit_code == 0
    assert json.loads(result.output)["display_name"] == "CLI Get Model"


def test_models_get_missing_model_exits_nonzero():
    result = _invoke("models", "get", "cli-test-provider/does-not-exist")
    assert result.exit_code != 0
    assert "not found" in result.output


def test_models_config_updates_tiers_and_enabled(model_repo_):
    _seed_model(model_repo_, id="cli-test-provider/cli-config-model")

    result = _invoke("models", "config", "cli-test-provider/cli-config-model", "--tiers", "0,1", "--disable")

    assert result.exit_code == 0
    body = json.loads(result.output)
    assert body["tier_eligibility"] == [0, 1]
    assert body["is_enabled"] is False


def test_models_config_sets_and_clears_context_format_pin(model_repo_):
    _seed_model(model_repo_, id="cli-test-provider/cli-pin-model")

    set_result = _invoke(
        "models", "config", "cli-test-provider/cli-pin-model", "--context-format", "toon", "--context-format-ttl", "3600"
    )
    assert set_result.exit_code == 0
    set_body = json.loads(set_result.output)
    assert set_body["context_format_pin"] == "toon"
    assert set_body["context_format_pin_expires_at"] is not None

    get_result = _invoke("models", "get", "cli-test-provider/cli-pin-model")
    assert json.loads(get_result.output)["context_format_pin"] == "toon"  # parity with the set

    clear_result = _invoke("models", "config", "cli-test-provider/cli-pin-model", "--context-format", "none")
    assert clear_result.exit_code == 0
    assert json.loads(clear_result.output)["context_format_pin"] is None


def test_models_config_rejects_unknown_context_format(model_repo_):
    _seed_model(model_repo_, id="cli-test-provider/cli-bad-format-model")

    result = _invoke("models", "config", "cli-test-provider/cli-bad-format-model", "--context-format", "yaml")

    assert result.exit_code != 0
    assert "unknown context format" in result.output


def test_models_sync_returns_a_list():
    result = _invoke("models", "sync")
    assert result.exit_code == 0
    assert isinstance(json.loads(result.output), list)


# --- tiers -------------------------------------------------------------------------


def test_tiers_list_includes_all_presets():
    result = _invoke("tiers", "list")
    assert result.exit_code == 0
    tiers = {t["tier"] for t in json.loads(result.output)}
    assert tiers == {0, 1, 2, 3, 4, 5}


def test_tiers_config_updates_max_latency():
    result = _invoke("tiers", "config", "2", "--max-latency", "33")
    assert result.exit_code == 0
    assert json.loads(result.output)["max_latency_seconds"] == 33


def test_tiers_config_unknown_tier_exits_nonzero():
    result = _invoke("tiers", "config", "9", "--max-latency", "10")
    assert result.exit_code != 0


def test_tiers_set_models_and_add_model():
    _invoke("tiers", "set-models", "3", "--models", "cli-test-provider/tier-model-a")
    result = _invoke("tiers", "add-model", "3", "--model", "cli-test-provider/tier-model-b")

    assert result.exit_code == 0
    assert json.loads(result.output)["allowed_models"] == [
        "cli-test-provider/tier-model-a", "cli-test-provider/tier-model-b",
    ]


# --- pin ---------------------------------------------------------------------------


def test_pin_lifecycle_set_list_remove():
    set_result = _invoke("pin", "set", "--tier", "3", "--task", "cli-pin-task", "--strategy", "cli_pin_v1")
    assert set_result.exit_code == 0
    assert json.loads(set_result.output)["strategy_id"] == "cli_pin_v1"

    list_result = _invoke("pin", "list")
    pins = json.loads(list_result.output)
    assert any(p["tier"] == 3 and p["task"] == "cli-pin-task" for p in pins)

    remove_result = _invoke("pin", "remove", "--tier", "3", "--task", "cli-pin-task")
    assert remove_result.exit_code == 0
    assert json.loads(remove_result.output)["removed"] is True


def test_pin_set_requires_strategy_or_model():
    result = _invoke("pin", "set", "--tier", "3")
    assert result.exit_code != 0
    assert "strategy" in result.output.lower()


def test_pin_remove_missing_pin_exits_nonzero():
    result = _invoke("pin", "remove", "--tier", "3", "--task", "no-such-cli-pin-task")
    assert result.exit_code != 0


# --- telemetry -----------------------------------------------------------------------


def test_telemetry_stats_and_history_are_lists():
    stats_result = _invoke("telemetry", "stats")
    assert stats_result.exit_code == 0
    assert isinstance(json.loads(stats_result.output), list)

    history_result = _invoke("telemetry", "history", "--limit", "5")
    assert history_result.exit_code == 0
    assert isinstance(json.loads(history_result.output), list)


# --- quota -------------------------------------------------------------------------


def test_quota_summary_is_a_list():
    result = _invoke("quota")
    assert result.exit_code == 0
    assert isinstance(json.loads(result.output), list)


def test_quota_for_local_provider_is_unbounded():
    result = _invoke("quota", "--provider", "ollama")
    assert result.exit_code == 0
    body = json.loads(result.output)
    assert body["window_limit_tokens"] is None
    assert body["quota_factor"] == 1.0


# --- run (router/executor mocked: no real subprocess/HTTP calls) -------------------


def _plan() -> ExecutionPlan:
    return ExecutionPlan(
        tier=1, task_type="general", strategy_id="general_t1_dynamic", prompt="hi",
        original_prompt="hi",
        selections=[ModelSelection(model_id="cli-test-provider/run-model", provider="cli-test-provider", role="primary")],
        allow_multi_model=False, retrieval_mode="none", needs_web=False, use_memory=False, memory_topic=None,
        require_verification=False, max_latency_seconds=10, max_model_calls=1, source="dynamic", reason="test",
    )


def _result(**overrides) -> ExecutionResult:
    defaults = dict(
        request_id="req-cli-1", tier_requested=1, tier_executed=1, strategy_id="general_t1_dynamic",
        task_type="general", success=True, response_text="cli answer",
        steps=[
            StepResult(
                role="primary", provider="cli-test-provider", model_id="cli-test-provider/run-model", success=True,
                response_text="cli answer", input_tokens=6, output_tokens=3, latency_ms=50, cost_usd=0.0,
                error_type=None, error_message=None, attempts=1,
            )
        ],
        total_input_tokens=6, total_output_tokens=3, total_cost_usd=0.0, latency_ms=50, error_type=None,
    )
    defaults.update(overrides)
    return ExecutionResult(**defaults)


class _FakeRouter:
    def __init__(self, plan=None, error=None):
        self._plan, self._error = plan, error
        self.last_request = None

    def build_execution_plan(self, request):
        self.last_request = request
        if self._error:
            raise self._error
        return self._plan


class _FakeExecutor:
    def __init__(self, result=None, error=None):
        self._result, self._error = result, error

    async def execute(self, plan):
        if self._error:
            raise self._error
        return self._result


class _FakePromptNormalizer:
    def __init__(self, prompt: str = "hello") -> None:
        self._prompt = prompt
        self.calls: list[str] = []

    def normalize(self, prompt: str) -> PromptNormalizationResult:
        self.calls.append(prompt)
        return PromptNormalizationResult(prompt=self._prompt, changed=self._prompt != prompt)


def test_run_prints_the_response_and_exits_zero(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr(cli_main, "get_router", lambda: _FakeRouter(plan=_plan()))
    monkeypatch.setattr(cli_main, "get_executor", lambda: _FakeExecutor(result=_result()))
    monkeypatch.setattr(cli_main, "get_prompt_normalizer", lambda: _FakePromptNormalizer(prompt="hello"))

    result = _invoke("run", "hello", "--tier", "1")

    assert result.exit_code == 0
    body = json.loads(result.output)
    assert body["response"] == "cli answer"
    assert body["success"] is True
    assert body["total_tokens"] == 9


def test_run_exits_nonzero_when_execution_fails(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr(cli_main, "get_router", lambda: _FakeRouter(plan=_plan()))
    monkeypatch.setattr(
        cli_main, "get_executor",
        lambda: _FakeExecutor(result=_result(success=False, response_text="", error_type="rate_limit")),
    )
    monkeypatch.setattr(cli_main, "get_prompt_normalizer", lambda: _FakePromptNormalizer(prompt="hello"))

    result = _invoke("run", "hello")

    assert result.exit_code == 1


def test_run_reports_no_eligible_model_cleanly(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr(cli_main, "get_router", lambda: _FakeRouter(error=NoEligibleModelError("nothing eligible")))
    monkeypatch.setattr(cli_main, "get_executor", lambda: _FakeExecutor())
    monkeypatch.setattr(cli_main, "get_prompt_normalizer", lambda: _FakePromptNormalizer(prompt="hello"))

    result = _invoke("run", "hello", "--tier", "5")

    assert result.exit_code == 1
    assert "nothing eligible" in result.output


def test_run_reports_unresolved_strategy_cleanly(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr(cli_main, "get_router", lambda: _FakeRouter(plan=_plan()))
    monkeypatch.setattr(
        cli_main, "get_executor", lambda: _FakeExecutor(error=UnresolvedStrategyError("coding_t3_custom"))
    )
    monkeypatch.setattr(cli_main, "get_prompt_normalizer", lambda: _FakePromptNormalizer(prompt="hello"))

    result = _invoke("run", "hello", "--force-strategy", "coding_t3_custom")

    assert result.exit_code == 1


def test_run_forwards_context_format_flag_to_the_router(monkeypatch: pytest.MonkeyPatch):
    router = _FakeRouter(plan=_plan())
    monkeypatch.setattr(cli_main, "get_router", lambda: router)
    monkeypatch.setattr(cli_main, "get_executor", lambda: _FakeExecutor(result=_result()))
    monkeypatch.setattr(cli_main, "get_prompt_normalizer", lambda: _FakePromptNormalizer(prompt="hello"))

    result = _invoke("run", "hello", "--context-format", "json")

    assert result.exit_code == 0
    assert router.last_request.force_context_format == "json"


def test_run_forwards_thinking_auto_retrieval_and_normalizes_prompt(monkeypatch: pytest.MonkeyPatch):
    router = _FakeRouter(plan=_plan())
    normalizer = _FakePromptNormalizer(prompt="structured hello")
    monkeypatch.setattr(cli_main, "get_router", lambda: router)
    monkeypatch.setattr(cli_main, "get_executor", lambda: _FakeExecutor(result=_result()))
    monkeypatch.setattr(cli_main, "get_prompt_normalizer", lambda: normalizer)

    result = _invoke("run", "hello", "--thinking", "--auto-retrieval")

    assert result.exit_code == 0
    assert normalizer.calls == ["hello"]
    assert router.last_request.prompt == "structured hello"
    assert router.last_request.thinking is True
    assert router.last_request.auto_retrieval is True


def test_run_can_skip_prompt_normalization(monkeypatch: pytest.MonkeyPatch):
    router = _FakeRouter(plan=_plan())
    normalizer = _FakePromptNormalizer(prompt="structured hello")
    monkeypatch.setattr(cli_main, "get_router", lambda: router)
    monkeypatch.setattr(cli_main, "get_executor", lambda: _FakeExecutor(result=_result()))
    monkeypatch.setattr(cli_main, "get_prompt_normalizer", lambda: normalizer)

    result = _invoke("run", "hello", "--no-normalize-prompt")

    assert result.exit_code == 0
    assert normalizer.calls == []
    assert router.last_request.prompt == "hello"


# --- stream --------------------------------------------------------------------------


def test_stream_reports_connection_failure_cleanly():
    # No API server is running in the test environment: this exercises the
    # real failure path (graceful error + exit 1) rather than a happy path
    # that would need a live server.
    result = _invoke("stream")
    assert result.exit_code == 1
    assert "could not connect" in result.output.lower()


# --- db ----------------------------------------------------------------------------


def test_db_upgrade_runs_cleanly():
    result = _invoke("db", "upgrade")
    assert result.exit_code == 0
    assert "upgraded" in result.output.lower()
