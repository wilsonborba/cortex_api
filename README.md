# Cortex API

Rust API for Cortex. The gateway owns authentication and supplies the user identity.
The API uses PostgreSQL for chat relations, Valkey for active text and scheduling,
Hippocampus for memory and files, and CouchDB for run records. Ollama runs locally.

## Run

Private credentials belong in `.env`. Non-secret settings belong in
`src/core/config.rs`. Development model files belong under the ignored `models/`
directory; the production Prism runtime and Bonsai weights are supplied as
explicit verified installer inputs and are not stored in the repository.

- `cargo run`: development listener and test stores.
- `cargo run -- --production`: port 8003, production stores and Hippo port 8001.
- `cargo run --bin migrate -- test`: explicit test database migrations.
- `cargo run --bin migrate -- cortex`: explicit production Cortex migrations.
- `./scripts/install.sh --prepare`: locked release build, staged binaries and units.
  `CARGO_TARGET_DIR` is supported. Run this step in the production checkout.
  When `CORTEX_BONSAI_ENABLED=true`, also set `CORTEX_PRISM_RUNTIME_SOURCE` to
  the pinned, extracted Prism runtime bundle and `CORTEX_BONSAI_MODEL_SOURCE`
  to the directory containing the two pinned model files. The runtime file list,
  symlink targets, lengths and SHA-256 values are recorded in
  `deploy/prism/runtime-manifest.json`; invalid or incomplete bundles fail closed.
- `./scripts/install.sh --check`: verify staged artifact hashes and source files.
- `./scripts/install.sh --activate`: administrator step. Installs the staged
  binaries before restart, then verifies each running executable hash.
  If Prism artifacts are staged, this installs its dedicated service account,
  versioned runtime, persistent weights and fixed service/control files. It
  leaves `prism.service` disabled and stopped; Prism remains on-demand.
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
the official profile remains selectable. Both profiles use text-only native
calls. On T1 image requests, one bounded call through the existing T0 vision
route adds a description before context assembly; every T1 call receives no
image payload. T0 image requests keep their existing route.

The production installer receives the verified runtime bundle and both pinned
model files as explicit inputs before T1 is enabled. Their destinations are the
versioned `/opt/prism/releases/` runtime (with `/opt/prism/current` as the stable
alias), `/var/lib/prism/models/bonsai/` for model weights, and `/etc/prism/` for
the pinned manifests. The service binds only to `127.0.0.1:49100`; no Prism API
key is configured. `--prepare` and `--check` only stage and verify files; they do
not install system users, change host paths, start the service, or load a model.

The accepted model pins are:

| Path | SHA-256 |
| --- | --- |
| `bonsai25-atomicchat-uncensored-PTQ1_0.gguf` | `f6b5f856c3981ff5ba1db1ce13cdf1ca85f2f701d26ce287e03983b659b681b4` |
| `bonsai25-official-PTQ1_0-mtp.gguf` | `e0fc5f3d47b9c926d52677d303b19df356a2376614b3e1cb9973865fc04ca431` |

The official profile uses the measured MTP export. The loader checks each
artifact hash before spawning the single fixed Prism command. Unknown profile
IDs, mismatched artifacts, failed readiness checks, and disabled T1 selection
fail closed. The accepted runtime server hash and complete bundled dependency
inventory are pinned in `deploy/prism/runtime-manifest.json`.

Plain labelled context is the default selected by the fixed real-model experiment.
JSON and TOON remain configurable for model context. Service traffic stays JSON.
