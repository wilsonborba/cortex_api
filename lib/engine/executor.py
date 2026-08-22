from __future__ import annotations

import asyncio
import functools
import re
import uuid
from dataclasses import dataclass, field, replace
from datetime import datetime, timezone
from time import monotonic
from typing import Any, Dict, List, Optional

from lib.core.logs import get_logger
from lib.core.settings import Settings, get_settings
from lib.dal.models import ModelCatalogEntry, TelemetryEvent
from lib.engine.context_format import has_active_context_format_pin, resolve_context_format
from lib.engine.drivers.agy_docker import AgyDockerDriver
from lib.engine.drivers.aion_labs import AionLabsDriver
from lib.engine.drivers.base import DriverResult, ExecutionDriver
from lib.engine.drivers.claude_docker import ClaudeDockerDriver
from lib.engine.drivers.cloudflare import CloudflareDriver
from lib.engine.drivers.codex import CodexDriver
from lib.engine.drivers.cohere import CohereDriver
from lib.engine.drivers.google_ai_studio import GoogleAIStudioDriver
from lib.engine.drivers.groq import GroqDriver
from lib.engine.drivers.huggingface import HuggingFaceDriver
from lib.engine.drivers.inference_net import InferenceNetDriver
from lib.engine.drivers.mistral import MistralDriver
from lib.engine.drivers.nvidia import NvidiaDriver
from lib.engine.drivers.ollama import OllamaDriver
from lib.engine.drivers.ollama_cloud import OllamaCloudDriver
from lib.engine.drivers.openrouter import OpenRouterDriver
from lib.engine.drivers.requesty import RequestyDriver
from lib.engine.drivers.sambanova import SambaNovaDriver
from lib.engine.drivers.siliconflow import SiliconFlowDriver
from lib.engine.drivers.zai import ZaiDriver
from lib.engine.format import JSON, TOON, encode
from lib.engine.quota import QuotaTracker
from lib.engine.registry_service import ModelRegistryService, build_default_registry_service
from lib.engine.retrieval.hippocampus import HippocampusClient, build_default_hippocampus_client
from lib.engine.retrieval.service import WebRetrievalService, build_default_web_retrieval_service
from lib.engine.router import ExecutionPlan, ModelSelection, NoEligibleModelError, Router, RoutingRequest, build_default_router
from lib.engine.telemetry import TelemetryLogger
from lib.core.text_sanitize import sanitize_text
from lib.engine.attachments import AttachmentIngestor, IngestedAttachments, build_default_attachment_ingestor

logger = get_logger(__name__)

# Worth one quick retry: plumbing hiccups, not "this will never work".
_RETRYABLE_ERRORS = {
    "unreachable",
    "http_error",
    "cli_error",
    "rate_limit",
    "rate_limit_exceeded",
    "provider_server_error",
    "provider_timeout",
    "empty_response",
}

TERMINAL_FALLBACK_MESSAGE = (
    "Sorry, we can't respond to your request right now. Our service is currently at capacity. Please try again in a few minutes."
)


def format_web_references(web_items: List[Dict[str, Any]]) -> str:
    if not web_items:
        return ""
    seen_urls = set()
    refs = []
    for item in web_items:
        url = item.get("url")
        if url and url not in seen_urls:
            seen_urls.add(url)
            refs.append(url)
    if not refs:
        return ""
    ref_lines = [f"{i+1}. {url}" for i, url in enumerate(refs)]
    return "\n\nReferences:\n" + "\n".join(ref_lines)

# Issue #20: the critic's first non-empty line must be exactly this, so a
# revision decision never depends on inferring "quality" from free text.
_VERDICT_LINE_RE = re.compile(r"^VERDICT:\s*(OK|NEEDS_REVISION)\s*$", re.IGNORECASE)


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
class GatheredContext:
    """Raw structured web/memory context, gathered once per request.

    Encoding (TOON or JSON, issue #16) happens separately via `.encode()`,
    possibly more than once -- the format-fallback retry in `_execute_plan`
    re-encodes the *same* gathered data as JSON rather than re-fetching
    anything, since the failure it's reacting to is (as best anyone can
    tell) about the encoding, not the underlying search/memory results.
    """

    web_items: List[Dict[str, Any]] = field(default_factory=list)
    memory_items: List[Dict[str, Any]] = field(default_factory=list)
    documents: int = 0
    source: str = "none"  # "none" | "web" | "hippocampus" | "hybrid"
    gather_latency_ms: int = 0

    @property
    def is_empty(self) -> bool:
        return not self.web_items and not self.memory_items

    def encode(self, format_name: str) -> str:
        parts = []
        if self.web_items:
            parts.append(f"## Web context\n\n{encode({'web_results': self.web_items}, format_name)}")
        if self.memory_items:
            parts.append(f"## Memory context\n\n{encode({'memory_chunks': self.memory_items}, format_name)}")
        return "\n\n".join(parts)


@dataclass(frozen=True)
class _ContextRetryState:
    """Carries what `_execute_plan`'s primary step needs to attempt the
    TOON->JSON fallback retry, without threading five separate parameters
    through the reroute-recursive call (which deliberately does NOT get
    one of these: reroute reuses the already-assembled prompt as-is, see
    `_try_reroute`)."""

    gathered: GatheredContext
    format_name: str
    base_prompt: str
    primary_model: Optional[ModelCatalogEntry]
    forced: bool

    @property
    def eligible_for_json_fallback(self) -> bool:
        # Only an *unpinned* model auto-tries JSON after a TOON failure: a
        # pin is a manual decision, it's never silently overridden. A
        # forced format (layer 3) is the caller's explicit choice for this
        # one call -- "se ele forçou... utiliza normal, acabou", no auto
        # fallback either. And there's nothing to fall back *from* unless
        # TOON was actually what got used.
        return (
            not self.forced
            and self.format_name == TOON
            and self.primary_model is not None
            and not has_active_context_format_pin(self.primary_model)
        )


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
    encoded (TOON by default, per the destination model's preference --
    issues #14/#15/#16) into the prompt every step sees: it's the "optional
    RAG" stage of the pipeline described in docs/specs.md, right before the
    model call.
    """

    def __init__(
        self,
        drivers: Dict[str, ExecutionDriver],
        quota_tracker: QuotaTracker,
        telemetry: Optional[TelemetryLogger] = None,
        router: Optional[Router] = None,
        web_retrieval: Optional[WebRetrievalService] = None,
        hippocampus: Optional[HippocampusClient] = None,
        registry: Optional[ModelRegistryService] = None,
        max_retries: int = 1,
        max_reroutes: int = 1,
        max_critic_revisions: int = 1,
        sanitize_enabled: bool = True,
        attachment_ingestor: Optional[AttachmentIngestor] = None,
    ) -> None:
        self._drivers = drivers
        self._quota_tracker = quota_tracker
        self._telemetry = telemetry or TelemetryLogger()
        self._router = router
        self._web_retrieval = web_retrieval
        self._hippocampus = hippocampus
        self._registry = registry
        self._max_retries = max_retries
        self._max_reroutes = max_reroutes
        self._max_critic_revisions = max_critic_revisions
        self._sanitize_enabled = sanitize_enabled
        self._attachment_ingestor = attachment_ingestor or build_default_attachment_ingestor()

    @property
    def drivers(self) -> Dict[str, ExecutionDriver]:
        return self._drivers

    async def execute(self, plan: ExecutionPlan) -> ExecutionResult:
        if not plan.selections:
            raise UnresolvedStrategyError(plan.strategy_id)

        primary_model = self._lookup_model(plan.selections[0].model_id)

        ingested = IngestedAttachments()
        if plan.attachments:
            vision_capable = bool(primary_model and primary_model.capabilities.get("vision", False))
            loop = asyncio.get_running_loop()
            ingested = await loop.run_in_executor(
                None, lambda: self._attachment_ingestor.ingest(plan.attachments, model_is_vision_capable=vision_capable)
            )
            if not ingested.ok:
                return ExecutionResult(
                    request_id=str(uuid.uuid4()), tier_requested=plan.tier, tier_executed=plan.tier,
                    strategy_id=plan.strategy_id, task_type=plan.task_type, success=False,
                    response_text="; ".join(ingested.errors), error_type="attachment_error",
                )
            if ingested.text_context and self._hippocampus is not None and plan.use_memory:
                try:
                    await self._hippocampus.store_event(
                        topic=plan.memory_topic or plan.task_type,
                        key=f"attachment:{plan.attachments[0].filename}",
                        value=ingested.text_context,
                    )
                except Exception as exc:
                    logger.warning("failed to store attachment transcript in memory: %s", exc)

        gathered = await self._gather_context(plan)
        format_name = resolve_context_format(primary_model, plan.context_format)

        context_retry: Optional[_ContextRetryState] = None
        prompt = plan.prompt
        if ingested.text_context:
            prompt = f"## Attachment context\n\n{ingested.text_context}\n\n{prompt}"
        working_plan = replace(plan, prompt=prompt) if prompt != plan.prompt else plan
        if not gathered.is_empty:
            context_text = gathered.encode(format_name)
            if context_text:
                working_plan = replace(working_plan, prompt=f"{context_text}\n\n{working_plan.prompt}")
            context_retry = _ContextRetryState(
                gathered=gathered, format_name=format_name, base_prompt=working_plan.prompt,
                primary_model=primary_model, forced=plan.context_format is not None,
            )

        return await self._execute_plan(
            working_plan, reroutes_left=self._max_reroutes, context_retry=context_retry,
            images=ingested.image_data_uris, request_id=str(uuid.uuid4()),
            deadline=monotonic() + working_plan.max_latency_seconds,
        )

    def _lookup_model(self, model_id: str) -> Optional[ModelCatalogEntry]:
        if self._registry is None:
            return None
        return self._registry.get_model(model_id)

    # -- retrieval assembly ---------------------------------------------------

    async def _gather_context(self, plan: ExecutionPlan) -> GatheredContext:
        if not plan.needs_web and not plan.use_memory:
            return GatheredContext()

        started = monotonic()
        web_items: List[Dict[str, Any]] = []
        memory_items: List[Dict[str, Any]] = []
        documents = 0
        used_web = False
        used_memory = False

        if plan.needs_web and self._web_retrieval is not None:
            try:
                loop = asyncio.get_running_loop()
                web_result = await loop.run_in_executor(None, self._web_retrieval.gather_context, plan.prompt)
                if web_result.items:
                    web_items = web_result.items
                    documents += len(web_result.sources)
                    used_web = True
            except Exception as exc:
                logger.warning("web retrieval failed for this request: %s", exc)

        if plan.use_memory and self._hippocampus is not None:
            try:
                topic = plan.memory_topic or plan.task_type
                chunks = await self._hippocampus.search_memory(topic, plan.prompt)
                if chunks:
                    memory_items = [{"topic": c.topic, "content": c.content, "score": c.score} for c in chunks]
                    documents += len(chunks)
                    used_memory = True
            except Exception as exc:
                logger.warning("memory retrieval failed for this request: %s", exc)

        source = "hybrid" if used_web and used_memory else "web" if used_web else "hippocampus" if used_memory else "none"
        latency_ms = int((monotonic() - started) * 1000)
        return GatheredContext(web_items, memory_items, documents, source, latency_ms)

    # -- pipeline execution -----------------------------------------------------

    async def _execute_plan(
        self,
        plan: ExecutionPlan,
        reroutes_left: int,
        context_retry: Optional[_ContextRetryState] = None,
        images: Optional[List[str]] = None,
        request_id: Optional[str] = None,
        deadline: Optional[float] = None,
    ) -> ExecutionResult:
        request_id = request_id or str(uuid.uuid4())
        deadline = deadline if deadline is not None else monotonic() + plan.max_latency_seconds
        if any(selection.role == "fallback" for selection in plan.selections):
            return await self._execute_ordered_cascade(
                plan, request_id=request_id, deadline=deadline, context_retry=context_retry, images=images
            )
        steps: List[StepResult] = []
        context = plan.prompt
        # The primary's original text, kept separate from `context` (which
        # gets overwritten by the refiner's output): the critic step needs
        # both, to judge the refined answer against its actual baseline
        # instead of reviewing a single text with nothing to compare it to.
        draft_text: Optional[str] = None
        refiner_selection = next((s for s in plan.selections if s.role == "refiner"), None)

        for index, selection in enumerate(plan.selections):
            is_primary = index == 0
            step = await self._run_step(
                plan, selection, context, deadline, draft=draft_text, images=images if is_primary else None
            )

            if (
                is_primary
                and not step.success
                and context_retry is not None
                and context_retry.eligible_for_json_fallback
            ):
                self._log_step(plan, request_id, selection, step, context_retry.gathered, context_retry.format_name)
                step = await self._retry_with_json_context(plan, selection, context, deadline, context_retry, request_id)
            else:
                gathered = context_retry.gathered if context_retry is not None else None
                step_format = context_retry.format_name if (is_primary and context_retry is not None) else None
                self._log_step(plan, request_id, selection, step, gathered, step_format)

            steps.append(step)

            # `context` tracks the latest *candidate answer* text -- not the
            # critic's own review, which isn't an answer at all (issue #20:
            # the reviser needs to see the answer being reviewed, not the
            # critic's verdict/feedback text sitting where it should be).
            if step.success and selection.role != "critic":
                # Pre-existing bug fixed by #19: `context` used to only get
                # updated after a *primary* success, so the critic step
                # always ended up reviewing the primary's original draft --
                # never the refiner's actual output. `draft_text` (#19, the
                # critic's "original draft" side) is tracked separately and
                # only ever set from primary.
                if selection.role == "primary":
                    draft_text = step.response_text or draft_text
                context = step.response_text or context

            if selection.role == "critic":
                if step.success:
                    context = await self._run_revision_loop(
                        plan, selection, refiner_selection, draft_text, step, context, deadline, request_id, steps
                    )
                continue

            if not step.success:
                # Primary/step failed: Mark model & provider in Cooldown
                self._quota_tracker.enter_cooldown(selection.model_id, provider=selection.provider, reason=step.error_message or step.error_type)
                
                # Attempt primary candidate rerouting first
                if self._router is not None and reroutes_left > 0:
                    rerouted = self._try_reroute(plan)
                    if rerouted is not None:
                        return await self._execute_plan(
                            rerouted, reroutes_left - 1, images=images, request_id=request_id, deadline=deadline
                        )
                
                # CLI fallback is allowed only for T3-T5.
                if plan.tier <= 2:
                    break

                # Primary Fallback Loop across CLI Sidecars (agy -> codex)
                cli_sidecars = [
                    ("agy", "agy/gemini-2.5-pro"),
                    ("codex", "codex/gpt-5.4"),
                ]
                for cli_provider, cli_model_id in cli_sidecars:
                    if cli_provider == selection.provider:
                        continue  # skip provider that just failed
                    sidecar_selection = ModelSelection(model_id=cli_model_id, provider=cli_provider, role="primary")
                    sidecar_step = await self._run_step(plan, sidecar_selection, context, deadline, images=images)
                    self._log_step(plan, request_id, sidecar_selection, sidecar_step, None, None)
                    steps.append(sidecar_step)
                    if sidecar_step.success:
                        return self._build_result(plan, request_id, steps, gathered=gathered if 'gathered' in locals() else None)
                    else:
                        self._quota_tracker.enter_cooldown(cli_model_id, provider=cli_provider, reason=sidecar_step.error_message or sidecar_step.error_type)
                break

        return self._build_result(plan, request_id, steps, gathered=gathered)

    async def _execute_ordered_cascade(
        self,
        plan: ExecutionPlan,
        *,
        request_id: str,
        deadline: float,
        context_retry: Optional[_ContextRetryState],
        images: Optional[List[str]],
    ) -> ExecutionResult:
        """Run an explicitly curated list in order, stopping on success.

        This path intentionally does not re-route or borrow an adjacent
        tier.  The local T1/T2 fallback and the T3-T5 CLI fallbacks are
        already prescribed by the product policy.
        """
        steps: List[StepResult] = []
        gathered = context_retry.gathered if context_retry is not None else None
        for index, selection in enumerate(plan.selections):
            step = await self._run_step(plan, selection, plan.prompt, deadline, images=images if index == 0 else None)
            self._log_step(
                plan, request_id, selection, step, gathered if index == 0 else None,
                context_retry.format_name if index == 0 and context_retry is not None else None,
            )
            steps.append(step)
            if step.success:
                return self._build_result(plan, request_id, steps, gathered=gathered)
            self._quota_tracker.enter_cooldown(
                selection.model_id, provider=selection.provider,
                reason=step.error_message or step.error_type,
            )

        if plan.tier >= 3:
            for provider, model_id in (("agy", "agy/gemini-2.5-pro"), ("codex", "codex/gpt-5.4")):
                selection = ModelSelection(model_id=model_id, provider=provider, role="fallback")
                step = await self._run_step(plan, selection, plan.prompt, deadline, images=images)
                self._log_step(plan, request_id, selection, step, None, None)
                steps.append(step)
                if step.success:
                    return self._build_result(plan, request_id, steps, gathered=gathered)
                self._quota_tracker.enter_cooldown(model_id, provider=provider, reason=step.error_message or step.error_type)

        return self._build_result(plan, request_id, steps, gathered=gathered)

    async def _run_revision_loop(
        self,
        plan: ExecutionPlan,
        critic_selection: ModelSelection,
        refiner_selection: Optional[ModelSelection],
        draft_text: Optional[str],
        critic_step: StepResult,
        current_answer: str,
        deadline: float,
        request_id: str,
        steps: List[StepResult],
    ) -> str:
        """Issue #20: closes the "critic found a problem -> do something
        about it" gap. Bounded by `self._max_critic_revisions`; stops on an
        OK verdict, a failed revise/re-review call, or the budget running
        out -- never indefinitely. Returns the latest accepted answer text
        (the original `current_answer` if no revision ever ran or the first
        one already checked out)."""
        verdict, feedback = _parse_critic_verdict(critic_step.response_text)
        revisions_left = self._max_critic_revisions

        while verdict == "needs_revision" and revisions_left > 0 and refiner_selection is not None:
            revisions_left -= 1
            reviser_selection = replace(refiner_selection, role="reviser")
            revise_step = await self._run_step(
                plan, reviser_selection, current_answer, deadline, draft=draft_text, feedback=feedback
            )
            self._log_step(plan, request_id, reviser_selection, revise_step, None, None)
            steps.append(revise_step)
            if not revise_step.success:
                if revise_step.error_type == "rate_limit":
                    self._quota_tracker.enter_cooldown(reviser_selection.model_id, reason=revise_step.error_message)
                break
            current_answer = revise_step.response_text or current_answer

            critic_step = await self._run_step(plan, critic_selection, current_answer, deadline, draft=draft_text)
            self._log_step(plan, request_id, critic_selection, critic_step, None, None)
            steps.append(critic_step)
            if not critic_step.success:
                if critic_step.error_type == "rate_limit":
                    self._quota_tracker.enter_cooldown(critic_selection.model_id, reason=critic_step.error_message)
                break
            verdict, feedback = _parse_critic_verdict(critic_step.response_text)

        return current_answer

    async def _retry_with_json_context(
        self,
        plan: ExecutionPlan,
        selection: ModelSelection,
        context: str,
        deadline: float,
        context_retry: _ContextRetryState,
        request_id: str,
    ) -> StepResult:
        """The "erro de comunicação" fallback: a TOON-context primary step
        failed on a model with no pin, so try once more with the exact same
        gathered data re-encoded as JSON. Success here means the model
        genuinely does better with JSON, so it's persisted (issue #15's
        `context_format_computed`) -- the next call to this model skips
        straight to JSON instead of repeating a TOON attempt that already
        failed once ("evitar erro de múltiplos try")."""
        json_context = context_retry.gathered.encode(JSON)
        retry_prompt = f"{json_context}\n\n{context_retry.base_prompt}" if json_context else context_retry.base_prompt
        retry_plan = replace(plan, prompt=retry_prompt)

        retry_step = await self._run_step(retry_plan, selection, context, deadline)
        self._log_step(plan, request_id, selection, retry_step, context_retry.gathered, JSON)

        if retry_step.success and context_retry.primary_model is not None and self._registry is not None:
            self._registry.set_context_format_computed(context_retry.primary_model.id, JSON)

        return retry_step

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
        self,
        plan: ExecutionPlan,
        selection: ModelSelection,
        context: str,
        deadline: float,
        draft: Optional[str] = None,
        feedback: Optional[str] = None,
        images: Optional[List[str]] = None,
    ) -> StepResult:
        driver = self._drivers.get(selection.provider)
        if driver is None:
            return StepResult(
                role=selection.role, provider=selection.provider, model_id=selection.model_id,
                success=False, response_text="", input_tokens=0, output_tokens=0, latency_ms=0, cost_usd=0.0,
                error_type="no_driver", error_message=f"no execution driver registered for {selection.provider!r}",
                attempts=0,
            )

        prompt = _build_role_prompt(selection.role, plan.prompt, context, draft=draft, feedback=feedback)
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
                    error_type="timeout", error_message="request deadline exhausted before this step ran",
                )
                break
            try:
                result = await asyncio.wait_for(
                    loop.run_in_executor(None, functools.partial(driver.run, bare_model, prompt, images=images)),
                    timeout=remaining
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

            if result and result.success and not result.response_text.strip():
                result = DriverResult(
                    success=False, response_text="", input_tokens=result.input_tokens, output_tokens=result.output_tokens,
                    latency_ms=result.latency_ms, cost_usd=result.cost_usd, error_type="empty_response",
                    error_message=f"{driver.provider}: step returned empty response text",
                )
            self._quota_tracker.record_execution(selection.provider, selection.model_id, result)
            if result.success or result.error_type not in _RETRYABLE_ERRORS:
                break

        assert result is not None
        response_text = result.response_text
        if self._sanitize_enabled and result.success and response_text:
            cleaned, stats = sanitize_text(response_text)
            if stats.removed_count or stats.replaced_count:
                logger.info(
                    "sanitized provider text: provider=%s model=%s removed=%d replaced=%d",
                    selection.provider, bare_model, stats.removed_count, stats.replaced_count,
                )
            response_text = cleaned

        return StepResult(
            role=selection.role, provider=selection.provider, model_id=selection.model_id,
            success=result.success, response_text=response_text,
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
        gathered: Optional[GatheredContext],
        context_format: Optional[str] = None,
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
            retrieval_source=gathered.source if gathered else "none",
            retrieval_documents=gathered.documents if gathered else 0,
            retrieval_latency_ms=gathered.gather_latency_ms if gathered else 0,
            estimated_cost_usd=step.cost_usd,
            # context_format is only set for the step(s) that actually used
            # an encoded context block (issues #15/#16); context_type mirrors
            # retrieval_source for that same step -- kept as its own column
            # since it answers a different question (what format was used
            # for this *kind* of context) than retrieval_source does (did
            # retrieval happen, and how).
            context_format=context_format,
            context_type=(gathered.source if gathered and gathered.source != "none" else None),
            phase="model",
            attempt_index=step.attempts,
            deadline_remaining_ms=None,
            audit_details=f"provider={selection.provider};model={selection.model_id};role={selection.role}",
        )
        self._telemetry.record_async(event)

    def _build_result(
        self,
        plan: ExecutionPlan,
        request_id: str,
        steps: List[StepResult],
        gathered: Optional[GatheredContext] = None,
    ) -> ExecutionResult:
        primary = steps[0] if steps else None
        success = bool(primary and any(s.success for s in steps))
        if not success:
            final_text = TERMINAL_FALLBACK_MESSAGE
            error_type = primary.error_type if (primary and primary.error_type) else "all_candidates_exhausted"
        else:
            final_text = next(
                (s.response_text for s in reversed(steps) if s.success and s.role in ("primary", "fallback", "refiner", "reviser")),
                "",
            )
            if gathered and gathered.web_items and "References:" not in final_text:
                refs_formatted = format_web_references(gathered.web_items)
                if refs_formatted:
                    final_text = final_text.rstrip() + refs_formatted
            error_type = None

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
            error_type=error_type,
        )


def _bare_model_name(model_id: str) -> str:
    """`ModelCatalogEntry.id` is `"provider/name"`; drivers want just `name`."""
    return model_id.split("/", 1)[1] if "/" in model_id else model_id


_VERDICT_INSTRUCTIONS = (
    "Respond in this exact format: the first line must be exactly 'VERDICT: OK' "
    "or 'VERDICT: NEEDS_REVISION' (nothing else on that line). If NEEDS_REVISION, "
    "follow it with a blank line and then concrete feedback: what's wrong and what to fix."
)


def _build_role_prompt(
    role: str, original_prompt: str, context: str, draft: Optional[str] = None, feedback: Optional[str] = None
) -> str:
    if role == "primary":
        return original_prompt
    if role == "refiner":
        return (
            f"{original_prompt}\n\n---\nA first draft answer was produced below. "
            f"Refine it: fix mistakes, tighten the reasoning, improve clarity. "
            f"Return only the improved answer.\n\nDraft:\n{context}"
        )
    if role == "critic":
        # With a distinct draft available (issue #19), the critic gets both
        # texts and is asked to judge the refinement against its actual
        # baseline -- not just review one text with nothing to compare it
        # to. Falls back to the single-answer framing when there's no
        # refiner in the pipeline (or its output happens to equal the draft).
        # The structured verdict line (issue #20) is what the revision loop
        # parses -- never inferred from free-text sentiment.
        if draft is not None and draft.strip() and draft != context:
            return (
                f"{original_prompt}\n\n---\nAn original draft and a refined answer are below. "
                f"Review the refined answer for correctness and completeness, and check whether "
                f"refining it actually improved on the original draft. {_VERDICT_INSTRUCTIONS}\n\n"
                f"Original draft:\n{draft}\n\nRefined answer:\n{context}"
            )
        return (
            f"{original_prompt}\n\n---\nReview the answer below for correctness and completeness. "
            f"{_VERDICT_INSTRUCTIONS}\n\nAnswer:\n{context}"
        )
    if role == "reviser":
        # Issue #20: a follow-up refiner pass addressing the critic's
        # specific feedback, run by the same model/provider as the original
        # refiner step (just relabeled for this call -- see _run_revision_loop).
        return (
            f"{original_prompt}\n\n---\nA reviewer found issues with the answer below and requested "
            f"a revision. Address the feedback precisely. Return only the revised answer.\n\n"
            f"Original draft:\n{draft or context}\n\nPrevious answer:\n{context}\n\n"
            f"Reviewer feedback:\n{feedback or '(no specific feedback provided)'}"
        )
    return original_prompt


def _parse_critic_verdict(response_text: str) -> tuple[str, str]:
    """Returns (verdict, feedback): verdict is "ok" or "needs_revision".

    Requires the verdict on the first non-empty line, exactly. Anything
    else -- an empty response, no verdict line, extra text before it --
    defaults to "ok" (the loop stops) rather than "needs_revision": a false
    OK from an uncooperative critic is a lesser problem than an unbounded
    revision loop burning calls/tokens/cost on a model that won't follow
    the format. This is a deliberate tradeoff, not an oversight.
    """
    lines = response_text.splitlines()
    for i, line in enumerate(lines):
        stripped = line.strip()
        if not stripped:
            continue
        match = _VERDICT_LINE_RE.match(stripped)
        if match is None:
            break
        feedback = "\n".join(lines[i + 1 :]).strip()
        verdict = "needs_revision" if match.group(1).upper() == "NEEDS_REVISION" else "ok"
        return verdict, feedback
    return "ok", ""


def build_default_drivers(settings: Optional[Settings] = None) -> Dict[str, ExecutionDriver]:
    settings = settings or get_settings()
    drivers = {
        "ollama": OllamaDriver(base_url=settings.ollama_base_url, timeout=settings.driver_timeout_seconds),
        "claude": ClaudeDockerDriver(command=settings.claude_command, timeout=settings.driver_timeout_seconds),
        "agy": AgyDockerDriver(command=settings.agy_command, timeout=settings.driver_timeout_seconds),
        "codex": CodexDriver(command=settings.codex_command, timeout=settings.driver_timeout_seconds),
        "groq": GroqDriver(api_key=settings.groq_api_key, timeout=settings.driver_timeout_seconds),
        "google_ai_studio": GoogleAIStudioDriver(
            api_key=settings.google_ai_studio_api_key, timeout=settings.driver_timeout_seconds
        ),
        "openrouter": OpenRouterDriver(api_key=settings.openrouter_api_key, timeout=settings.driver_timeout_seconds),
        "cloudflare": CloudflareDriver(
            api_key=settings.cloudflare_api_key,
            account_id=settings.cloudflare_account_id,
            timeout=settings.driver_timeout_seconds,
        ),
        "cohere": CohereDriver(api_key=settings.cohere_api_key, timeout=settings.driver_timeout_seconds),
        "mistral": MistralDriver(api_key=settings.mistral_api_key, timeout=settings.driver_timeout_seconds),
        "nvidia": NvidiaDriver(api_key=settings.nvidia_api_key, timeout=settings.driver_timeout_seconds),
        "zai": ZaiDriver(api_key=settings.zai_api_key, timeout=settings.driver_timeout_seconds),
        "requesty": RequestyDriver(api_key=settings.requesty_api_key, timeout=settings.driver_timeout_seconds),
        "huggingface": HuggingFaceDriver(
            api_key=settings.huggingface_api_key, timeout=settings.driver_timeout_seconds
        ),
        "ollama_cloud": OllamaCloudDriver(
            api_key=settings.ollama_cloud_api_key, timeout=settings.driver_timeout_seconds
        ),
        "aion_labs": AionLabsDriver(api_key=settings.aion_labs_api_key, timeout=settings.driver_timeout_seconds),
        "siliconflow": SiliconFlowDriver(
            api_key=settings.siliconflow_api_key, timeout=settings.driver_timeout_seconds
        ),
        "inference_net": InferenceNetDriver(
            api_key=settings.inference_net_api_key, timeout=settings.driver_timeout_seconds
        ),
        "sambanova": SambaNovaDriver(api_key=settings.sambanova_api_key, timeout=settings.driver_timeout_seconds),
    }
    for provider in settings.disabled_providers:
        drivers.pop(provider.strip().lower(), None)
    return drivers


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
        registry=build_default_registry_service(settings=settings),
        max_critic_revisions=settings.executor_max_critic_revisions,
        max_retries=settings.executor_max_retries,
        max_reroutes=settings.executor_max_reroutes,
        sanitize_enabled=settings.sanitize_provider_text,
    )
