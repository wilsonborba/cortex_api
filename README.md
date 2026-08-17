# cortex

Multi-model orchestration system: routes AI requests across providers (local and external)
based on a **quality/effort tier**, not a fixed provider choice.

## Core idea

Tiers don't represent providers. They represent six levels of quality/effort, and any given
tier can be served by different providers/models depending on what performs best for that
task at that moment.

- **Model Registry** — knows what models exist and their capability scores (reasoning, coding,
  writing, latency, cost, tier range).
- **Router** — given a task and a requested tier, decides which model(s) and execution plan to
  use, from a single-model call up to multi-model pipelines (retrieval → model → refiner →
  critic).
- **Executor** — runs the chosen plan. Simple tiers are a direct call; higher tiers can involve
  multi-step orchestration (e.g. LangGraph) when a workflow actually needs it.
- **Telemetry** — every execution logs provider, model, task type, tokens, latency, cost, and
  quality signal. Over time this turns "which model is better?" into an evidence-based
  question instead of a subjective one.

## Tiers (T0–T5)

| Tier | Goal | Typical strategy |
|------|------|-------------------|
| T0 Basic | immediate response | 1 cheap/local model |
| T1 Light | slightly better | 1 model + simple context/RAG |
| T2 Standard | good general answer | mid model + retrieval/rerank |
| T3 Advanced | relevant reasoning | strong model, or cheap model → refiner |
| T4 High | high reliability | research + strong model + review |
| T5 Ultra | best possible result | decomposition + multiple models + critique/verification |

A tier defines a **budget/policy envelope** (max model calls, allow multi-model, allow
external calls, retrieval depth, latency budget) — not a fixed sequence of models. The Router
builds the execution plan from that envelope, evaluated against what's in the Model Registry.

## Status

Early architecture/design phase — see `docs/specs.md` (local, untracked) for the full design
discussion. No implementation yet.
