from __future__ import annotations

from lib.core.settings import Settings
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.engine.drivers.base import DriverResult
from lib.engine.prompt_normalizer import PromptNormalizer
from lib.engine.registry_service import ModelRegistryService


class _CapturingOllamaDriver:
    provider = "ollama"

    def __init__(self) -> None:
        self.calls: list[tuple[str, str]] = []

    def run(self, model: str, prompt: str, images=None) -> DriverResult:
        self.calls.append((model, prompt))
        return DriverResult(success=True, response_text="rewritten prompt", input_tokens=1, output_tokens=1, latency_ms=1)


def _seed(repo: ModelRepository, *, model_id: str, vision: bool) -> None:
    repo.upsert(
        ModelCatalogEntry(
            id=model_id,
            provider="ollama",
            display_name=model_id,
            access_status=AccessStatus.AVAILABLE.value,
            context_window=8192,
            is_local=True,
            tier_eligibility=[0],
            capabilities={"vision": vision},
            cost_per_million_tokens=0.0,
            is_enabled=True,
        )
    )


def test_normalizer_uses_configured_text_model_not_vision_model(model_repo: ModelRepository):
    settings = Settings(
        local_text_model_id="ollama/normalizer-heretic-text",
        local_vision_model_id="ollama/normalizer-qwen-vision",
    )
    _seed(model_repo, model_id=settings.local_vision_model_id, vision=True)
    _seed(model_repo, model_id=settings.local_text_model_id, vision=False)
    driver = _CapturingOllamaDriver()
    normalizer = PromptNormalizer(
        registry=ModelRegistryService(discoveries=[], repository=model_repo),
        drivers={"ollama": driver},
        settings=settings,
    )

    result = normalizer.normalize("make this clearer")

    assert result.success is True
    assert result.used_model_id == settings.local_text_model_id
    assert driver.calls[0][0] == "normalizer-heretic-text"


def test_normalizer_does_not_fall_back_to_vision_model_when_text_role_is_missing(model_repo: ModelRepository):
    settings = Settings(
        local_text_model_id="ollama/normalizer-missing-text-role",
        local_vision_model_id="ollama/normalizer-only-vision",
    )
    _seed(model_repo, model_id=settings.local_vision_model_id, vision=True)
    driver = _CapturingOllamaDriver()
    normalizer = PromptNormalizer(
        registry=ModelRegistryService(discoveries=[], repository=model_repo),
        drivers={"ollama": driver},
        settings=settings,
    )

    result = normalizer.normalize("make this clearer")

    assert result.success is False
    assert result.error_type == "normalizer_unavailable"
    assert driver.calls == []
