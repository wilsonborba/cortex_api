from __future__ import annotations

from dataclasses import dataclass, field
from typing import List, Optional, Union

from lib.core.settings import Settings, get_settings
from lib.engine.attachments import Attachment
from lib.dal.models import ModelCatalogEntry, RoutingPin, TierPolicy
from lib.dal.repositories.pin_repository import RoutingPinRepository
from lib.engine.classifier import classify_complexity, extract_tier_directive
from lib.engine.quota import QuotaTracker
from lib.engine.registry_service import ModelRegistryService, build_default_registry_service
from lib.engine.scoring import ModelScorer, build_default_scorer
from lib.engine.tiers import MAX_TIER, MIN_TIER, TierService
from lib.engine.curated_tier_catalog import model_ids_for_tier

@dataclass(frozen=True)
class ModelSelection:
    model_id: str
    provider: str
    role: str  # "primary" | "refiner" | "researcher" | "critic"
    score: Optional[float] = None


@dataclass(frozen=True)
class ExecutionPlan:
    """What the Executor (issue #9) should run. Router's job ends here: it
    decides *what* to run, not how to run a multi-step pipeline.

    `selections` is empty for `source in ("override", "pin")` when the
    override/pin names a `strategy_id` rather than a concrete model: that
    strategy's own model sequence is the Executor's strategy registry to
    resolve, not the Router's.
    """

    tier: int
    task_type: str
    strategy_id: str
    prompt: str
    original_prompt: str
    tenant_id: str
    conversation_id: Optional[str]
    selections: List[ModelSelection]
    allow_multi_model: bool
    retrieval_mode: str
    needs_web: bool
    use_memory: bool
    memory_topic: Optional[str]
    require_verification: bool
    max_latency_seconds: int
    max_model_calls: int
    source: str  # "override" | "pin" | "dynamic"
    reason: str
    context_format: Optional[str] = None  # layer-3 force (#16); None => Executor resolves it itself
    attachments: List[Attachment] = field(default_factory=list)


@dataclass(frozen=True)
class RoutingRequest:
    prompt: str
    tenant_id: str = "default"
    conversation_id: Optional[str] = None
    tier: Optional[Union[int, str]] = None  # None or "auto" => resolved by classifier/directive
    task_type: str = "general"
    needs_web: bool = False
    use_memory: bool = False
    auto_retrieval: bool = False
    thinking: bool = False
    memory_topic: Optional[str] = None
    force_model: Optional[str] = None
    force_provider: Optional[str] = None
    force_strategy: Optional[str] = None
    force_context_format: Optional[str] = None  # "toon" | "json" (#16): overrides the per-model auto/pin default
    max_provider_candidates: Optional[int] = None  # None => defaults to settings.max_provider_candidates (5); 0/unlimited => check all
    attachments: List[Attachment] = field(default_factory=list)


class NoEligibleModelError(RuntimeError):
    """Raised when a tier's envelope (plus any force_model/force_provider) leaves zero candidates."""


class Router:
    """Ties Tier envelopes, the Model Registry, the Quota Tracker, dynamic
    scoring, and manual pinning into one `build_execution_plan()` call.

    Precedence, strongest first: request-level force_* overrides > manual
    pins (strategy pin, then model pin) > dynamic scoring.
    """

    def __init__(
        self,
        registry: Optional[ModelRegistryService] = None,
        quota_tracker: Optional[QuotaTracker] = None,
        scorer: Optional[ModelScorer] = None,
        tier_service: Optional[TierService] = None,
        pin_repo: Optional[RoutingPinRepository] = None,
        settings: Optional[Settings] = None,
    ) -> None:
        self._settings = settings or get_settings()
        self._quota_tracker = quota_tracker or QuotaTracker(settings=self._settings)
        self._registry = registry or ModelRegistryService(discoveries=[])
        self._scorer = scorer or build_default_scorer(settings=self._settings, quota_tracker=self._quota_tracker)
        self._tier_service = tier_service or TierService()
        self._pin_repo = pin_repo or RoutingPinRepository()

    # -- tier resolution ----------------------------------------------------

    def resolve_tier(self, requested_tier: Optional[Union[int, str]], prompt: str) -> tuple[int, str]:
        """Explicit numeric tier > prompt directive (`/t3`, `[T4]`) > auto-classify.

        The directive is always stripped from the prompt, even when an
        explicit tier makes its value moot, so the model never sees it.
        """
        directive_tier, cleaned_prompt = extract_tier_directive(prompt)
        if requested_tier is not None and requested_tier != "auto":
            return _clamp_tier(int(requested_tier)), cleaned_prompt
        if directive_tier is not None:
            return _clamp_tier(directive_tier), cleaned_prompt
        return _clamp_tier(classify_complexity(cleaned_prompt)), cleaned_prompt

    # -- main entry point -----------------------------------------------------

    def build_execution_plan(self, request: RoutingRequest) -> ExecutionPlan:
        tier, prompt = self.resolve_tier(request.tier, request.prompt)
        envelope = self._tier_service.get_envelope(tier)

        # Operational policy: Ollama keeps one model resident, so the local
        # text and vision models cannot compete as generic Tier-0 candidates.
        # This deliberately outranks caller overrides, pins and scoring.
        if self._has_image_attachment(request):
            return self._plan_required_local_model(
                request, tier, prompt, envelope,
                model_id=self._settings.local_vision_model_id,
                role_name="vision",
                require_vision=True,
            )
        if tier == 0:
            return self._plan_required_local_model(
                request, tier, prompt, envelope,
                model_id=self._settings.local_text_model_id,
                role_name="text",
                require_vision=False,
            )

        if request.force_model or request.force_provider or request.force_strategy:
            return self._plan_from_override(request, tier, prompt, envelope)

        pin = self._pin_repo.get_pin(tier, task_type=request.task_type)
        if pin is not None:
            return self._plan_from_pin(request, tier, prompt, envelope, pin)

        return self._plan_dynamic(request, tier, prompt, envelope)

    @staticmethod
    def _has_image_attachment(request: RoutingRequest) -> bool:
        return any(attachment.mime_type.startswith("image/") for attachment in request.attachments)

    def _plan_required_local_model(
        self,
        request: RoutingRequest,
        tier: int,
        prompt: str,
        envelope: TierPolicy,
        *,
        model_id: str,
        role_name: str,
        require_vision: bool,
    ) -> ExecutionPlan:
        """Build a plan for an explicitly configured local model role."""
        model = self._registry.get_model(model_id)
        valid = (
            model is not None
            and model.is_local
            and model.is_enabled
            and model.access_status == "AVAILABLE"
            and (not require_vision or self._registry.is_vision_capable(model))
        )
        if not valid:
            capability = " and marked vision-capable" if require_vision else ""
            raise NoEligibleModelError(
                f"Configured local {role_name} model {model_id!r} is not available, enabled and local{capability}"
            )

        selection = ModelSelection(model_id=model.id, provider=model.provider, role="primary")
        return self._make_plan(
            tier, request, prompt, envelope, [selection],
            strategy_id=f"{request.task_type}_t{tier}_ollama_{role_name}_policy",
            source="local-role-policy",
            reason=f"configured local {role_name} routing policy",
        )

    # -- layer 3: request-level overrides --------------------------------------

    def _plan_from_override(
        self, request: RoutingRequest, tier: int, prompt: str, envelope: TierPolicy
    ) -> ExecutionPlan:
        if request.force_strategy:
            return self._make_plan(
                tier, request, prompt, envelope, selections=[],
                strategy_id=request.force_strategy, source="override",
                reason="force_strategy override",
            )

        candidates = self._candidates(tier, envelope, provider=request.force_provider, model_id=request.force_model)
        if not candidates:
            raise NoEligibleModelError(
                f"No AVAILABLE model matches force_model={request.force_model!r} "
                f"force_provider={request.force_provider!r} at tier {tier}"
            )
        chosen = candidates[0]
        selections = [ModelSelection(model_id=chosen.id, provider=chosen.provider, role="primary")]
        return self._make_plan(
            tier, request, prompt, envelope, selections,
            strategy_id=f"{request.task_type}_t{tier}_forced", source="override",
            reason="request-level force_model/force_provider",
        )

    # -- manual pinning ------------------------------------------------------

    def _plan_from_pin(
        self, request: RoutingRequest, tier: int, prompt: str, envelope: TierPolicy, pin: RoutingPin
    ) -> ExecutionPlan:
        if pin.strategy_id:
            return self._make_plan(
                tier, request, prompt, envelope, selections=[],
                strategy_id=pin.strategy_id, source="pin",
                reason=f"strategy pin by {pin.pinned_by or 'unknown'}",
            )
        if pin.model_id:
            model = self._registry.get_model(pin.model_id)
            provider = model.provider if model else pin.model_id.split("/", 1)[0]
            selections = [ModelSelection(model_id=pin.model_id, provider=provider, role="primary")]
            return self._make_plan(
                tier, request, prompt, envelope, selections,
                strategy_id=f"{request.task_type}_t{tier}_pinned", source="pin",
                reason=f"model pin by {pin.pinned_by or 'unknown'}",
            )
        # A pin record with neither field set carries no instruction: fall through.
        return self._plan_dynamic(request, tier, prompt, envelope)

    # -- dynamic scoring -----------------------------------------------------

    def _plan_dynamic(
        self, request: RoutingRequest, tier: int, prompt: str, envelope: TierPolicy
    ) -> ExecutionPlan:
        candidates = self._candidates(tier, envelope, max_candidates=request.max_provider_candidates)
        if not candidates:
            raise NoEligibleModelError(f"No AVAILABLE model eligible for tier {tier}")

        # A persistent allowed-models policy is a reviewed execution
        # cascade.  Do not score or re-sort it: position is intentional.
        # The executor tries `primary`, then each `fallback`, stopping at the
        # first success.
        if self._is_curated_policy(tier, envelope):
            selections = [
                ModelSelection(
                    model_id=model.id,
                    provider=model.provider,
                    role="primary" if index == 0 else "fallback",
                )
                for index, model in enumerate(candidates)
            ]
            return self._make_plan(
                tier, request, prompt, envelope, selections,
                strategy_id=f"{request.task_type}_t{tier}_{selections[0].provider}_curated",
                source="dynamic", reason="ordered curated tier catalog",
            )

        scored = [
            self._scorer.score(model, request.task_type, envelope.max_latency_seconds, requested_tier=tier)
            for model in candidates
        ]
        threshold = self._settings.quota_critical_threshold
        preferred = [s for s in scored if s.quota_factor >= threshold]
        pool = preferred if preferred else scored
        pool = sorted(pool, key=lambda s: s.score, reverse=True)
        degraded = not preferred

        roles = ["primary"]
        if request.thinking and envelope.allow_multi_model and envelope.max_model_calls > 1 and len(pool) > 1:
            roles.append("refiner")
        if (
            request.thinking
            and envelope.require_verification
            and envelope.max_model_calls > len(roles)
            and len(pool) > len(roles)
        ):
            roles.append("critic")

        selections = [
            ModelSelection(model_id=s.model_id, provider=s.provider, role=role, score=round(s.score, 4))
            for role, s in zip(roles, pool)
        ]
        reason = "dynamic scoring"
        if degraded:
            reason += f" (all candidates below quota critical threshold {threshold})"

        strategy_id = f"{request.task_type}_t{tier}_{selections[0].provider}_dynamic"
        return self._make_plan(tier, request, prompt, envelope, selections, strategy_id, source="dynamic", reason=reason)

    @staticmethod
    def _is_curated_policy(tier: int, envelope: TierPolicy) -> bool:
        """Custom API restrictions still use dynamic scoring; only the
        shipped reviewed catalog is a deterministic execution cascade."""
        return envelope.allowed_models == model_ids_for_tier(tier)

    # -- shared helpers --------------------------------------------------------

    def _candidates(
        self,
        tier: int,
        envelope: TierPolicy,
        provider: Optional[str] = None,
        model_id: Optional[str] = None,
        max_candidates: Optional[int] = None,
    ) -> List[ModelCatalogEntry]:
        disabled = {provider.strip().lower() for provider in self._settings.disabled_providers}
        models = self._registry.list_available_for_router(tier=tier)
        models = [m for m in models if not m.tier_eligibility or tier in m.tier_eligibility]
        models = [m for m in models if m.provider.lower() not in disabled]

        if envelope.allowed_models is not None:
            allowed = set(envelope.allowed_models)
            models = [m for m in models if m.id in allowed]
            if self._is_curated_policy(tier, envelope):
                # The reviewed policy list is an ordered cascade, not an
                # unordered filter.  Ad-hoc API restrictions remain dynamic.
                order = {model_id: index for index, model_id in enumerate(envelope.allowed_models)}
                models.sort(key=lambda model: order[model.id])

            # T1/T2 reserve the local model for after every curated remote
            # candidate.  It is deliberately not part of `allowed_models`:
            # `/tiers` exposes the five reviewed remote choices, while this
            # fallback remains explicit and deterministic at runtime.
            if tier in (1, 2) and self._is_curated_policy(tier, envelope):
                local_fallback_id = model_ids_for_tier(0)[0]
                local_fallback = next(
                    (
                        model for model in self._registry.list_models()
                        if model.id == local_fallback_id
                        and model.provider == "ollama"
                        and model.is_local
                        and model.is_enabled
                        and model.access_status == "AVAILABLE"
                    ),
                    None,
                )
                if local_fallback is not None and not any(model.id == local_fallback.id for model in models):
                    models.append(local_fallback)
        if not envelope.allow_external and any(m.is_local for m in models):
            models = [m for m in models if m.is_local]
        if provider:
            models = [m for m in models if m.provider == provider]
        if model_id:
            models = [m for m in models if m.id == model_id]

        # Apply configurable search limit (default 5; 0 or None means unlimited).
        # The curated list is already ordered and must never be re-ranked.
        max_limit = max_candidates if max_candidates is not None else getattr(self._settings, "max_provider_candidates", 5)
        if (
            not self._is_curated_policy(tier, envelope)
            and max_limit is not None
            and max_limit > 0
            and len(models) > max_limit
        ):
            models = models[:max_limit]

        return models

    def _make_plan(
        self,
        tier: int,
        request: RoutingRequest,
        prompt: str,
        envelope: TierPolicy,
        selections: List[ModelSelection],
        strategy_id: str,
        source: str,
        reason: str,
    ) -> ExecutionPlan:
        auto_retrieval = request.auto_retrieval
        allow_multi_model = request.thinking and envelope.allow_multi_model
        require_verification = request.thinking and envelope.require_verification
        return ExecutionPlan(
            tier=tier,
            task_type=request.task_type,
            strategy_id=strategy_id,
            prompt=prompt,
            original_prompt=request.prompt,
            tenant_id=request.tenant_id,
            conversation_id=request.conversation_id,
            selections=selections,
            allow_multi_model=allow_multi_model,
            retrieval_mode=envelope.retrieval_mode,
            needs_web=request.needs_web or auto_retrieval,
            use_memory=request.use_memory or auto_retrieval,
            memory_topic=request.memory_topic,
            require_verification=require_verification,
            max_latency_seconds=(self._settings.thinking_timeout_seconds if request.thinking else envelope.max_latency_seconds),
            max_model_calls=envelope.max_model_calls,
            source=source,
            reason=reason,
            context_format=request.force_context_format,
            attachments=request.attachments,
        )


def _clamp_tier(tier: int) -> int:
    return max(MIN_TIER, min(MAX_TIER, tier))


def build_default_router(settings: Optional[Settings] = None) -> Router:
    settings = settings or get_settings()
    quota_tracker = QuotaTracker(settings=settings)
    return Router(
        registry=build_default_registry_service(settings=settings),
        quota_tracker=quota_tracker,
        scorer=build_default_scorer(settings=settings, quota_tracker=quota_tracker),
        tier_service=TierService(),
        settings=settings,
    )
