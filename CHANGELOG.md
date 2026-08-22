# Changelog


## Unreleased


### Bug Fixes

- Implement instant 2-layer SecurityShield evaluation and instant Proxy Passthrough for memory and tasks

- Remove claude from sidecars and drivers, use agy and codex as active CLI sidecars

- Resolve deadline latency budget exhaustion in executor step fallback and ensure SecurityShield direct return

- Restore verifiable security and tier contracts


### Documentation

- Update changelog for runtime contract fix


### Features

- Implement execution trace logging with auto-rotation in DAL

- Implement granular error taxonomy across drivers and pipeline

- Formalize opt-in capability contract with false defaults

- Implement tenant namespace isolation in HippocampusClient

- Implement plane-slim Task Management integration & PlaneClient

- Implement application tenant isolation boundaries and tenant_id schema

- Implement max_provider_candidates flag, smart fallbacks, dynamic test seed and visual test runner enhancements

- Implement issues #46-#50 (provider cooldown, ollama tier 0 fallback, security shield fix, document ingestion & plane-slim dynamic DB lifecycle)

- Implement issues #51-#55 (passthrough proxy, 2-layer security shield, real PDF/Whisper ingestion, hybrid tag extractor & multi-task loop)

- Replace placeholders with real multi-paragraph PDF and real spoken Portuguese WAV audio file via gTTS and pypdf

- Download external public audio sample from Wikimedia Commons and ingest both audio files in test runner

- Update all Tier preset policies (Tiers 0-5) to 180s max_latency_seconds budget

- Add structured execution audit fields


### Refactoring

- Restructure Cortex to align with standard project architecture (lib/domain & lib/dal)


### Tests

- Enhance test_tiers.py with capabilities, Hippocampus, plane-slim and security tests

- Update API HTTP client timeout to 180s (3 minutes)


### merge

- Integrate runtime contract fix (#57)


### security

- Implement context-aware SecurityShield & prompt injection guardrail


## v0.1.0 - 2026-08-22


### Bug Fixes

- Give the critic step the original draft, not just the refined answer (wilsonborba/cortex#19)

- Align HippocampusClient with the real hippocampus contract

- Move cortex API off port 8000 to avoid conflict with kgcb-api

- Correct ROOT_DIR path in uninstall.sh after move to scripts/

- Untrack docs/ directory per AI_AGENT_Working_Rules section 9 and consolidate in documentation/

- Make startup model discovery non-blocking and improve service manager fallback

- Make external CLI binaries explicit and portable

- Make docker judge variants opt-in

- Align default judge models and codex catalog with supported CLI flags (#33)


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

- Update changelog for tier 0-5 eligibility expansion

- Update release notes for routing quality improvements

- Update release notes for routing follow-ups

- Update release notes for deploy-safe cli mapping


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

- Enable full tier 0-5 eligibility across cloud and local providers

- Improve tier quality controls and prompt normalization

- Expose quality controls and improve retrieval heuristics

- Align installer judges and catalog provenance

- Persist task bank and benchmark all eligible models

- Implement static tier catalog, configurable timeouts, terminal fallback and web references


### Refactoring

- Use generic custom judge command lists


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

- Issue #24 routing quality improvements into release/main

- Issues #25 #26 #27 routing follow-ups into release/main

- Issues #28 #29 deploy-safe cli mapping into release/main

- Align calibration install flow into release/main

- Finalize calibration install flow in release/main

- Finalize generic judge command lists in release/main

- Issues #33 #34 #35 full calibration sweep into release/main


