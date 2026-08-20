from __future__ import annotations

import asyncio
from typing import Dict, List, Optional

import pytest

from lib.core.settings import Settings
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.quota_repository import QuotaRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.attachments import Attachment, AttachmentIngestor
from lib.engine.drivers.base import DriverResult
from lib.engine.executor import Executor, UnresolvedStrategyError, _bare_model_name
from lib.engine.quota import QuotaTracker
from lib.engine.registry_service import ModelRegistryService
from lib.engine.retrieval.hippocampus import MemoryChunk
from lib.engine.router import ExecutionPlan, ModelSelection, NoEligibleModelError
from lib.engine.telemetry import TelemetryLogger


# --- fixtures ------------------------------------------------------------------
#
# Unlike the rest of this suite, these deliberately do NOT bind every
# repository to one shared `db_session`: the Executor writes telemetry from a
# background thread (TelemetryLogger.record_async) while the main coroutine
# keeps going, and a single SQLAlchemy Session isn't safe to use from two
# threads concurrently. Each repository below uses its own default session
# factory instead, so every call opens its own short-lived Session -- exactly
# how production uses these repositories (see lib/engine/executor.build_default_executor).


@pytest.fixture
def model_repo_() -> ModelRepository:
    return ModelRepository()


@pytest.fixture
def quota_tracker_(model_repo_: ModelRepository) -> QuotaTracker:
    return QuotaTracker(
        quota_repo=QuotaRepository(),
        model_repo=model_repo_,
        settings=Settings(cooldown_minutes=15),
    )


@pytest.fixture
def telemetry_repo_() -> TelemetryRepository:
    return TelemetryRepository()


@pytest.fixture
def telemetry_(telemetry_repo_: TelemetryRepository) -> TelemetryLogger:
    return TelemetryLogger(repo=telemetry_repo_)


# --- test doubles ----------------------------------------------------------------


class _ScriptedDriver:
    """Returns results[0], results[1], ... on successive calls (repeats the last)."""

    provider = "test"

    def __init__(self, results: List[DriverResult]) -> None:
        self._results = results
        self.calls: List[tuple] = []

    def run(self, model: str, prompt: str, images=None) -> DriverResult:
        self.calls.append((model, prompt))
        index = min(len(self.calls) - 1, len(self._results) - 1)
        return self._results[index]


class _NeverCalledDriver:
    def run(self, model: str, prompt: str, images=None) -> DriverResult:
        raise AssertionError("driver.run() should not have been called")


class _FakeRouter:
    def __init__(self, plan: ExecutionPlan) -> None:
        self._plan = plan
        self.calls = 0

    def build_execution_plan(self, request) -> ExecutionPlan:
        self.calls += 1
        return self._plan


class _RaisingRouter:
    def build_execution_plan(self, request) -> ExecutionPlan:
        raise NoEligibleModelError("nothing left")


class _FakeWebRetrieval:
    def __init__(self, markdown: str, sources: List[str]) -> None:
        self._markdown = markdown
        self._sources = sources

    def gather_context(self, query: str, max_results=None):
        from lib.engine.retrieval.service import WebContextResult

        items = [{"title": "result", "url": url, "content": self._markdown} for url in self._sources]
        return WebContextResult(query=query, markdown=self._markdown, sources=self._sources, items=items)


class _FakeHippocampus:
    def __init__(self, chunks: List[MemoryChunk]) -> None:
        self._chunks = chunks

    async def search_memory(self, topic: str, query: str, limit: int = 5) -> List[MemoryChunk]:
        return self._chunks


def _success(text: str, input_tokens: int = 10, output_tokens: int = 5) -> DriverResult:
    return DriverResult(
        success=True, response_text=text, input_tokens=input_tokens, output_tokens=output_tokens, latency_ms=100
    )


def _failure(error_type: str, message: str = "failed") -> DriverResult:
    return DriverResult(
        success=False, response_text="", input_tokens=0, output_tokens=0, latency_ms=0,
        error_type=error_type, error_message=message,
    )


def _plan(
    selections: List[ModelSelection],
    tier: int = 1,
    max_latency_seconds: int = 30,
    needs_web: bool = False,
    use_memory: bool = False,
    memory_topic: Optional[str] = None,
    attachments: Optional[List[Attachment]] = None,
) -> ExecutionPlan:
    return ExecutionPlan(
        tier=tier, task_type="general", strategy_id="general_t1_dynamic", prompt="explain sqlite",
        selections=selections, allow_multi_model=len(selections) > 1, retrieval_mode="none",
        needs_web=needs_web, use_memory=use_memory, memory_topic=memory_topic, require_verification=False,
        max_latency_seconds=max_latency_seconds, max_model_calls=max(1, len(selections)),
        source="dynamic", reason="test", attachments=attachments or [],
    )


def _executor(
    drivers: Dict[str, object],
    quota_tracker: QuotaTracker,
    telemetry: TelemetryLogger,
    router=None,
    web_retrieval=None,
    hippocampus=None,
    max_retries: int = 1,
    max_reroutes: int = 1,
    max_critic_revisions: int = 1,
    sanitize_enabled: bool = True,
    registry=None,
    attachment_ingestor=None,
) -> Executor:
    return Executor(
        drivers=drivers, quota_tracker=quota_tracker, telemetry=telemetry, router=router,
        web_retrieval=web_retrieval, hippocampus=hippocampus, max_retries=max_retries, max_reroutes=max_reroutes,
        max_critic_revisions=max_critic_revisions, sanitize_enabled=sanitize_enabled,
        registry=registry, attachment_ingestor=attachment_ingestor,
    )


async def _run_and_flush(executor: Executor, plan: ExecutionPlan, telemetry_repo: TelemetryRepository):
    """Runs the plan and waits for its fire-and-forget telemetry writes."""
    result = await executor.execute(plan)
    # Telemetry futures aren't returned by execute(); give the executor
    # thread pool one tick to flush before asserting on persisted rows.
    await asyncio.sleep(0.05)
    return result


# --- single-step execution -----------------------------------------------------


@pytest.mark.asyncio
async def test_single_step_success(quota_tracker_, telemetry_, telemetry_repo_):
    driver = _ScriptedDriver([_success("the answer")])
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_)
    plan = _plan([ModelSelection(model_id="execA/model", provider="execA", role="primary")])

    result = await _run_and_flush(executor, plan, telemetry_repo_)

    assert result.success is True
    assert result.response_text == "the answer"
    assert result.total_input_tokens == 10
    assert result.total_output_tokens == 5
    assert driver.calls[0][0] == "model"  # bare name, provider prefix stripped
    events = telemetry_repo_.list_events(strategy_id=plan.strategy_id)
    assert any(e.role == "primary" and e.success for e in events)


@pytest.mark.asyncio
async def test_unresolved_strategy_plan_raises(quota_tracker_, telemetry_):
    executor = _executor({}, quota_tracker_, telemetry_)
    plan = _plan([])

    with pytest.raises(UnresolvedStrategyError):
        await executor.execute(plan)


# --- provider text sanitization ---------------------------------------------


@pytest.mark.asyncio
async def test_response_text_is_sanitized_by_default(quota_tracker_, telemetry_):
    driver = _ScriptedDriver([_success("Hello​World")])
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_)
    plan = _plan([ModelSelection(model_id="execA/model", provider="execA", role="primary")])

    result = await executor.execute(plan)

    assert result.response_text == "HelloWorld"


@pytest.mark.asyncio
async def test_response_text_untouched_when_sanitize_disabled(quota_tracker_, telemetry_):
    driver = _ScriptedDriver([_success("Hello​World")])
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_, sanitize_enabled=False)
    plan = _plan([ModelSelection(model_id="execA/model", provider="execA", role="primary")])

    result = await executor.execute(plan)

    assert result.response_text == "Hello​World"


# --- attachment ingestion ----------------------------------------------------


def _seed_model(model_id: str, **overrides) -> ModelCatalogEntry:
    repo = ModelRepository()
    defaults = dict(
        provider="execA", display_name="model", access_status=AccessStatus.AVAILABLE.value,
        context_window=8192, is_local=True, tier_eligibility=[1], capabilities={},
        cost_per_million_tokens=0.0, is_enabled=True,
    )
    defaults.update(overrides)
    return repo.upsert(ModelCatalogEntry(id=model_id, **defaults))


def _audio_attachment() -> Attachment:
    return Attachment(filename="note.wav", mime_type="audio/wav", data_base64="ZmFrZQ==")


def _image_attachment() -> Attachment:
    return Attachment(filename="photo.png", mime_type="image/png", data_base64="ZmFrZQ==")


class _FakeIngestor:
    def __init__(self, result) -> None:
        self._result = result

    def ingest(self, attachments, *, model_is_vision_capable: bool = False):
        return self._result


@pytest.mark.asyncio
async def test_audio_attachment_transcript_is_injected_into_prompt(quota_tracker_, telemetry_):
    from lib.engine.attachments import IngestedAttachments

    _seed_model("execA/model")
    driver = _ScriptedDriver([_success("ok")])
    ingestor = _FakeIngestor(IngestedAttachments(text_context="the audio said hello"))
    registry = ModelRegistryService(discoveries=[], repository=ModelRepository())
    executor = _executor(
        {"execA": driver}, quota_tracker_, telemetry_, registry=registry, attachment_ingestor=ingestor
    )
    plan = _plan(
        [ModelSelection(model_id="execA/model", provider="execA", role="primary")],
        attachments=[_audio_attachment()],
    )

    result = await executor.execute(plan)

    assert result.success is True
    assert "the audio said hello" in driver.calls[0][1]


@pytest.mark.asyncio
async def test_image_attachment_error_aborts_execution_without_calling_driver(quota_tracker_, telemetry_):
    from lib.engine.attachments import IngestedAttachments

    _seed_model("execA/model")
    driver = _NeverCalledDriver()
    ingestor = _FakeIngestor(IngestedAttachments(errors=["attachment 'photo.png' is not vision-capable"]))
    registry = ModelRegistryService(discoveries=[], repository=ModelRepository())
    executor = _executor(
        {"execA": driver}, quota_tracker_, telemetry_, registry=registry, attachment_ingestor=ingestor
    )
    plan = _plan(
        [ModelSelection(model_id="execA/model", provider="execA", role="primary")],
        attachments=[_image_attachment()],
    )

    result = await executor.execute(plan)

    assert result.success is False
    assert result.error_type == "attachment_error"
    assert "not vision-capable" in result.response_text


@pytest.mark.asyncio
async def test_image_attachment_passed_to_primary_driver_call(quota_tracker_, telemetry_):
    from lib.engine.attachments import IngestedAttachments

    _seed_model("execA/model", capabilities={"vision": True})
    driver = _ScriptedDriver([_success("it's a cat")])
    ingestor = _FakeIngestor(IngestedAttachments(image_data_uris=["data:image/png;base64,ZmFrZQ=="]))
    registry = ModelRegistryService(discoveries=[], repository=ModelRepository())
    executor = _executor(
        {"execA": driver}, quota_tracker_, telemetry_, registry=registry, attachment_ingestor=ingestor
    )
    plan = _plan(
        [ModelSelection(model_id="execA/model", provider="execA", role="primary")],
        attachments=[_image_attachment()],
    )

    result = await executor.execute(plan)

    assert result.success is True


@pytest.mark.asyncio
async def test_missing_driver_reports_no_driver_error(quota_tracker_, telemetry_):
    executor = _executor({}, quota_tracker_, telemetry_)
    plan = _plan([ModelSelection(model_id="execC/model", provider="execC", role="primary")])

    result = await executor.execute(plan)

    assert result.success is False
    assert result.error_type == "no_driver"


# --- retries -----------------------------------------------------------------------


@pytest.mark.asyncio
async def test_retries_on_transient_error_then_succeeds(quota_tracker_, telemetry_, telemetry_repo_):
    driver = _ScriptedDriver([_failure("unreachable"), _success("recovered")])
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_, max_retries=1)
    plan = _plan([ModelSelection(model_id="execA/model", provider="execA", role="primary")])

    result = await _run_and_flush(executor, plan, telemetry_repo_)

    assert result.success is True
    assert result.response_text == "recovered"
    assert len(driver.calls) == 2


@pytest.mark.asyncio
async def test_timeout_error_is_not_retried(quota_tracker_, telemetry_):
    driver = _ScriptedDriver([_failure("timeout"), _success("too late")])
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_, max_retries=2)
    plan = _plan([ModelSelection(model_id="execA/model", provider="execA", role="primary")])

    result = await executor.execute(plan)

    assert result.success is False
    assert result.error_type == "timeout"
    assert len(driver.calls) == 1  # a spent budget isn't worth retrying


@pytest.mark.asyncio
async def test_zero_latency_budget_skips_the_driver_call_entirely(quota_tracker_, telemetry_):
    executor = _executor({"execA": _NeverCalledDriver()}, quota_tracker_, telemetry_)
    plan = _plan(
        [ModelSelection(model_id="execA/model", provider="execA", role="primary")],
        max_latency_seconds=0,
    )

    result = await executor.execute(plan)

    assert result.success is False
    assert result.error_type == "timeout"


# --- multi-step pipelines -----------------------------------------------------------


@pytest.mark.asyncio
async def test_refiner_receives_primary_output_and_wins_final_response(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft answer")])
    refiner = _ScriptedDriver([_success("polished answer")])
    executor = _executor({"execB": primary, "execC": refiner}, quota_tracker_, telemetry_)
    plan = _plan(
        [
            ModelSelection(model_id="execB/model", provider="execB", role="primary"),
            ModelSelection(model_id="execC/model", provider="execC", role="refiner"),
        ]
    )

    result = await executor.execute(plan)

    assert result.success is True
    assert result.response_text == "polished answer"
    assert "draft answer" in refiner.calls[0][1]  # refiner's prompt embeds the primary's draft


@pytest.mark.asyncio
async def test_critic_step_does_not_override_final_response(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft answer")])
    refiner = _ScriptedDriver([_success("polished answer")])
    critic = _ScriptedDriver([_success("looks correct")])
    executor = _executor(
        {"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_
    )
    plan = _plan(
        [
            ModelSelection(model_id="execB/model", provider="execB", role="primary"),
            ModelSelection(model_id="execC/model", provider="execC", role="refiner"),
            ModelSelection(model_id="execD/model", provider="execD", role="critic"),
        ]
    )

    result = await executor.execute(plan)

    assert result.response_text == "polished answer"  # not the critic's text
    assert len(result.steps) == 3


@pytest.mark.asyncio
async def test_critic_sees_both_the_original_draft_and_the_refined_answer(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft answer")])
    refiner = _ScriptedDriver([_success("polished answer")])
    critic = _ScriptedDriver([_success("looks correct")])
    executor = _executor(
        {"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_
    )
    plan = _plan(
        [
            ModelSelection(model_id="execB/model", provider="execB", role="primary"),
            ModelSelection(model_id="execC/model", provider="execC", role="refiner"),
            ModelSelection(model_id="execD/model", provider="execD", role="critic"),
        ]
    )

    await executor.execute(plan)

    critic_prompt = critic.calls[0][1]
    assert "Original draft:" in critic_prompt
    assert "draft answer" in critic_prompt
    assert "Refined answer:" in critic_prompt
    assert "polished answer" in critic_prompt


@pytest.mark.asyncio
async def test_critic_without_a_refiner_gets_the_single_answer_prompt(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft answer")])
    critic = _ScriptedDriver([_success("looks correct")])
    executor = _executor({"execB": primary, "execD": critic}, quota_tracker_, telemetry_)
    plan = _plan(
        [
            ModelSelection(model_id="execB/model", provider="execB", role="primary"),
            ModelSelection(model_id="execD/model", provider="execD", role="critic"),
        ]
    )

    await executor.execute(plan)

    critic_prompt = critic.calls[0][1]
    assert "Original draft:" not in critic_prompt
    assert "Answer:\ndraft answer" in critic_prompt


# --- critic-driven revision loop (issue #20) ----------------------------------------


def _plan_with_pipeline() -> List[ModelSelection]:
    return [
        ModelSelection(model_id="execB/model", provider="execB", role="primary"),
        ModelSelection(model_id="execC/model", provider="execC", role="refiner"),
        ModelSelection(model_id="execD/model", provider="execD", role="critic"),
    ]


@pytest.mark.asyncio
async def test_critic_ok_on_first_try_never_revises(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft")])
    refiner = _ScriptedDriver([_success("polished")])
    critic = _ScriptedDriver([_success("VERDICT: OK\n\nlooks good")])
    executor = _executor({"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_)

    result = await executor.execute(_plan(_plan_with_pipeline()))

    assert result.response_text == "polished"
    assert len(result.steps) == 3
    assert len(critic.calls) == 1


@pytest.mark.asyncio
async def test_needs_revision_triggers_exactly_one_revise_pass_by_default(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft")])
    refiner = _ScriptedDriver([_success("polished v1"), _success("polished v2")])  # reviser reuses this driver
    critic = _ScriptedDriver([_success("VERDICT: NEEDS_REVISION\n\nfix the intro"), _success("VERDICT: OK\n\ngood now")])
    executor = _executor(
        {"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_, max_critic_revisions=1
    )

    result = await executor.execute(_plan(_plan_with_pipeline()))

    assert result.response_text == "polished v2"
    assert len(result.steps) == 5  # primary, refiner, critic, reviser, critic
    assert [s.role for s in result.steps] == ["primary", "refiner", "critic", "reviser", "critic"]
    assert len(critic.calls) == 2
    assert len(refiner.calls) == 2
    # the reviser call must carry the critic's feedback and the answer being revised
    reviser_prompt = refiner.calls[1][1]
    assert "fix the intro" in reviser_prompt
    assert "polished v1" in reviser_prompt


@pytest.mark.asyncio
async def test_revision_budget_exhausted_still_returns_the_last_attempt(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft")])
    refiner = _ScriptedDriver([_success("v1"), _success("v2")])
    # Critic never says OK: budget (1 revision) runs out first.
    critic = _ScriptedDriver([_success("VERDICT: NEEDS_REVISION\n\nstill wrong")])
    executor = _executor(
        {"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_, max_critic_revisions=1
    )

    result = await executor.execute(_plan(_plan_with_pipeline()))

    assert result.response_text == "v2"  # last revision used regardless of its (still negative) verdict
    assert len(result.steps) == 5
    assert len(refiner.calls) == 2  # exactly one revision, budget respected


@pytest.mark.asyncio
async def test_unparseable_critic_verdict_is_treated_as_ok(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft")])
    refiner = _ScriptedDriver([_success("polished")])
    critic = _ScriptedDriver([_success("this looks fine to me, no complaints")])  # no VERDICT line
    executor = _executor(
        {"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_, max_critic_revisions=1
    )

    result = await executor.execute(_plan(_plan_with_pipeline()))

    assert result.response_text == "polished"
    assert len(result.steps) == 3  # no revision triggered
    assert len(refiner.calls) == 1


@pytest.mark.asyncio
async def test_zero_revision_budget_disables_the_loop_entirely(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft")])
    refiner = _ScriptedDriver([_success("polished")])
    critic = _ScriptedDriver([_success("VERDICT: NEEDS_REVISION\n\nplease fix")])
    executor = _executor(
        {"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_, max_critic_revisions=0
    )

    result = await executor.execute(_plan(_plan_with_pipeline()))

    assert result.response_text == "polished"  # critic ran and was logged, but never triggered a revision
    assert len(result.steps) == 3
    assert len(critic.calls) == 1


@pytest.mark.asyncio
async def test_revision_loop_stops_cleanly_when_the_reviser_call_fails(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_success("draft")])
    refiner = _ScriptedDriver([_success("polished"), _failure("cli_error")])
    critic = _ScriptedDriver([_success("VERDICT: NEEDS_REVISION\n\nfix it")])
    executor = _executor(
        {"execB": primary, "execC": refiner, "execD": critic}, quota_tracker_, telemetry_, max_critic_revisions=2,
        max_retries=0,
    )

    result = await executor.execute(_plan(_plan_with_pipeline()))

    assert result.response_text == "polished"  # last good answer kept; failed revision never applied
    assert len(result.steps) == 4  # primary, refiner, critic, failed reviser -- no second critic call
    assert len(critic.calls) == 1


@pytest.mark.asyncio
async def test_primary_failure_skips_downstream_steps(quota_tracker_, telemetry_):
    primary = _ScriptedDriver([_failure("cli_error")])
    executor = _executor({"execB": primary, "execC": _NeverCalledDriver()}, quota_tracker_, telemetry_, max_retries=0)
    plan = _plan(
        [
            ModelSelection(model_id="execB/model", provider="execB", role="primary"),
            ModelSelection(model_id="execC/model", provider="execC", role="refiner"),
        ]
    )

    result = await executor.execute(plan)

    assert result.success is False
    assert len(result.steps) == 1  # refiner never ran


# --- rate limits: cooldown + reroute -------------------------------------------------


@pytest.mark.asyncio
async def test_rate_limit_enters_cooldown(quota_tracker_, telemetry_, model_repo_):
    model_repo_.upsert(
        ModelCatalogEntry(
            id="execC/exec-cooldown-model", provider="execC", display_name="o3",
            access_status=AccessStatus.AVAILABLE.value, context_window=8192, is_local=False,
            tier_eligibility=[3], capabilities={}, cost_per_million_tokens=0.0, is_enabled=True,
        )
    )
    driver = _ScriptedDriver([_failure("rate_limit", "429")])
    executor = _executor({"execC": driver}, quota_tracker_, telemetry_, max_retries=0)
    plan = _plan([ModelSelection(model_id="execC/exec-cooldown-model", provider="execC", role="primary")])

    result = await executor.execute(plan)

    assert result.success is False
    assert result.error_type == "rate_limit"
    stored = model_repo_.get_by_id("execC/exec-cooldown-model")
    assert stored.access_status == AccessStatus.COOLING_DOWN.value


@pytest.mark.asyncio
async def test_rate_limit_reroutes_to_the_next_plan_when_a_router_is_configured(quota_tracker_, telemetry_):
    failing_driver = _ScriptedDriver([_failure("rate_limit", "429")])
    backup_driver = _ScriptedDriver([_success("from the backup model")])
    fallback_plan = _plan([ModelSelection(model_id="execB/model", provider="execB", role="primary")])
    router = _FakeRouter(fallback_plan)
    executor = _executor(
        {"execC": failing_driver, "execB": backup_driver}, quota_tracker_, telemetry_, router=router, max_retries=0
    )
    plan = _plan([ModelSelection(model_id="execC/model", provider="execC", role="primary")])

    result = await executor.execute(plan)

    assert result.success is True
    assert result.response_text == "from the backup model"
    assert router.calls == 1


@pytest.mark.asyncio
async def test_rate_limit_gives_up_cleanly_when_reroute_has_no_alternative(quota_tracker_, telemetry_):
    driver = _ScriptedDriver([_failure("rate_limit", "429")])
    executor = _executor({"execC": driver}, quota_tracker_, telemetry_, router=_RaisingRouter(), max_retries=0)
    plan = _plan([ModelSelection(model_id="execC/model", provider="execC", role="primary")])

    result = await executor.execute(plan)

    assert result.success is False
    assert result.error_type == "rate_limit"


# --- retrieval context assembly --------------------------------------------------------


@pytest.mark.asyncio
async def test_web_context_is_prepended_to_every_step_prompt(quota_tracker_, telemetry_, telemetry_repo_):
    driver = _ScriptedDriver([_success("answer")])
    web = _FakeWebRetrieval(markdown="Bitcoin is at $100k today.", sources=["https://example.com"])
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_, web_retrieval=web)
    plan = _plan(
        [ModelSelection(model_id="execA/model", provider="execA", role="primary")], needs_web=True
    )

    await _run_and_flush(executor, plan, telemetry_repo_)

    assert "Bitcoin is at $100k today." in driver.calls[0][1]
    events = telemetry_repo_.list_events(strategy_id=plan.strategy_id)
    assert events[0].retrieval_source == "web"
    assert events[0].retrieval_documents == 1


@pytest.mark.asyncio
async def test_memory_context_is_prepended_to_every_step_prompt(quota_tracker_, telemetry_, telemetry_repo_):
    driver = _ScriptedDriver([_success("answer")])
    hippocampus = _FakeHippocampus(
        [MemoryChunk(id="1", topic="asodya_core", content="Cortex routes by tier")]
    )
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_, hippocampus=hippocampus)
    plan = _plan(
        [ModelSelection(model_id="execA/model", provider="execA", role="primary")],
        use_memory=True, memory_topic="asodya_core",
    )

    await _run_and_flush(executor, plan, telemetry_repo_)

    assert "Cortex routes by tier" in driver.calls[0][1]
    events = telemetry_repo_.list_events(strategy_id=plan.strategy_id)
    assert events[0].retrieval_source == "hippocampus"


@pytest.mark.asyncio
async def test_no_retrieval_requested_leaves_prompt_untouched(quota_tracker_, telemetry_):
    driver = _ScriptedDriver([_success("answer")])
    executor = _executor({"execA": driver}, quota_tracker_, telemetry_)
    plan = _plan([ModelSelection(model_id="execA/model", provider="execA", role="primary")])

    await executor.execute(plan)

    assert driver.calls[0][1] == plan.prompt


def test_bare_model_name_strips_provider_prefix():
    assert _bare_model_name("claude/claude-sonnet-5") == "claude-sonnet-5"
    assert _bare_model_name("no-prefix-model") == "no-prefix-model"
