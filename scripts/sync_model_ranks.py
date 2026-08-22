#!/usr/bin/env python3
"""sync_model_ranks.py: Synchronize model leaderboard rankings into model_tier_benchmark.db.

Fetches model list from Cortex API (/models) or ModelRepository fallback,
queries community benchmark leaderboards (LMSYS Chatbot Arena / LiveBench feeds),
maps Elo scores to Cortex effort tiers (0-5) and capability priors,
and updates cortex/lib/dal/seeds/model_tier_benchmark.db.
"""

import json
import sqlite3
import sys
import urllib.request
from pathlib import Path
from typing import Any, Dict, List

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from lib.engine.curated_tier_catalog import CURATED_TIER_CATALOG

# Root directory of cortex
ROOT_DIR = Path(__file__).resolve().parent.parent
SEED_DB_FILE = ROOT_DIR / "lib" / "dal" / "seeds" / "model_tier_benchmark.db"

LMSYS_API_URL = "https://raw.githubusercontent.com/oolong-tea-2026/arena-ai-leaderboards/main/data/latest.json"


def fetch_cortex_models(api_url: str = "http://127.0.0.1:8003/models") -> List[Dict[str, Any]]:
    """Fetch current registered models from Cortex API endpoint or DB repository fallback."""
    try:
        req = urllib.request.Request(api_url, headers={"User-Agent": "Cortex-Sync/1.0"})
        with urllib.request.urlopen(req, timeout=5) as resp:
            if resp.status == 200:
                data = json.loads(resp.read().decode("utf-8"))
                print(f"[sync] Loaded {len(data)} models from Cortex API endpoint ({api_url}).")
                return data
    except Exception as exc:
        print(f"[sync] Could not connect to API ({exc}); attempting local DB fallback...")

    sys.path.insert(0, str(ROOT_DIR))
    try:
        from lib.dal.repositories.model_repository import ModelRepository
        repo = ModelRepository()
        models = repo.list_models()
        print(f"[sync] Loaded {len(models)} models from local database catalog.")
        return [
            {
                "id": m.id,
                "provider": m.provider,
                "tier_eligibility": m.tier_eligibility,
                "capabilities": m.capabilities,
                "is_local": m.is_local,
            }
            for m in models
        ]
    except Exception as exc:
        print(f"[sync] Local DB fallback skipped ({exc}).")
        return []


def fetch_online_leaderboard() -> Dict[str, float]:
    """Fetch LMSYS leaderboard ELO scores from public community archive or return fallback dictionary."""
    elo_scores: Dict[str, float] = {}
    try:
        req = urllib.request.Request(LMSYS_API_URL, headers={"User-Agent": "Cortex-Sync/1.0"})
        with urllib.request.urlopen(req, timeout=5) as resp:
            if resp.status == 200:
                payload = json.loads(resp.read().decode("utf-8"))
                for entry in payload.get("leaderboard", []):
                    name = entry.get("model") or entry.get("key") or ""
                    elo = entry.get("elo") or entry.get("score")
                    if name and elo:
                        elo_scores[str(name).lower()] = float(elo)
                print(f"[sync] Fetched {len(elo_scores)} model scores from LMSYS community feed.")
    except Exception as exc:
        print(f"[sync] Online leaderboard fetch info: {exc} (using catalog baselines).")

    return elo_scores


def elo_to_tier_eligibility(elo: float, is_local: bool = False, model_id: str = "") -> List[int]:
    """Convert Elo rating and local status to effort tier eligibility range."""
    if is_local:
        lowered = model_id.lower()
        if any(size in lowered for size in ["1b", "2b", "3b", "4b", "7b", "8b", "9b"]):
            return [0, 1]
        return [1, 2, 3]

    if elo >= 1350:
        return [4, 5]
    if elo >= 1280:
        return [3, 4, 5]
    if elo >= 1200:
        return [2, 3, 4]
    if elo >= 1100:
        return [1, 2, 3]
    return [0, 1, 2]


def elo_to_capabilities(elo: float, is_coding: bool = False, is_vision: bool = False) -> Dict[str, Any]:
    """Derive capability priors (0.0 to 1.0) from Elo score."""
    base = min(0.98, max(0.40, (elo - 900.0) / 500.0))
    coding = base + 0.05 if is_coding else base - 0.02
    reasoning = base + 0.03
    res: Dict[str, Any] = {
        "general": round(base, 2),
        "reasoning": round(min(0.98, reasoning), 2),
        "coding": round(min(0.98, coding), 2),
    }
    if is_vision:
        res["vision"] = True
    return res


def apply_curated_overrides(cursor: sqlite3.Cursor) -> None:
    """Make reviewed, single-tier assignments win over generic Elo fallback.

    Most provider catalog IDs do not exist in public leaderboards.  Their
    old shared ELO=1200 fallback made them indistinguishable and therefore
    unsafe as a runtime policy.  The reviewed runtime catalog is the source
    of truth for its small set of generation candidates.
    """
    for tier, candidates in CURATED_TIER_CATALOG.items():
        for candidate in candidates:
            cursor.execute(
                "UPDATE model_benchmarks SET tier_eligibility_json = ? WHERE model_key = ?",
                (json.dumps([tier]), candidate.model_id),
            )
    cursor.execute("INSERT OR REPLACE INTO benchmark_metadata VALUES ('version', '2.0-curated');")
    cursor.execute("INSERT OR REPLACE INTO benchmark_metadata VALUES ('source', 'Curated Cortex runtime catalog; generic LMSYS/LiveBench fallback retained as metadata only');")


def sync() -> None:
    cortex_models = fetch_cortex_models()
    online_scores = fetch_online_leaderboard()

    SEED_DB_FILE.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(SEED_DB_FILE))
    cursor = conn.cursor()

    cursor.execute("""
    CREATE TABLE IF NOT EXISTS benchmark_metadata (
        key TEXT PRIMARY KEY,
        value TEXT
    );
    """)

    cursor.execute("""
    CREATE TABLE IF NOT EXISTS tier_brackets (
        tier INTEGER PRIMARY KEY,
        min_elo REAL,
        label TEXT
    );
    """)

    cursor.execute("""
    CREATE TABLE IF NOT EXISTS model_benchmarks (
        model_key TEXT PRIMARY KEY,
        display_name TEXT,
        elo REAL,
        tier_eligibility_json TEXT,
        capabilities_json TEXT,
        is_local INTEGER DEFAULT 0
    );
    """)

    cursor.execute("INSERT OR REPLACE INTO benchmark_metadata VALUES ('version', '1.0');")
    cursor.execute("INSERT OR REPLACE INTO benchmark_metadata VALUES ('updated_at', '2026-08-22');")
    cursor.execute("INSERT OR REPLACE INTO benchmark_metadata VALUES ('source', 'LMSYS Chatbot Arena Leaderboard & LiveBench Suite');")

    cursor.execute("SELECT model_key FROM model_benchmarks;")
    existing_keys = {row[0] for row in cursor.fetchall()}

    updated_count = 0
    for model in cortex_models:
        model_id = model.get("id", "")
        if not model_id:
            continue
        lowered_id = model_id.lower()
        is_local = 1 if model.get("is_local") else 0

        matched_elo = None
        for k, v in online_scores.items():
            if k in lowered_id or lowered_id in k:
                matched_elo = v
                break

        elo = matched_elo or 1200.0
        is_coding = any(w in lowered_id for w in ["coder", "codex", "code"])
        is_vision = any(w in lowered_id for w in ["vision", "vl", "llava"])

        tier_json = json.dumps(elo_to_tier_eligibility(elo, is_local=bool(is_local), model_id=model_id))
        cap_json = json.dumps(elo_to_capabilities(elo, is_coding=is_coding, is_vision=is_vision))
        display_name = model.get("display_name") or model_id

        cursor.execute(
            "INSERT OR REPLACE INTO model_benchmarks VALUES (?, ?, ?, ?, ?, ?);",
            (model_id, display_name, elo, tier_json, cap_json, is_local),
        )
        updated_count += 1

    apply_curated_overrides(cursor)

    conn.commit()
    cursor.execute("SELECT COUNT(*) FROM model_benchmarks;")
    total_count = cursor.fetchone()[0]
    conn.close()

    print(f"[sync] Successfully updated {SEED_DB_FILE.relative_to(ROOT_DIR)} with {total_count} model benchmark entries ({updated_count} synced).")


if __name__ == "__main__":
    sync()
