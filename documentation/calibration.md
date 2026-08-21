# Model Auto-Calibration

Cortex supports an optional model auto-calibration workflow that benchmarks eligible models for `T2` to `T5`, asks supported external judges to score the responses, and persists a reusable baseline for future routing.

## Why this exists

Fresh installs often know which models are *available*, but not which ones are best for each tier on that specific machine/account mix.
Calibration gives Cortex an initial quality prior before runtime telemetry has enough data to learn on its own.

## Source precedence

Calibration resolution always follows:

```text
personal_calibration.db
        ↓
canonical_calibration.db
        ↓
default selector metadata and fallback scoring
```

- `var/personal_calibration.db`
  - local override
  - Git-ignored
  - created/updated by normal calibration runs
- `var/canonical_calibration.db`
  - distributed repository baseline
  - Git-tracked
  - never written by ordinary calibration

Removing `personal_calibration.db` naturally exposes the canonical baseline again on the next load.

## Profiles

### Light

- never runs calibration;
- never requires judge CLIs;
- only reads `Personal > Canonical > Default`.

### Medium / Complete

- may run calibration during installation or later via CLI;
- detect available judges automatically;
- keep installation successful even when calibration is skipped, partial, or unavailable.

## Supported judges

Cortex currently auto-detects these judge families:

- `gemini` via `agy` plus Google credentials;
- `claude` via `claude` plus Claude credentials;
- `codex` via `codex` plus `~/.codex/auth.json`.

Use:

```bash
cortex calibration judges
```

## Progress reporting

Calibration uses an informative progress bar that shows:

- current tier;
- current candidate model;
- whether Cortex is generating or judging;
- current task id;
- completed vs total steps;
- elapsed time.

This same progress output appears when calibration is launched from `./scripts/install.sh`.

## Running calibration manually

```bash
cortex calibration status
cortex calibration judges
cortex calibration run --profile medium --judges all
cortex calibration run --profile complete --judges codex,claude
```

Normal calibration writes only `var/personal_calibration.db`.

## How ranking is computed

For each eligible `tier × task × model` combination, Cortex:

1. generates a real candidate response using the provider driver;
2. asks one or more judges for a structured JSON score;
3. stores individual judge evaluations;
4. aggregates quality, disagreement, latency, and cost;
5. ranks models per tier.

Aggregation is quality-dominant by design:

- quality: 80%
- latency: 15%
- cost: 5%

At runtime, the selector still uses the normal routing formula, but calibrated quality now has substantially more influence than raw speed.

## Runtime learning vs calibration

Calibration is an initial baseline.
Runtime telemetry and normal routing still continue to evolve behavior afterward; calibration does not permanently freeze rankings.

## Publishing a new canonical baseline

A maintainer can intentionally publish a local baseline as the distributed baseline by renaming:

```bash
mv var/personal_calibration.db var/canonical_calibration.db
```

No schema conversion is required because both databases share the same schema.
