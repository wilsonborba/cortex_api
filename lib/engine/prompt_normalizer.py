from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, Optional

from lib.core.logs import get_logger
from lib.engine.drivers.base import ExecutionDriver
from lib.engine.registry_service import ModelRegistryService

logger = get_logger(__name__)

_NORMALIZER_SYSTEM_PROMPT = (
    "You rewrite user requests into a clearer execution prompt without changing meaning. "
    "Preserve intent, constraints, names, numbers, and requested output format. "
    "Do not answer the task. Do not add new requirements. Return only the rewritten prompt."
)


@dataclass(frozen=True)
class PromptNormalizationResult:
    prompt: str
    used_model_id: Optional[str] = None
    provider: Optional[str] = None
    changed: bool = False
    success: bool = True
    error_type: Optional[str] = None


class PromptNormalizer:
    def __init__(
        self,
        registry: ModelRegistryService,
        drivers: Dict[str, ExecutionDriver],
    ) -> None:
        self._registry = registry
        self._drivers = drivers

    def normalize(self, prompt: str) -> PromptNormalizationResult:
        prompt = (prompt or "").strip()
        if not prompt:
            return PromptNormalizationResult(prompt=prompt, success=False, error_type="normalizer_empty_prompt")

        model = self._pick_model()
        if model is None:
            return PromptNormalizationResult(prompt=prompt, success=False, error_type="normalizer_unavailable")

        driver = self._drivers.get(model.provider)
        if driver is None:
            return PromptNormalizationResult(prompt=prompt, success=False, error_type="normalizer_unavailable")

        rewrite_prompt = (
            f"{_NORMALIZER_SYSTEM_PROMPT}\n\n"
            f"Original user request:\n{prompt}\n\n"
            "Rewritten prompt:"
        )
        bare_model = model.id.split("/", 1)[1] if "/" in model.id else model.id
        try:
            result = driver.run(bare_model, rewrite_prompt)
        except Exception as exc:
            logger.warning("prompt normalization failed for %s: %s", model.id, exc)
            return PromptNormalizationResult(prompt=prompt, success=False, error_type="normalizer_failed")
        if not result.success:
            return PromptNormalizationResult(prompt=prompt, success=False, error_type="normalizer_failed")

        rewritten = (result.response_text or "").strip()
        if not rewritten:
            return PromptNormalizationResult(prompt=prompt, success=False, error_type="normalizer_empty_response")
        return PromptNormalizationResult(
            prompt=rewritten,
            used_model_id=model.id,
            provider=model.provider,
            changed=rewritten != prompt,
        )

    def _pick_model(self):
        local_candidates = [
            model
            for model in self._registry.list_available_for_router(tier=0)
            if model.is_local and model.provider == "ollama" and model.provider in self._drivers
        ]
        if local_candidates:
            return local_candidates[0]

        return None
