"""Auditable generation-model catalog used by the runtime tier cascade.

Provider discovery is intentionally broad: it finds chat, embedding, image,
speech and safety models.  Discovery output is *not* a routing policy.  This
module is the small, reviewed policy that turns that output into an ordered
generation cascade.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Final


@dataclass(frozen=True)
class CuratedCandidate:
    model_id: str
    rationale: str


# Candidates are intentionally assigned to one tier only.  The order is the
# execution order, not a score to be re-sorted at request time.
CURATED_TIER_CATALOG: Final[dict[int, tuple[CuratedCandidate, ...]]] = {
    0: (
        CuratedCandidate("ollama/qwen2.5vl:7b", "Configured local general-purpose fallback."),
    ),
    1: (
        CuratedCandidate("requesty/deepinfra/Qwen/Qwen3.5-2B", "2B lightweight generation model."),
        CuratedCandidate("huggingface/Qwen/Qwen3-4B-Instruct-2507", "4B instruction model."),
        CuratedCandidate("huggingface/Qwen/Qwen2.5-Coder-3B-Instruct", "3B code-oriented lightweight model."),
        CuratedCandidate("siliconflow/Qwen/Qwen2.5-7B-Instruct", "7B fast instruction model."),
        CuratedCandidate("siliconflow/Qwen/Qwen3-8B", "8B upper bound for fast/light requests."),
    ),
    2: (
        CuratedCandidate("mistral/ministral-3b-2512", "3B compact general model reserved for standard work."),
        CuratedCandidate("mistral/ministral-8b-2512", "8B compact general model reserved for standard work."),
        CuratedCandidate("siliconflow/google/gemma-4-12B-it", "12B instruction model."),
        CuratedCandidate("siliconflow/Qwen/Qwen3-14B", "14B general instruction model."),
        CuratedCandidate("huggingface/microsoft/phi-4", "Mid-size reasoning model."),
    ),
    3: (
        CuratedCandidate("groq/openai/gpt-oss-20b", "20B general model for advanced requests."),
        CuratedCandidate("huggingface/Qwen/Qwen2.5-Coder-32B-Instruct", "32B coding model."),
        CuratedCandidate("siliconflow/Qwen/Qwen3-32B", "32B general model."),
        CuratedCandidate("ollama_cloud/gemma4:31b", "31B cloud model."),
        CuratedCandidate("mistral/mistral-medium-2505", "Mistral medium model for advanced requests."),
    ),
    4: (
        CuratedCandidate("sambanova/Meta-Llama-3.3-70B-Instruct", "70B high-capability instruction model."),
        CuratedCandidate("siliconflow/Qwen/Qwen2.5-72B-Instruct", "72B high-capability model."),
        CuratedCandidate("ollama_cloud/gpt-oss:120b", "120B cloud model."),
        CuratedCandidate("ollama_cloud/qwen3.5:397b", "397B cloud high-capability model."),
        CuratedCandidate("mistral/mistral-large-2512", "Mistral large model."),
    ),
    5: (
        CuratedCandidate("requesty/openai-responses/gpt-5.6-sol", "Flagship reasoning candidate."),
        CuratedCandidate("nvidia/nvidia/nemotron-3-ultra-550b-a55b", "550B flagship candidate."),
        CuratedCandidate("ollama_cloud/mistral-large-3:675b", "675B cloud flagship candidate."),
        CuratedCandidate("siliconflow/deepseek-ai/DeepSeek-R1", "High-reasoning fallback candidate."),
    ),
}


def model_ids_for_tier(tier: int) -> list[str]:
    """Return the reviewed candidates in deterministic execution order."""
    return [candidate.model_id for candidate in CURATED_TIER_CATALOG[tier]]
