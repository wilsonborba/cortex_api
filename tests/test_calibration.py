from __future__ import annotations

from pathlib import Path

from lib.core.settings import Settings
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.engine.calibration import CalibrationEngine, CalibrationResolver, CalibrationStore, JudgeDetector, JudgeInfo
from lib.engine.scoring import ModelScorer
from lib.engine.quota import QuotaTracker


class _FakeDriver:
    def __init__(self, response_text: str, *, success: bool = True) -> None:
        self.response_text = response_text
        self.success = success
        self.calls: list[tuple[str, str]] = []

    def run(self, model: str, prompt: str, images=None):
        from lib.engine.drivers.base import DriverResult

        self.calls.append((model, prompt))
        return DriverResult(
            success=self.success,
            response_text=self.response_text,
            input_tokens=11,
            output_tokens=17,
            latency_ms=1200,
            cost_usd=0.01,
            error_type=None if self.success else "cli_error",
            error_message=None if self.success else "failed",
            raw={},
        )


class _FakeRegistry:
    def __init__(self, model: ModelCatalogEntry) -> None:
        self.model = model

    def list_available_for_router(self, tier: int | None = None):
        if tier is None or tier in (self.model.tier_eligibility or []):
            return [self.model]
        return []


def _seed_store(path: Path, *, model_id: str, provider: str, tier: int, quality_score: float) -> None:
    store = CalibrationStore(path)
    run_id = store.start_run(profile="medium", judges=["codex"], source="personal" if "personal" in path.name else "canonical")
    store.replace_rankings(
        run_id,
        [{
            "tier": tier,
            "provider": provider,
            "model_id": model_id,
            "quality_score": quality_score,
            "aggregate_score": quality_score,
            "avg_latency_ms": 1000.0,
            "avg_cost_usd": 0.0,
            "disagreement": 0.0,
            "evaluation_count": 2,
            "task_count": 2,
            "rank_position": 1,
        }],
    )
    store.finish_run(run_id, status="completed")


def test_calibration_resolver_prefers_personal_then_canonical_then_default(tmp_path: Path):
    canonical = tmp_path / "canonical_calibration.db"
    personal = tmp_path / "personal_calibration.db"
    settings = Settings(calibration_canonical_db_path=canonical, calibration_personal_db_path=personal)

    _seed_store(canonical, model_id="claude/baseline", provider="claude", tier=3, quality_score=0.7)
    resolver = CalibrationResolver(settings=settings)
    assert resolver.status().source == "canonical"

    _seed_store(personal, model_id="codex/personal", provider="codex", tier=3, quality_score=0.9)
    assert resolver.status().source == "personal"

    personal.unlink()
    assert resolver.status().source == "canonical"

    canonical.unlink()
    assert resolver.status().source == "default"


def test_judge_detector_reports_available_tools(monkeypatch, tmp_path: Path):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    for name in ("agy", "agy-docker", "claude", "claude-docker", "codex"):
        path = bin_dir / name
        path.write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
        path.chmod(0o755)

    claude_creds = tmp_path / ".claude.json"
    claude_creds.write_text("{}", encoding="utf-8")
    codex_auth = tmp_path / "auth.json"
    codex_auth.write_text('{"auth_mode":"api_key"}', encoding="utf-8")

    monkeypatch.setenv("PATH", f"{bin_dir}:{Path().cwd()}")
    settings = Settings(
        agy_command=str(bin_dir / "agy"),
        agy_docker_command=str(bin_dir / "agy-docker"),
        claude_command=str(bin_dir / "claude"),
        claude_docker_command=str(bin_dir / "claude-docker"),
        codex_command=str(bin_dir / "codex"),
        claude_credentials_path=claude_creds,
        codex_auth_path=codex_auth,
        google_api_key="test-key",
    )
    judges = {judge.id: judge for judge in JudgeDetector(settings=settings).detect()}

    assert judges["agy"].available is True
    assert judges["claude"].available is True
    assert judges["codex"].available is True
    assert judges["agy-docker"].available is True
    assert judges["claude-docker"].available is True


def test_calibration_engine_writes_personal_only_and_keeps_canonical_unchanged(tmp_path: Path):
    canonical = tmp_path / "canonical_calibration.db"
    personal = tmp_path / "personal_calibration.db"
    _seed_store(canonical, model_id="claude/baseline", provider="claude", tier=2, quality_score=0.6)
    canonical_before = canonical.read_bytes()

    settings = Settings(
        calibration_canonical_db_path=canonical,
        calibration_personal_db_path=personal,
        calibration_max_models_per_tier=1,
    )
    model = ModelCatalogEntry(
        id="claude/test-model",
        provider="claude",
        display_name="Test Claude",
        access_status=AccessStatus.AVAILABLE.value,
        tier_eligibility=[2, 3, 4, 5],
        capabilities={"general": 0.4},
        is_enabled=True,
        is_local=False,
        cost_per_million_tokens=1.0,
    )
    registry = _FakeRegistry(model)
    candidate_driver = _FakeDriver("useful answer")
    judge_driver = _FakeDriver('{"score": 92, "reasoning": "strong answer"}')
    detector = type("_Detector", (), {"available": lambda self: [JudgeInfo("codex", "codex", "codex", "codex", True, "ok", "o3", "codex")]})()
    engine = CalibrationEngine(
        settings=settings,
        registry=registry,
        drivers={"claude": candidate_driver, "codex": judge_driver},
        detector=detector,
    )

    summary = engine.run(profile="medium", judge_ids=["codex"], show_progress=False)

    assert summary.source == "personal"
    assert summary.evaluations > 0
    assert personal.exists()
    assert canonical.read_bytes() == canonical_before


def test_scorer_uses_calibrated_quality_when_available(tmp_path: Path, telemetry_repo, quota_repo, model_repo):
    temp_dir = tmp_path / "calibration"
    temp_dir.mkdir(parents=True, exist_ok=True)
    canonical = temp_dir / "canonical_calibration.db"
    personal = temp_dir / "personal_calibration.db"
    for path in (canonical, personal):
        if path.exists():
            path.unlink()
    _seed_store(personal, model_id="claude/calibrated", provider="claude", tier=3, quality_score=0.93)

    settings = Settings(calibration_canonical_db_path=canonical, calibration_personal_db_path=personal)
    model_repo.upsert(ModelCatalogEntry(
        id="claude/calibrated",
        provider="claude",
        display_name="Calibrated Claude",
        access_status=AccessStatus.AVAILABLE.value,
        tier_eligibility=[3],
        capabilities={"general": 0.2},
        is_enabled=True,
        is_local=False,
        cost_per_million_tokens=1.0,
    ))
    tracker = QuotaTracker(quota_repo=quota_repo, model_repo=model_repo, settings=settings)
    scorer = ModelScorer(quota_tracker=tracker, telemetry_repo=telemetry_repo, calibration_resolver=CalibrationResolver(settings=settings))
    model = model_repo.get_by_id("claude/calibrated")

    result = scorer.score(model, "general", 45, requested_tier=3)

    assert result.capability == 0.93
