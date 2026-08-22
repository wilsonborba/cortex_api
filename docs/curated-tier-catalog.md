# Curated runtime tier catalog

Provider discovery is inventory, not routing policy. The discovered catalog can
contain chat, embedding, speech, image, moderation and tool models. Runtime
generation is limited to the reviewed, ordered catalog in
`lib/engine/curated_tier_catalog.py`.

## Cascade contract

| Tier | Ordered path |
| --- | --- |
| 0 | Local Ollama, then terminal unavailable response |
| 1 | Five reviewed Tier-1 models, local Ollama, terminal response |
| 2 | Five reviewed Tier-2 models, local Ollama, terminal response |
| 3–5 | Reviewed candidates for that tier, AGY CLI, Codex CLI, terminal response |

Each reviewed model belongs to one tier only. A model assigned to Tier 3 or
higher can never be selected for Tiers 0–2. There is no adjacent-tier
borrowing.

The normal request deadline is 300 seconds and is absolute across the entire
cascade. `thinking=true` has a separate 600-second absolute deadline.

## Applying the catalog

Run the script against the local API after the service is running the matching
code revision:

```sh
.venv/bin/python scripts/apply_curated_tier_catalog.py
```

It updates the benchmark seed and uses `PATCH /models/{model_id}` plus
`PATCH /tiers/{tier}` to persist the reviewed assignments. It does not enable
providers or alter uncurated models.
