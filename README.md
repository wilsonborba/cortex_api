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
They use test stores. Cache integration tests read only `CORTEX_TEST_REDIS_URL`
(set to a separate test cache index or a local throwaway Valkey via
`scripts/setup-test-valkey.sh`, never `CORTEX_REDIS_URL`) and skip when unset.
Local reports and benchmark data stay under ignored `docs/`.

The shield uses deterministic EN/PT/TH rules and the publisher ProtectAI
`deberta-v3-base-prompt-injection-v2` ONNX model on CPU. Its Apache-2.0 model
is English-focused, and the publisher project is archived. No multilingual or
jailbreak detection guarantee is made. Missing model files fail closed.

## Optional Bonsai profiles

Bonsai 2.5 T1 is disabled by default. To enable it, set the service-only
`CORTEX_BONSAI_ENABLED=true`; clients cannot choose a runtime endpoint or model
file. The API starts one owned Prism `llama-server` on loopback when a T1 chat
needs it, then stops it before local T0 inference or Whisper uses the GPU.
Omitted model selection defaults to the evaluated AtomicChat uncensored profile;
the official profile remains selectable. Both are text-only. Image requests keep
the existing T0 vision route.

Provision the pinned local files before enabling T1:

| Path | SHA-256 |
| --- | --- |
| `models/prism/llama-server` | `89e7f20b23b97bf77b91a0143a168763f7835fd6e0a7a7a23bb70951d991c02c` |
| `models/bonsai/bonsai25-atomicchat-uncensored-PTQ1_0.gguf` | `f6b5f856c3981ff5ba1db1ce13cdf1ca85f2f701d26ce287e03983b659b681b4` |
| `models/bonsai/bonsai25-official-PTQ1_0-mtp.gguf` | `e0fc5f3d47b9c926d52677d303b19df356a2376614b3e1cb9973865fc04ca431` |

The official profile uses the measured MTP export. The loader checks each
artifact hash before spawning the single fixed Prism command. Unknown profile
IDs, mismatched artifacts, failed readiness checks, and disabled T1 selection
fail closed. `install.sh --prepare` and `--check` verify these files when T1 is
enabled; neither command starts the model or changes a service.

Plain labelled context is the default selected by the fixed real-model experiment.
JSON and TOON remain configurable for model context. Service traffic stays JSON.
