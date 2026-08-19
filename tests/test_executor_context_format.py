from __future__ import annotations

from datetime import datetime, timedelta, timezone
from typing import List, Optional

import pytest

from lib.core.settings import Settings
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.quota_repository import QuotaRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.drivers.base import DriverResult
from lib.engine.executor import Executor
from lib.engine.format import JSON, TOON
from lib.engine.quota import QuotaTracker
from lib.engine.registry_service import ModelRegistryService
from lib.engine.router import ExecutionPlan, ModelSelection
from lib.engine.telemetry import TelemetryLogger


# --- fixtures (see tests/test_executor.py for why these don't share a db_session) --


@pytest.fixture
def model_repo_() -> ModelRepository:
    return ModelRepository()


@pytest.fixture
def registry_() -> ModelRegistryService:
    return ModelRegistryService(discoveries=[], repository=ModelRepository())


@pytest.fixture
def quota_tracker_(model_repo_: ModelRepository) -> QuotaTracker:
    return QuotaTracker(quota_repo=QuotaRepository(), model_repo=model_repo_, settings=Settings())


@pytest.fixture
def telemetry_repo_() -> TelemetryRepository:
    return TelemetryRepository()


@pytest.fixture
def telemetry_(telemetry_repo_: TelemetryRepository) -> TelemetryLogger:
    return TelemetryLogger(repo=telemetry_repo_)


# --- test doubles (mirrors tests/test_executor.py's) --------------------------------


class _ScriptedDriver:
    def __init__(self, results: List[DriverResult]) -> None:
        self._results = results
        self.calls: List[tuple] = []

    def run(self, model: str, prompt: str) -> DriverResult:
        self.calls.append((model, prompt))
        index = min(len(self.calls) - 1, len(self._results) - 1)
        return self._results[index]


class _FakeWebRetrieval:
    def __init__(self, items: List[dict]) -> None:
        self._items = items

    def gather_context(self, query: str, max_results=None):
        from lib.engine.retrieval.service import WebContextResult

        return WebContextResult(
            query=query, markdown="unused", sources=[i["url"] for i in self._items], items=self._items
        )


def _success(text: str = "ok") -> DriverResult:
    return DriverResult(success=True, response_text=text, input_tokens=5, output_tokens=3, latency_ms=10)


def _failure(error_type: str) -> DriverResult:
    return DriverResult(
        success=False, response_text="", input_tokens=0, output_tokens=0, latency_ms=0,
        error_type=error_type, error_message="failed",
    )


def _plan(**overrides) -> ExecutionPlan:
    defaults = dict(
        tier=1, task_type="general", strategy_id="general_t1_dynamic", prompt="what is sqlite",
        selections=[ModelSelection(model_id="ctxexec/model", provider="ctxexec", role="primary")],
        allow_multi_model=False, retrieval_mode="none", needs_web=True, use_memory=False, memory_topic=None,
        require_verification=False, max_latency_seconds=30, max_model_calls=1, source="dynamic", reason="test",
        context_format=None,
    )
    defaults.update(overrides)
    return ExecutionPlan(**defaults)


def _seed_model(repo: ModelRepository, model_id: str = "ctxexec/model", **overrides) -> ModelCatalogEntry:
    defaults = dict(
        provider="ctxexec", display_name="model", access_status=AccessStatus.AVAILABLE.value,
        context_window=8192, is_local=False, tier_eligibility=[1], capabilities={},
        cost_per_million_tokens=0.0, is_enabled=True,
    )
    defaults.update(overrides)
    return repo.upsert(ModelCatalogEntry(id=model_id, **defaults))


WEB_ITEMS = [{"title": "SQLite", "url": "https://sqlite.org", "content": "SQLite is a database engine"}]


def _executor(driver, quota_tracker, telemetry, registry, max_retries: int = 0) -> Executor:
    return Executor(
        drivers={"ctxexec": driver}, quota_tracker=quota_tracker, telemetry=telemetry,
        router=None, web_retrieval=_FakeWebRetrieval(WEB_ITEMS), hippocampus=None,
        registry=registry, max_retries=max_retries, max_reroutes=0,
    )


# --- format resolution: auto / pin / force -----------------------------------------


@pytest.mark.asyncio
async def test_defaults_to_toon_when_model_has_no_preference(model_repo_, registry_, quota_tracker_, telemetry_):
    _seed_model(model_repo_, "ctxexec/toon-default")
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/toon-default", provider="ctxexec", role="primary")])

    await executor.execute(plan)

    prompt = driver.calls[0][1]
    assert "web_results[1]{title,url,content}:" in prompt  # TOON tabular header


@pytest.mark.asyncio
async def test_uses_computed_preference_when_set(model_repo_, registry_, quota_tracker_, telemetry_):
    _seed_model(model_repo_, "ctxexec/computed-json", context_format_computed=JSON)
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/computed-json", provider="ctxexec", role="primary")])

    await executor.execute(plan)

    prompt = driver.calls[0][1]
    assert '"web_results"' in prompt  # JSON key, not a TOON header
    assert "[1]{title,url,content}" not in prompt


@pytest.mark.asyncio
async def test_active_pin_overrides_computed_default(model_repo_, registry_, quota_tracker_, telemetry_):
    _seed_model(
        model_repo_, "ctxexec/pinned", context_format_computed=JSON, context_format_pin=TOON,
    )
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/pinned", provider="ctxexec", role="primary")])

    await executor.execute(plan)

    assert "web_results[1]{title,url,content}:" in driver.calls[0][1]


@pytest.mark.asyncio
async def test_expired_pin_falls_back_to_computed(model_repo_, registry_, quota_tracker_, telemetry_):
    past = datetime.now(timezone.utc) - timedelta(minutes=1)
    _seed_model(
        model_repo_, "ctxexec/pin-expired", context_format_computed=JSON, context_format_pin=TOON,
        context_format_pin_expires_at=past,
    )
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/pin-expired", provider="ctxexec", role="primary")])

    await executor.execute(plan)

    assert '"web_results"' in driver.calls[0][1]  # pin expired: computed (JSON) wins


@pytest.mark.asyncio
async def test_forced_format_overrides_an_active_pin(model_repo_, registry_, quota_tracker_, telemetry_):
    _seed_model(model_repo_, "ctxexec/force-over-pin", context_format_pin=TOON)
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(
        selections=[ModelSelection(model_id="ctxexec/force-over-pin", provider="ctxexec", role="primary")],
        context_format=JSON,
    )

    await executor.execute(plan)

    assert '"web_results"' in driver.calls[0][1]


@pytest.mark.asyncio
async def test_unknown_model_defaults_to_toon_without_crashing(quota_tracker_, telemetry_, registry_):
    # force_model to something never seeded in the registry.
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/never-registered", provider="ctxexec", role="primary")])

    await executor.execute(plan)

    assert "web_results[1]{title,url,content}:" in driver.calls[0][1]


# --- JSON fallback retry on failure -------------------------------------------------


@pytest.mark.asyncio
async def test_toon_failure_retries_in_json_and_persists_the_preference(
    model_repo_, registry_, quota_tracker_, telemetry_
):
    _seed_model(model_repo_, "ctxexec/fallback")
    driver = _ScriptedDriver([_failure("cli_error"), _success("recovered")])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/fallback", provider="ctxexec", role="primary")])

    result = await executor.execute(plan)

    assert result.success is True
    assert result.response_text == "recovered"
    assert len(driver.calls) == 2
    assert "web_results[1]{title,url,content}:" in driver.calls[0][1]  # 1st attempt: TOON
    assert '"web_results"' in driver.calls[1][1]  # 2nd attempt: JSON
    assert model_repo_.get_by_id("ctxexec/fallback").context_format_computed == JSON


@pytest.mark.asyncio
async def test_pinned_model_never_gets_the_auto_fallback(model_repo_, registry_, quota_tracker_, telemetry_):
    _seed_model(model_repo_, "ctxexec/pinned-no-fallback", context_format_pin=TOON)
    driver = _ScriptedDriver([_failure("cli_error")])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/pinned-no-fallback", provider="ctxexec", role="primary")])

    result = await executor.execute(plan)

    assert result.success is False
    assert len(driver.calls) == 1  # no automatic retry: the pin is a manual decision


@pytest.mark.asyncio
async def test_forced_format_never_gets_the_auto_fallback(model_repo_, registry_, quota_tracker_, telemetry_):
    _seed_model(model_repo_, "ctxexec/forced-no-fallback")
    driver = _ScriptedDriver([_failure("cli_error")])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(
        selections=[ModelSelection(model_id="ctxexec/forced-no-fallback", provider="ctxexec", role="primary")],
        context_format=TOON,
    )

    result = await executor.execute(plan)

    assert result.success is False
    assert len(driver.calls) == 1


@pytest.mark.asyncio
async def test_already_json_failure_does_not_retry_again(model_repo_, registry_, quota_tracker_, telemetry_):
    _seed_model(model_repo_, "ctxexec/already-json", context_format_computed=JSON)
    driver = _ScriptedDriver([_failure("cli_error")])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/already-json", provider="ctxexec", role="primary")])

    result = await executor.execute(plan)

    assert result.success is False
    assert len(driver.calls) == 1  # nothing to fall back to from JSON


# --- telemetry: context_format / context_type -------------------------------------


@pytest.mark.asyncio
async def test_telemetry_records_format_and_type_for_the_primary_step(
    model_repo_, registry_, quota_tracker_, telemetry_, telemetry_repo_
):
    import asyncio

    _seed_model(model_repo_, "ctxexec/telemetry-model")
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(selections=[ModelSelection(model_id="ctxexec/telemetry-model", provider="ctxexec", role="primary")])

    await executor.execute(plan)
    await asyncio.sleep(0.05)

    events = telemetry_repo_.list_events(strategy_id=plan.strategy_id)
    assert events[0].context_format == TOON
    assert events[0].context_type == "web"


@pytest.mark.asyncio
async def test_telemetry_leaves_format_and_type_null_when_no_context_was_gathered(
    model_repo_, registry_, quota_tracker_, telemetry_, telemetry_repo_
):
    import asyncio

    _seed_model(model_repo_, "ctxexec/no-context-model")
    driver = _ScriptedDriver([_success()])
    executor = _executor(driver, quota_tracker_, telemetry_, registry_)
    plan = _plan(
        selections=[ModelSelection(model_id="ctxexec/no-context-model", provider="ctxexec", role="primary")],
        needs_web=False,
    )

    await executor.execute(plan)
    await asyncio.sleep(0.05)

    events = telemetry_repo_.list_events(strategy_id=plan.strategy_id)
    assert events[0].context_format is None
    assert events[0].context_type is None
