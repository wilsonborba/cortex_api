# Changelog


## v0.1.0 - 2026-08-20


### Features

- Enable full tier 0-5 eligibility across cloud and local providers


## v0.1.0 - 2026-08-20


### Bug Fixes

- Give the critic step the original draft, not just the refined answer (wilsonborba/cortex#19)

- Align HippocampusClient with the real hippocampus contract

- Move cortex API off port 8000 to avoid conflict with kgcb-api

- Correct ROOT_DIR path in uninstall.sh after move to scripts/

- Untrack docs/ directory per AI_AGENT_Working_Rules section 9 and consolidate in documentation/

- Make startup model discovery non-blocking and improve service manager fallback


### Chores

- Initial project scaffolding

- Release v0.1.0


### Documentation

- Add README describing project intent and tier architecture

- Comprehensive production README, service help enhancements, and verification

- Add dedicated documentation directory, provider guides, external tools setup, and multilingual scalar switcher

- Generate CHANGELOG.md for v0.1.0 via git-cliff

- Update CHANGELOG.md for v0.1.0

- Update changelog for scalar /docs route

- Update changelog for multilingual scalar specs


### Features

- Project scaffolding, dependency management and core settings (wilsonborba/cortex#1)

- Structured logging and real-time WebSocket log streaming (wilsonborba/cortex#2)

- Data Access Layer (DAL), SQLAlchemy models, Alembic migrations & test safety guardrail (wilsonborba/cortex#3)

- Model Registry, provider auto-discovery and access status management (wilsonborba/cortex#4)

- Real-time quota and token availability tracker with sliding window budget (wilsonborba/cortex#5)

- Real-time web search and content scraping (DuckDuckGo/SearXNG + Trafilatura/Crawl4AI) (wilsonborba/cortex#6)

- Hippocampus client adapter for semantic memory and S3 document context (wilsonborba/cortex#7)

- Routing engine, tier envelopes (T0-T5), dynamic scoring and manual pinning (wilsonborba/cortex#8)

- Multi-model execution engine and asynchronous telemetry logging (wilsonborba/cortex#9)

- Native FastAPI REST API with Ports & Adapters presentation layer (wilsonborba/cortex#10)

- OpenAI-compatible API facade (/v1/chat/completions + SSE streaming) (wilsonborba/cortex#11)

- Unified Typer CLI with full parity to the REST API (wilsonborba/cortex#12)

- Close the quota-cooldown loop with a real live-probe re-sync (wilsonborba/cortex#13)

- TOON encoder and extensible internal context-format registry (wilsonborba/cortex#14)

- Per-model context-format preference (auto + pin) with telemetry-driven evaluation (wilsonborba/cortex#15)

- Wire 3-layer context-format resolution (auto -> pin -> force) into the Executor (wilsonborba/cortex#16)

- CLI/API exposure for context-format preference (pin, force, inspect) (wilsonborba/cortex#17)

- Critic-driven revision loop with a bounded, structured verdict (wilsonborba/cortex#20)

- Add free-tier OpenAI-compatible provider adapters

- Add install/uninstall scripts for the API service

- Sanitize invisible Unicode from provider responses

- Accept audio/image attachments (local Whisper + Ollama vision)

- Understand video via async job (ffmpeg extract + Whisper + vision)

- Add hardware capability detection and auto profile evaluation

- Implement robust cross-platform installer, service manager, and clean uninstaller

- Add interactive Scalar endpoint and comprehensive multilingual API reference (EN, PT, TH)

- Add profile awareness, capability introspection, and graceful degradation for optional features

- Add interactive profile selection, smart port detection with reuse, and runtime capability summary

- Map /docs route directly to Scalar interactive documentation

- Add interactive multilingual language switcher and localized openapi specs for Scalar


### merge

- Feat: project scaffolding, dependency management and core settings (#1)

- Feat: structured logging and real-time WebSocket log streaming (#2)

- Feat: Data Access Layer (DAL), SQLAlchemy models, Alembic migrations & test safety guardrail (#3)

- Feat: Model Registry, provider auto-discovery and access status management (#4)

- Feat: real-time quota and token availability tracker with sliding window budget (#5)

- Feat: real-time web search and content scraping (DuckDuckGo/SearXNG + Trafilatura/Crawl4AI) (#6)

- Feat: Hippocampus client adapter for semantic memory and S3 document context (#7)

- Feat: routing engine, tier envelopes (T0-T5), dynamic scoring and manual pinning (#8)

- Feat: multi-model execution engine and asynchronous telemetry logging (#9)

- Feat: native FastAPI REST API with Ports & Adapters presentation layer (#10)

- Feat: OpenAI-compatible API facade (/v1/chat/completions + SSE streaming) (#11)

- Feat: unified Typer CLI with full parity to the REST API (#12)

- Feat: close the quota-cooldown loop with a real live-probe re-sync (#13)

- Feat: TOON encoder and extensible internal context-format registry (#14)

- Feat: per-model context-format preference (auto + pin) with telemetry-driven evaluation (#15)

- Feat: wire 3-layer context-format resolution (auto -> pin -> force) into the Executor (#16)

- Feat: CLI/API exposure for context-format preference (pin, force, inspect) (#17)

- Fix: give the critic step the original draft, not just the refined answer (#19)

- Feat: critic-driven revision loop with a bounded, structured verdict (#20)

- Feat: free-tier provider adapters + install/uninstall scripts (#21)

- Fix cortex uninstall.sh path after scripts/ move

- Incorporate hardware detection and profiles into release/main

- Incorporate robust installer, service lifecycle, and clean uninstaller into release/main

- Incorporate Scalar documentation and multilingual API references into release/main

- Incorporate production README and complete verification into release/main

- Incorporate runtime profile awareness and graceful capability degradation into release/main

- Incorporate interactive profile selection, smart port detection, and capability reporting into release/main

- Incorporate documentation directory and multilingual scalar navigation into release/main


