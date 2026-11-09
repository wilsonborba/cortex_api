from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, Optional

from lib.core.logs import get_logger
from lib.core.settings import Settings, get_settings
from lib.dal.models import AccessStatus
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
        settings: Optional[Settings] = None,
    ) -> None:
        self._registry = registry
        self._drivers = drivers
        self._settings = settings or get_settings()

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
        """Return only the configured local text model.

        Prompt normalization is text generation.  It must not select the
        vision model by registry ordering, and it must fail explicitly rather
        than silently choosing another local model when the text role is down.
        """
        model = self._registry.get_model(self._settings.local_text_model_id)
        if (
            model is None
            or not model.is_local
            or model.provider != "ollama"
            or model.provider not in self._drivers
            or not model.is_enabled
            or model.access_status != AccessStatus.AVAILABLE.value
        ):
            return None
        return model
