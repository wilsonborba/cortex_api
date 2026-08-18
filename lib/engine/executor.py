from __future__ import annotations

import asyncio
import uuid
from dataclasses import dataclass, field, replace
from datetime import datetime, timezone
from time import monotonic
from typing import Dict, List, Optional

from lib.core.logs import get_logger
from lib.core.settings import Settings, get_settings
from lib.dal.models import TelemetryEvent
from lib.engine.drivers.agy_docker import AgyDockerDriver
from lib.engine.drivers.base import DriverResult, ExecutionDriver
from lib.engine.drivers.claude_docker import ClaudeDockerDriver
from lib.engine.drivers.codex import CodexDriver
from lib.engine.drivers.ollama import OllamaDriver
from lib.engine.quota import QuotaTracker
from lib.engine.retrieval.hippocampus import HippocampusClient, build_default_hippocampus_client, format_memory_context
from lib.engine.retrieval.service import WebRetrievalService, build_default_web_retrieval_service
from lib.engine.router import ExecutionPlan, ModelSelection, NoEligibleModelError, Router, RoutingRequest, build_default_router
from lib.engine.telemetry import TelemetryLogger

logger = get_logger(__name__)

# Worth one quick retry: plumbing hiccups, not "this will never work".
_RETRYABLE_ERRORS = {"unreachable", "http_error", "cli_error"}


@dataclass(frozen=True)
class StepResult:
    role: str
    provider: str
    model_id: str
    success: bool
    response_text: str
    input_tokens: int
    output_tokens: int
    latency_ms: int
    cost_usd: float
    error_type: Optional[str]
    error_message: Optional[str]
    attempts: int


@dataclass(frozen=True)
class RetrievalStats:
    source: str  # "none" | "web" | "hippocampus" | "hybrid"
    documents: int
    latency_ms: int


@dataclass(frozen=True)
class ExecutionResult:
    request_id: str
    tier_requested: int
    tier_executed: int
    strategy_id: str
    task_type: str
    success: bool
    response_text: str
    steps: List[StepResult] = field(default_factory=list)
    total_input_tokens: int = 0
    total_output_tokens: int = 0
    total_cost_usd: float = 0.0
    latency_ms: int = 0
    error_type: Optional[str] = None


class UnresolvedStrategyError(RuntimeError):
    """A plan named a `strategy_id` (force_strategy / strategy pin) but no
    concrete model sequence: there's no strategy registry yet to resolve one
    from, that's a deliberate gap left open in #8 for a future issue."""

    def __init__(self, strategy_id: str) -> None:
        super().__init__(
            f"strategy_id={strategy_id!r} has no concrete model selections to run "
            "(force_strategy/strategy-pin plans aren't resolvable yet)"
        )
        self.strategy_id = strategy_id


class Executor:
    """Runs an `ExecutionPlan` (from the Router, issue #8): single-step for
    T0-T2, generator -> refiner -> critic for T3-T5, whatever the plan's
    `selections` actually contain.

    Retrieval (web + memory) is assembled here, once per request, and
    prepended to the prompt every step sees: it's the "optional RAG" stage
    of the pipeline described in docs/specs.md, right before the model call.
    """

    def __init__(
        self,
        drivers: Dict[str, ExecutionDriver],
        quota_tracker: QuotaTracker,
        telemetry: Optional[TelemetryLogger] = None,
        router: Optional[Router] = None,
        web_retrieval: Optional[WebRetrievalService] = None,
        hippocampus: Optional[HippocampusClient] = None,
        max_retries: int = 1,
        max_reroutes: int = 1,
    ) -> None:
        self._drivers = drivers
        self._quota_tracker = quota_tracker
        self._telemetry = telemetry or TelemetryLogger()
        self._router = router
        self._web_retrieval = web_retrieval
        self._hippocampus = hippocampus
        self._max_retries = max_retries
        self._max_reroutes = max_reroutes

    async def execute(self, plan: ExecutionPlan) -> ExecutionResult:
        if not plan.selections:
            raise UnresolvedStrategyError(plan.strategy_id)

        retrieval, context_prefix = await self._gather_context(plan)
        if context_prefix:
            plan = replace(plan, prompt=f"{context_prefix}\n\n{plan.prompt}")

        return await self._execute_plan(plan, retrieval, reroutes_left=self._max_reroutes)

    # -- retrieval assembly ---------------------------------------------------

    async def _gather_context(self, plan: ExecutionPlan) -> tuple[RetrievalStats, str]:
        if not plan.needs_web and not plan.use_memory:
            return RetrievalStats("none", 0, 0), ""

        started = monotonic()
        parts: List[str] = []
        documents = 0
        used_web = False
        used_memory = False

        if plan.needs_web and self._web_retrieval is not None:
            try:
                loop = asyncio.get_running_loop()
                web_result = await loop.run_in_executor(None, self._web_retrieval.gather_context, plan.prompt)
                if web_result.markdown:
                    parts.append(f"## Web context\n\n{web_result.markdown}")
                    documents += len(web_result.sources)
                    used_web = True
            except Exception as exc:
                logger.warning("web retrieval failed for this request: %s", exc)

        if plan.use_memory and self._hippocampus is not None:
            try:
                topic = plan.memory_topic or plan.task_type
                chunks = await self._hippocampus.search_memory(topic, plan.prompt)
                memory_md = format_memory_context(chunks)
                if memory_md:
                    parts.append(f"## Memory context\n\n{memory_md}")
                    documents += len(chunks)
                    used_memory = True
            except Exception as exc:
                logger.warning("memory retrieval failed for this request: %s", exc)

        source = "hybrid" if used_web and used_memory else "web" if used_web else "hippocampus" if used_memory else "none"
        latency_ms = int((monotonic() - started) * 1000)
        return RetrievalStats(source, documents, latency_ms), "\n\n".join(parts)

    # -- pipeline execution -----------------------------------------------------

    async def _execute_plan(
        self, plan: ExecutionPlan, retrieval: RetrievalStats, reroutes_left: int
    ) -> ExecutionResult:
        request_id = str(uuid.uuid4())
        deadline = monotonic() + plan.max_latency_seconds
        steps: List[StepResult] = []
        context = plan.prompt

        for selection in plan.selections:
            step = await self._run_step(plan, selection, context, deadline)
            steps.append(step)
            self._log_step(plan, request_id, selection, step, retrieval)

            if selection.role != "primary":
                continue
            if step.success:
                context = step.response_text or context
                continue

            # Primary failed: refining/critiquing a failure serves no purpose.
            if step.error_type == "rate_limit":
                self._quota_tracker.enter_cooldown(selection.model_id, reason=step.error_message)
                if self._router is not None and reroutes_left > 0:
                    rerouted = self._try_reroute(plan)
                    if rerouted is not None:
                        return await self._execute_plan(rerouted, retrieval, reroutes_left - 1)
            break

        return self._build_result(plan, request_id, steps)

    def _try_reroute(self, plan: ExecutionPlan) -> Optional[ExecutionPlan]:
        assert self._router is not None
        try:
            return self._router.build_execution_plan(
                RoutingRequest(
                    prompt=plan.prompt,
                    tier=plan.tier,
                    task_type=plan.task_type,
                    needs_web=False,  # already gathered for this request; don't redo it
                    use_memory=False,
                    memory_topic=plan.memory_topic,
                )
            )
        except NoEligibleModelError:
            return None

    async def _run_step(
        self, plan: ExecutionPlan, selection: ModelSelection, context: str, deadline: float
    ) -> StepResult:
        driver = self._drivers.get(selection.provider)
        if driver is None:
            return StepResult(
                role=selection.role, provider=selection.provider, model_id=selection.model_id,
                success=False, response_text="", input_tokens=0, output_tokens=0, latency_ms=0, cost_usd=0.0,
                error_type="no_driver", error_message=f"no execution driver registered for {selection.provider!r}",
                attempts=0,
            )

        prompt = _build_role_prompt(selection.role, plan.prompt, context)
        bare_model = _bare_model_name(selection.model_id)
        loop = asyncio.get_running_loop()

        attempts = 0
        result: Optional[DriverResult] = None
        while attempts < max(1, self._max_retries + 1):
            attempts += 1
            remaining = deadline - monotonic()
            if remaining <= 0:
                result = DriverResult(
                    success=False, response_text="", input_tokens=0, output_tokens=0, latency_ms=0,
                    error_type="timeout", error_message="tier latency budget exhausted before this step ran",
                )
                break
            try:
                result = await asyncio.wait_for(
                    loop.run_in_executor(None, driver.run, bare_model, prompt), timeout=remaining
                )
            except asyncio.TimeoutError:
                # The thread may still be running underneath (drivers use
                # blocking subprocess/HTTP calls); its result is simply
                # discarded once the tier's latency budget is spent.
                result = DriverResult(
                    success=False, response_text="", input_tokens=0, output_tokens=0, latency_ms=0,
                    error_type="timeout", error_message=f"exceeded {remaining:.1f}s remaining budget",
                )
                break

            self._quota_tracker.record_execution(selection.provider, selection.model_id, result)
            if result.success or result.error_type not in _RETRYABLE_ERRORS:
                break

        assert result is not None
        return StepResult(
            role=selection.role, provider=selection.provider, model_id=selection.model_id,
            success=result.success, response_text=result.response_text,
            input_tokens=result.input_tokens, output_tokens=result.output_tokens,
            latency_ms=result.latency_ms, cost_usd=result.cost_usd,
            error_type=result.error_type, error_message=result.error_message, attempts=attempts,
        )

    # -- result assembly -----------------------------------------------------

    def _log_step(
        self,
        plan: ExecutionPlan,
        request_id: str,
        selection: ModelSelection,
        step: StepResult,
        retrieval: RetrievalStats,
    ) -> None:
        now = datetime.now(timezone.utc)
        event = TelemetryEvent(
            request_id=request_id,
            execution_id=str(uuid.uuid4()),
            strategy_id=plan.strategy_id,
            tier_requested=plan.tier,
            tier_executed=plan.tier,
            task_type=plan.task_type,
            provider=step.provider,
            model=step.model_id,  # full catalog id, e.g. "claude/claude-sonnet-5"
            role=step.role,
            input_tokens=step.input_tokens,
            output_tokens=step.output_tokens,
            total_tokens=step.input_tokens + step.output_tokens,
            started_at=now,
            finished_at=now,
            latency_seconds=step.latency_ms / 1000.0,
            latency_ms=step.latency_ms,
            success=step.success,
            error_type=step.error_type,
            retrieval_source=retrieval.source,
            retrieval_documents=retrieval.documents,
            retrieval_latency_ms=retrieval.latency_ms,
            estimated_cost_usd=step.cost_usd,
        )
        self._telemetry.record_async(event)

    def _build_result(self, plan: ExecutionPlan, request_id: str, steps: List[StepResult]) -> ExecutionResult:
        primary = steps[0] if steps else None
        success = bool(primary and primary.success)
        final_text = next(
            (s.response_text for s in reversed(steps) if s.success and s.role in ("primary", "refiner")), ""
        )
        return ExecutionResult(
            request_id=request_id,
            tier_requested=plan.tier,
            tier_executed=plan.tier,
            strategy_id=plan.strategy_id,
            task_type=plan.task_type,
            success=success,
            response_text=final_text,
            steps=steps,
            total_input_tokens=sum(s.input_tokens for s in steps),
            total_output_tokens=sum(s.output_tokens for s in steps),
            total_cost_usd=sum(s.cost_usd for s in steps),
            latency_ms=sum(s.latency_ms for s in steps),
            error_type=None if success else (primary.error_type if primary else "no_steps_executed"),
        )


def _bare_model_name(model_id: str) -> str:
    """`ModelCatalogEntry.id` is `"provider/name"`; drivers want just `name`."""
    return model_id.split("/", 1)[1] if "/" in model_id else model_id


def _build_role_prompt(role: str, original_prompt: str, context: str) -> str:
    if role == "primary":
        return original_prompt
    if role == "refiner":
        return (
            f"{original_prompt}\n\n---\nA first draft answer was produced below. "
            f"Refine it: fix mistakes, tighten the reasoning, improve clarity. "
            f"Return only the improved answer.\n\nDraft:\n{context}"
        )
    if role == "critic":
        return (
            f"{original_prompt}\n\n---\nReview the answer below for correctness and completeness. "
            f"List concrete issues, or state that it's correct.\n\nAnswer:\n{context}"
        )
    return original_prompt


def build_default_drivers(settings: Optional[Settings] = None) -> Dict[str, ExecutionDriver]:
    settings = settings or get_settings()
    return {
        "ollama": OllamaDriver(base_url=settings.ollama_base_url, timeout=settings.driver_timeout_seconds),
        "claude": ClaudeDockerDriver(command=settings.claude_docker_command, timeout=settings.driver_timeout_seconds),
        "agy": AgyDockerDriver(command=settings.agy_docker_command, timeout=settings.driver_timeout_seconds),
        "codex": CodexDriver(command=settings.codex_command, timeout=settings.driver_timeout_seconds),
    }


def build_default_executor(settings: Optional[Settings] = None) -> Executor:
    settings = settings or get_settings()
    quota_tracker = QuotaTracker(settings=settings)
    return Executor(
        drivers=build_default_drivers(settings=settings),
        quota_tracker=quota_tracker,
        telemetry=TelemetryLogger(),
        router=build_default_router(settings=settings),
        web_retrieval=build_default_web_retrieval_service(settings=settings),
        hippocampus=build_default_hippocampus_client(settings=settings),
        max_retries=settings.executor_max_retries,
        max_reroutes=settings.executor_max_reroutes,
    )
