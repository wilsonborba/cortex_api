# Cortex API

Rust API for Cortex. The gateway owns authentication and supplies the user identity.
The API uses PostgreSQL for chat relations, Valkey for active text and scheduling,
Hippocampus for memory and files, and CouchDB for run records. Ollama runs locally.

## Run

Private credentials belong in `.env`. Non-secret settings belong in
`src/core/config.rs`. Keep model files under the ignored `models/` directory.

- `cargo run`: development listener and test stores.
- `cargo run -- --production`: port 8003, production stores and Hippo port 8001.
- `cargo run --bin migrate -- test`: explicit test database migrations.
- `cargo run --bin migrate -- cortex`: explicit production Cortex migrations.
- `./scripts/install.sh --prepare`: locked release build, staged binaries and units.
  `CARGO_TARGET_DIR` is supported. Run this step in the production checkout.
- `./scripts/install.sh --check`: verify staged artifact hashes and source files.
- `./scripts/install.sh --activate`: administrator step. Installs the staged
  binaries before restart, then verifies each running executable hash.
  Missing, changed or stale prepared artifacts are rejected before service changes.
  Run only in the coordinated deployment window.

Installation never migrates a database. Back up Hippo and stop both legacy APIs
before its schema migration. The original annotated `v1.0.0` tag stays available
for rollback. A database restore is required before starting Hippo v1 again.

## Checks

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.
Ignored tests need explicit local dependencies or the approved MSI GPU window.
They use test stores. Local reports and benchmark data stay under ignored `docs/`.

The shield uses deterministic EN/PT/TH rules and the publisher ProtectAI
`deberta-v3-base-prompt-injection-v2` ONNX model on CPU. Its Apache-2.0 model
is English-focused, and the publisher project is archived. No multilingual or
jailbreak detection guarantee is made. Missing model files fail closed.

Plain labelled context is the default selected by the fixed real-model experiment.
JSON and TOON remain configurable for model context. Service traffic stays JSON.
