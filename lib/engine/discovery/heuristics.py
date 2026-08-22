import json
import re
import sqlite3
from functools import lru_cache
from pathlib import Path
from typing import Any, Optional


_SIZE_RE = re.compile(r"(?P<size>\d+(?:\.\d+)?)\s*(?P<suffix>[bm])", re.IGNORECASE)

_LOW_TIER_MARKERS = ("mini", "small", "nano", "lite", "flash-lite", "8b", "7b", "4b", "3b", "2b", "1.8b")
_MID_TIER_MARKERS = ("flash", "medium", "13b", "14b", "20b", "22b", "27b", "30b", "32b", "34b")
_HIGH_TIER_MARKERS = (
    "opus", "sonnet", "o1", "o3", "r1", "pro", "70b", "72b", "86b", "120b", "405b", "large",
    "deepseek", "gemini-2.5-pro", "gpt-5", "command-r-plus",
)
_VISION_MARKERS = ("vision", "vl", "llava")
_CODING_MARKERS = ("coder", "codex", "code", "devstral")

BENCHMARK_DB_PATH = Path(__file__).resolve().parent.parent.parent / "dal" / "seeds" / "model_tier_benchmark.db"


@lru_cache(maxsize=1)
def load_benchmark_catalog() -> dict[str, Any]:
    if not BENCHMARK_DB_PATH.exists():
        return {}
    try:
        conn = sqlite3.connect(str(BENCHMARK_DB_PATH))
        cursor = conn.cursor()
        cursor.execute("SELECT model_key, display_name, elo, tier_eligibility_json, capabilities_json, is_local FROM model_benchmarks")
        rows = cursor.fetchall()
        conn.close()

        catalog: dict[str, Any] = {}
        for row in rows:
            model_key, display_name, elo, tier_json, cap_json, is_local = row
            catalog[model_key] = {
                "display_name": display_name,
                "elo": elo,
                "tier_eligibility": json.loads(tier_json) if tier_json else [],
                "capabilities": json.loads(cap_json) if cap_json else {},
                "is_local": bool(is_local),
            }
        return catalog
    except Exception:
        return {}


def find_benchmark_entry(model_name: str, is_local: bool = False) -> Optional[dict[str, Any]]:
    catalog = load_benchmark_catalog()
    if not catalog:
        return None
    lowered = model_name.lower().strip()

    # Exact match first
    if lowered in catalog:
        entry = catalog[lowered]
        if not is_local or entry.get("is_local"):
            return entry

    if is_local:
        # Local models use parameter-size heuristics unless explicitly registered with exact key
        return None

    # Substring / key containment match for cloud models
    for key, data in catalog.items():
        if data.get("is_local"):
            continue
        key_lowered = key.lower()
        if key_lowered == lowered or key_lowered in lowered or lowered in key_lowered:
            return data
    return None


def infer_tier_eligibility(model_name: str, parameter_size: Optional[str] = None, is_local: bool = False) -> list[int]:
    lowered = model_name.lower()

    if is_local:
        bm_entry = find_benchmark_entry(model_name, is_local=True)
        if bm_entry and "tier_eligibility" in bm_entry:
            return list(bm_entry["tier_eligibility"])

        size = _parse_billions(parameter_size or lowered)
        if size is None:
            return [0, 1, 2, 3]
        if size <= 9:
            return [0, 1, 2, 3]
        if size <= 20:
            return [1, 2, 3, 4]
        return [2, 3, 4, 5]

    bm_entry = find_benchmark_entry(model_name, is_local=False)
    if bm_entry and "tier_eligibility" in bm_entry:
        return list(bm_entry["tier_eligibility"])

    if any(marker in lowered for marker in _HIGH_TIER_MARKERS):
        return [3, 4, 5]
    if any(marker in lowered for marker in _MID_TIER_MARKERS):
        return [2, 3, 4]
    if any(marker in lowered for marker in _LOW_TIER_MARKERS):
        return [1, 2, 3]
    return [1, 2, 3, 4]


def infer_capabilities(model_name: str, parameter_size: Optional[str] = None) -> dict[str, Any]:
    bm_entry = find_benchmark_entry(model_name, is_local=False)
    if bm_entry and "capabilities" in bm_entry:
        return dict(bm_entry["capabilities"])

    lowered = model_name.lower()
    tiers = infer_tier_eligibility(model_name, parameter_size=parameter_size, is_local=False)
    max_tier = max(tiers) if tiers else 3

    general = 0.65
    reasoning = 0.55
    coding = 0.5

    if max_tier >= 5:
        general = 0.88
        reasoning = 0.92
        coding = 0.82
    elif max_tier == 4:
        general = 0.78
        reasoning = 0.8
        coding = 0.72
    elif max_tier == 3:
        general = 0.68
        reasoning = 0.65
        coding = 0.6

    if any(marker in lowered for marker in _CODING_MARKERS):
        coding = max(coding, 0.9)
    elif "qwen" in lowered or "gpt-oss" in lowered:
        coding = max(coding, 0.7)

    capabilities: dict[str, Any] = {
        "general": round(general, 2),
        "reasoning": round(reasoning, 2),
        "coding": round(coding, 2),
    }
    if any(marker in lowered for marker in _VISION_MARKERS):
        capabilities["vision"] = True
    return capabilities


def _parse_billions(text: str) -> Optional[float]:
    match = _SIZE_RE.search(text)
    if not match:
        return None
    size = float(match.group("size"))
    suffix = match.group("suffix").lower()
    if suffix == "m":
        return size / 1000.0
    return size
