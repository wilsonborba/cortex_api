#!/usr/bin/env python3
"""Apply the reviewed tier catalog through Cortex's own configuration API.

The script updates the seed benchmark and then patches only the reviewed
generation models and tier policies.  It never enables/disables providers or
touches uncurated catalog entries.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from lib.engine.curated_tier_catalog import CURATED_TIER_CATALOG, model_ids_for_tier
from scripts.sync_model_ranks import SEED_DB_FILE, apply_curated_overrides


def request_json(method: str, url: str, payload: dict | None = None) -> dict:
    data = json.dumps(payload).encode() if payload is not None else None
    request = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.loads(response.read().decode())


def update_seed_benchmark() -> None:
    with sqlite3.connect(SEED_DB_FILE) as connection:
        apply_curated_overrides(connection.cursor())
    print(f"updated seed benchmark: {SEED_DB_FILE}")


def apply_api(api_url: str) -> None:
    api_url = api_url.rstrip("/")
    for tier, candidates in CURATED_TIER_CATALOG.items():
        for candidate in candidates:
            encoded_id = urllib.parse.quote(candidate.model_id, safe="/")
            try:
                request_json("PATCH", f"{api_url}/models/{encoded_id}", {"tier_eligibility": [tier]})
            except urllib.error.HTTPError as error:
                if error.code == 404:
                    raise RuntimeError(f"curated model missing from active catalog: {candidate.model_id}") from error
                raise
        request_json(
            "PATCH", f"{api_url}/tiers/{tier}",
            {"allowed_models": model_ids_for_tier(tier), "max_latency_seconds": 300},
        )
        print(f"configured tier {tier}: {len(candidates)} curated candidates")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api-url", default="http://127.0.0.1:8003")
    parser.add_argument("--seed-only", action="store_true")
    args = parser.parse_args()
    update_seed_benchmark()
    if not args.seed_only:
        apply_api(args.api_url)


if __name__ == "__main__":
    main()
