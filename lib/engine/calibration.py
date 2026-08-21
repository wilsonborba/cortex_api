from __future__ import annotations

import json
import shutil
import sqlite3
import statistics
import subprocess
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Iterable, Optional, Sequence

from rich.console import Console
from rich.progress import BarColumn, MofNCompleteColumn, Progress, SpinnerColumn, TaskProgressColumn, TextColumn, TimeElapsedColumn

from lib.core.settings import Settings, get_settings
from lib.dal.models import ModelCatalogEntry
from lib.engine.drivers.agy_docker import AgyDockerDriver
from lib.engine.drivers.base import DriverResult, parse_json_or_none
from lib.engine.drivers.claude_docker import ClaudeDockerDriver
from lib.engine.drivers.codex import CodexDriver
from lib.engine.registry_service import ModelRegistryService, build_default_registry_service

SUPPORTED_CALIBRATION_PROFILES = {"medium", "complete"}
CALIBRATION_TIERS = (2, 3, 4, 5)


@dataclass(frozen=True)
class BenchmarkTask:
    tier: int
    task_id: str
    prompt: str
    rubric: str


@dataclass(frozen=True)
class JudgeInfo:
    id: str
    family: str
    provider: str
    command: str
    available: bool
    reason: str
    default_model: str
    label: str


@dataclass(frozen=True)
class CalibrationSummary:
    run_id: str
    source: str
    judges: list[str]
    models_tested: int
    evaluations: int
    response_attempts: int
    response_failures: int
    best_models_by_tier: dict[int, str]
    source_path: Optional[str] = None


@dataclass(frozen=True)
class CalibrationSource:
    source: str
    path: Optional[Path]
    exists: bool
    updated_at: Optional[str]
    best_models_by_tier: dict[int, str]
    judges: list[str]
    rankings_by_tier: dict[int, list[dict]]
    evaluation_count: int = 0
    response_count: int = 0


BENCHMARK_TASKS: tuple[BenchmarkTask, ...] = (
    BenchmarkTask(
        tier=2,
        task_id="t2_summary",
        prompt="Summarize the tradeoffs between SQLite and PostgreSQL for a medium-sized internal tool.",
        rubric="Reward clarity, factual correctness, concise structure, and direct practical guidance.",
    ),
    BenchmarkTask(
        tier=2,
        task_id="t2_rewrite",
        prompt="Rewrite this support reply to sound clearer and more empathetic: 'We got your request. Wait.'",
        rubric="Reward tone, usefulness, and polished wording.",
    ),
    BenchmarkTask(
        tier=3,
        task_id="t3_debug",
        prompt="A Python function returns duplicated rows after a JOIN. Explain two likely causes and how to verify each.",
        rubric="Reward debugging usefulness, accuracy, and concrete verification steps.",
    ),
    BenchmarkTask(
        tier=3,
        task_id="t3_design",
        prompt="Outline a small REST API design for task tracking with projects, items, and status filters.",
        rubric="Reward coherent structure, practical endpoints, and sensible data modeling.",
    ),
    BenchmarkTask(
        tier=4,
        task_id="t4_architecture",
        prompt="Propose an architecture for a multi-provider AI router with fallback, quotas, and telemetry persistence.",
        rubric="Reward systems thinking, failure handling, and realistic implementation detail.",
    ),
    BenchmarkTask(
        tier=4,
        task_id="t4_review",
        prompt="Review this change request: migrate a sync HTTP client to async while preserving retries and timeouts. List key risks.",
        rubric="Reward depth, prioritization, and risk awareness.",
    ),
    BenchmarkTask(
        tier=5,
        task_id="t5_coding",
        prompt="Write a Python function that batches a list into chunks of size N and explain edge cases and tests.",
        rubric="Reward code quality, explanation depth, correctness, and test thinking.",
    ),
    BenchmarkTask(
        tier=5,
        task_id="t5_strategy",
        prompt="Design an evaluation plan for ranking LLMs across coding, writing, and research tasks without overfitting to speed.",
        rubric="Reward rigor, nuance, and evaluation methodology.",
    ),
)


class CalibrationStore:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._ensure_schema()

    def _connect(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self.path)
        conn.row_factory = sqlite3.Row
        return conn

    def _ensure_schema(self) -> None:
        with self._connect() as conn:
            conn.executescript(
                """
                CREATE TABLE IF NOT EXISTS calibration_runs (
                    run_id TEXT PRIMARY KEY,
                    profile TEXT NOT NULL,
                    source TEXT NOT NULL,
                    judges_json TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    finished_at TEXT,
                    status TEXT NOT NULL,
                    notes TEXT
                );

                CREATE TABLE IF NOT EXISTS benchmark_responses (
                    response_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    tier INTEGER NOT NULL,
                    task_id TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    model_id TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    response_text TEXT,
                    success INTEGER NOT NULL,
                    input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL,
                    latency_ms INTEGER NOT NULL,
                    cost_usd REAL NOT NULL,
                    error_type TEXT,
                    error_message TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES calibration_runs(run_id)
                );

                CREATE TABLE IF NOT EXISTS judge_evaluations (
                    evaluation_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    response_id TEXT NOT NULL,
                    judge_family TEXT NOT NULL,
                    judge_provider TEXT NOT NULL,
                    judge_model TEXT NOT NULL,
                    score REAL,
                    reasoning TEXT,
                    success INTEGER NOT NULL,
                    error_type TEXT,
                    error_message TEXT,
                    raw_json TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES calibration_runs(run_id),
                    FOREIGN KEY(response_id) REFERENCES benchmark_responses(response_id)
                );

                CREATE TABLE IF NOT EXISTS tier_rankings (
                    ranking_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    tier INTEGER NOT NULL,
                    provider TEXT NOT NULL,
                    model_id TEXT NOT NULL,
                    quality_score REAL NOT NULL,
                    aggregate_score REAL NOT NULL,
                    avg_latency_ms REAL NOT NULL,
                    avg_cost_usd REAL NOT NULL,
                    disagreement REAL NOT NULL,
                    evaluation_count INTEGER NOT NULL,
                    task_count INTEGER NOT NULL,
                    rank_position INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES calibration_runs(run_id)
                );
                """
            )

    def start_run(self, profile: str, judges: Sequence[str], source: str = "personal") -> str:
        run_id = str(uuid.uuid4())
        now = _utcnow()
        with self._connect() as conn:
            conn.execute(
                "INSERT INTO calibration_runs(run_id, profile, source, judges_json, started_at, status) VALUES (?, ?, ?, ?, ?, ?)",
                (run_id, profile, source, json.dumps(list(judges)), now, "running"),
            )
        return run_id

    def finish_run(self, run_id: str, status: str, notes: Optional[str] = None) -> None:
        with self._connect() as conn:
            conn.execute(
                "UPDATE calibration_runs SET finished_at=?, status=?, notes=? WHERE run_id=?",
                (_utcnow(), status, notes, run_id),
            )

    def record_response(
        self,
        *,
        run_id: str,
        tier: int,
        task_id: str,
        model: ModelCatalogEntry,
        prompt: str,
        result: DriverResult,
    ) -> str:
        response_id = str(uuid.uuid4())
        with self._connect() as conn:
            conn.execute(
                """
                INSERT INTO benchmark_responses(
                    response_id, run_id, tier, task_id, provider, model_id, prompt, response_text, success,
                    input_tokens, output_tokens, latency_ms, cost_usd, error_type, error_message, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    response_id,
                    run_id,
                    tier,
                    task_id,
                    model.provider,
                    model.id,
                    prompt,
                    result.response_text,
                    int(result.success),
                    result.input_tokens,
                    result.output_tokens,
                    result.latency_ms,
                    result.cost_usd,
                    result.error_type,
                    result.error_message,
                    _utcnow(),
                ),
            )
        return response_id

    def record_evaluation(
        self,
        *,
        run_id: str,
        response_id: str,
        judge: JudgeInfo,
        judge_model: str,
        score: Optional[float],
        reasoning: Optional[str],
        success: bool,
        error_type: Optional[str] = None,
        error_message: Optional[str] = None,
        raw_json: Optional[dict] = None,
    ) -> None:
        with self._connect() as conn:
            conn.execute(
                """
                INSERT INTO judge_evaluations(
                    evaluation_id, run_id, response_id, judge_family, judge_provider, judge_model,
                    score, reasoning, success, error_type, error_message, raw_json, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    str(uuid.uuid4()),
                    run_id,
                    response_id,
                    judge.family,
                    judge.provider,
                    judge_model,
                    score,
                    reasoning,
                    int(success),
                    error_type,
                    error_message,
                    json.dumps(raw_json or {}),
                    _utcnow(),
                ),
            )

    def replace_rankings(self, run_id: str, rankings: Sequence[dict]) -> None:
        with self._connect() as conn:
            conn.execute("DELETE FROM tier_rankings WHERE run_id=?", (run_id,))
            for row in rankings:
                conn.execute(
                    """
                    INSERT INTO tier_rankings(
                        ranking_id, run_id, tier, provider, model_id, quality_score, aggregate_score,
                        avg_latency_ms, avg_cost_usd, disagreement, evaluation_count, task_count, rank_position, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        str(uuid.uuid4()),
                        run_id,
                        row["tier"],
                        row["provider"],
                        row["model_id"],
                        row["quality_score"],
                        row["aggregate_score"],
                        row["avg_latency_ms"],
                        row["avg_cost_usd"],
                        row["disagreement"],
                        row["evaluation_count"],
                        row["task_count"],
                        row["rank_position"],
                        _utcnow(),
                    ),
                )

    def latest_run_id(self) -> Optional[str]:
        with self._connect() as conn:
            row = conn.execute(
                "SELECT run_id FROM calibration_runs WHERE status IN ('completed', 'partial') ORDER BY COALESCE(finished_at, started_at) DESC LIMIT 1"
            ).fetchone()
        return None if row is None else str(row["run_id"])

    def best_models_by_tier(self, run_id: Optional[str] = None) -> dict[int, str]:
        run_id = run_id or self.latest_run_id()
        if run_id is None:
            return {}
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT tier, model_id FROM tier_rankings WHERE run_id=? AND rank_position=1 ORDER BY tier ASC",
                (run_id,),
            ).fetchall()
        return {int(r["tier"]): str(r["model_id"]) for r in rows}

    def calibrated_quality(self, model_id: str, tier: int) -> Optional[float]:
        run_id = self.latest_run_id()
        if run_id is None:
            return None
        with self._connect() as conn:
            row = conn.execute(
                "SELECT quality_score FROM tier_rankings WHERE run_id=? AND tier=? AND model_id=? LIMIT 1",
                (run_id, tier, model_id),
            ).fetchone()
        return None if row is None else float(row["quality_score"])

    def metadata(self) -> Optional[dict]:
        run_id = self.latest_run_id()
        if run_id is None:
            return None
        with self._connect() as conn:
            run_row = conn.execute(
                "SELECT run_id, source, judges_json, profile, status, started_at, finished_at FROM calibration_runs WHERE run_id=?",
                (run_id,),
            ).fetchone()
            eval_count = conn.execute(
                "SELECT COUNT(*) AS count FROM judge_evaluations WHERE run_id=?",
                (run_id,),
            ).fetchone()["count"]
            response_count = conn.execute(
                "SELECT COUNT(*) AS count FROM benchmark_responses WHERE run_id=?",
                (run_id,),
            ).fetchone()["count"]
        return {
            "run_id": str(run_row["run_id"]),
            "source": str(run_row["source"]),
            "profile": str(run_row["profile"]),
            "status": str(run_row["status"]),
            "judges": json.loads(run_row["judges_json"]),
            "started_at": run_row["started_at"],
            "finished_at": run_row["finished_at"],
            "evaluation_count": int(eval_count),
            "response_count": int(response_count),
            "best_models_by_tier": self.best_models_by_tier(run_id),
        }

    def rankings(self, top_n: int = 3, run_id: Optional[str] = None) -> dict[int, list[dict]]:
        run_id = run_id or self.latest_run_id()
        if run_id is None:
            return {}
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT tier, model_id, provider, quality_score, aggregate_score, avg_latency_ms, avg_cost_usd, disagreement, evaluation_count, task_count, rank_position FROM tier_rankings WHERE run_id=? AND rank_position <= ? ORDER BY tier ASC, rank_position ASC",
                (run_id, top_n),
            ).fetchall()
        grouped: dict[int, list[dict]] = {}
        for row in rows:
            grouped.setdefault(int(row["tier"]), []).append({
                "rank": int(row["rank_position"]),
                "model_id": str(row["model_id"]),
                "provider": str(row["provider"]),
                "quality_score": float(row["quality_score"]),
                "aggregate_score": float(row["aggregate_score"]),
                "avg_latency_ms": float(row["avg_latency_ms"]),
                "avg_cost_usd": float(row["avg_cost_usd"]),
                "disagreement": float(row["disagreement"]),
                "evaluation_count": int(row["evaluation_count"]),
                "task_count": int(row["task_count"]),
            })
        return grouped


class CalibrationResolver:
    def __init__(self, settings: Optional[Settings] = None) -> None:
        self.settings = settings or get_settings()

    @property
    def personal_path(self) -> Path:
        return self.settings.calibration_personal_db_path

    @property
    def canonical_path(self) -> Path:
        return self.settings.calibration_canonical_db_path

    def resolve(self) -> tuple[str, Optional[CalibrationStore]]:
        for source, path in (("personal", self.personal_path), ("canonical", self.canonical_path)):
            if path.exists():
                store = CalibrationStore(path)
                if store.latest_run_id() is not None:
                    return source, store
        return "default", None

    def status(self, top_n: int = 3) -> CalibrationSource:
        source, store = self.resolve()
        if store is None:
            return CalibrationSource(source="default", path=None, exists=False, updated_at=None, best_models_by_tier={}, judges=[], rankings_by_tier={})
        meta = store.metadata() or {}
        active_path = self.personal_path if source == "personal" else self.canonical_path
        return CalibrationSource(
            source=source,
            path=active_path,
            exists=active_path.exists(),
            updated_at=meta.get("finished_at") or meta.get("started_at"),
            best_models_by_tier=meta.get("best_models_by_tier", {}),
            judges=list(meta.get("judges", [])),
            rankings_by_tier=store.rankings(top_n=top_n),
            evaluation_count=int(meta.get("evaluation_count", 0)),
            response_count=int(meta.get("response_count", 0)),
        )

    def get_quality(self, model_id: str, tier: int) -> Optional[float]:
        _, store = self.resolve()
        if store is None:
            return None
        return store.calibrated_quality(model_id=model_id, tier=tier)


class JudgeDetector:
    def __init__(self, settings: Optional[Settings] = None) -> None:
        self.settings = settings or get_settings()

    def detect(self) -> list[JudgeInfo]:
        judges = [
            self._detect_agy(),
            self._detect_claude(),
            self._detect_codex(),
        ]
        for extra in (self._detect_agy_docker(), self._detect_claude_docker(), self._detect_codex_docker()):
            if extra is not None:
                judges.append(extra)
        return judges

    def available(self) -> list[JudgeInfo]:
        return [judge for judge in self.detect() if judge.available]

    def _detect_agy(self) -> JudgeInfo:
        command = self.settings.agy_command
        binary = shutil.which(command)
        available = bool(binary and (self.settings.google_api_key or self.settings.google_ai_studio_api_key))
        reason = "command and Google credentials present" if available else "requires agy plus Google credentials"
        return JudgeInfo("agy", "agy", "agy", command, available, reason, "gemini-2.5-pro", "agy")

    def _detect_agy_docker(self) -> Optional[JudgeInfo]:
        command = self.settings.agy_docker_command
        if not command:
            return None
        binary = shutil.which(command)
        available = bool(binary and (self.settings.google_api_key or self.settings.google_ai_studio_api_key))
        reason = "custom AGY Docker command and Google credentials present" if available else "custom AGY Docker variant is configured but not ready"
        return JudgeInfo("agy-docker", "agy", "agy", command, available, reason, "gemini-2.5-pro", "agy-docker")

    def _detect_claude(self) -> JudgeInfo:
        command = self.settings.claude_command
        binary = shutil.which(command)
        creds = self.settings.claude_credentials_path.exists()
        available = bool(binary and creds)
        reason = "command and Claude credentials present" if available else "requires claude plus ~/.claude credentials"
        return JudgeInfo("claude", "claude", "claude", command, available, reason, "claude-opus-5", "claude")

    def _detect_claude_docker(self) -> Optional[JudgeInfo]:
        command = self.settings.claude_docker_command
        if not command:
            return None
        binary = shutil.which(command)
        creds = self.settings.claude_credentials_path.exists()
        available = bool(binary and creds)
        reason = "custom Claude Docker command and credentials present" if available else "custom Claude Docker variant is configured but not ready"
        return JudgeInfo("claude-docker", "claude", "claude", command, available, reason, "claude-opus-5", "claude-docker")

    def _detect_codex(self) -> JudgeInfo:
        command = self.settings.codex_command
        binary = shutil.which(command)
        creds = self.settings.codex_auth_path.exists()
        available = bool(binary and creds)
        reason = "command and Codex auth present" if available else "requires codex plus ~/.codex/auth.json"
        return JudgeInfo("codex", "codex", "codex", command, available, reason, "o3", "codex")

    def _detect_codex_docker(self) -> Optional[JudgeInfo]:
        command = self.settings.codex_docker_command
        if not command:
            return None
        binary = shutil.which(command)
        creds = self.settings.codex_auth_path.exists()
        available = bool(binary and creds)
        reason = "custom Codex Docker command and auth present" if available else "custom Codex Docker variant is configured but not ready"
        return JudgeInfo("codex-docker", "codex", "codex", command, available, reason, "o3", "codex-docker")


class CalibrationEngine:
    def __init__(
        self,
        settings: Optional[Settings] = None,
        registry: Optional[ModelRegistryService] = None,
        drivers: Optional[dict[str, object]] = None,
        detector: Optional[JudgeDetector] = None,
        console: Optional[Console] = None,
    ) -> None:
        self.settings = settings or get_settings()
        self.registry = registry or build_default_registry_service(settings=self.settings)
        self.drivers = drivers or build_default_drivers_for_calibration(self.settings)
        self.detector = detector or JudgeDetector(settings=self.settings)
        self.console = console or Console()
        self.personal_store = CalibrationStore(self.settings.calibration_personal_db_path)

    def run(
        self,
        *,
        profile: str,
        judge_ids: Optional[Sequence[str]] = None,
        show_progress: bool = True,
    ) -> CalibrationSummary:
        normalized_profile = profile.lower()
        if normalized_profile not in SUPPORTED_CALIBRATION_PROFILES:
            raise ValueError(f"Profile '{profile}' does not support calibration")

        judges = self._select_judges(judge_ids)
        if not judges:
            return CalibrationSummary(
                run_id="",
                source="default",
                judges=[],
                models_tested=0,
                evaluations=0,
                response_attempts=0,
                response_failures=0,
                best_models_by_tier=self.personal_store.best_models_by_tier(),
                source_path=None,
            )

        candidates = self._eligible_candidates()
        work_items = [
            (tier, task, model)
            for tier in CALIBRATION_TIERS
            for task in self._tasks_for_tier(tier)
            for model in candidates.get(tier, [])
        ]
        run_id = self.personal_store.start_run(profile=normalized_profile, judges=[j.id for j in judges])
        response_attempts = 0
        response_failures = 0
        evaluation_count = 0
        rankings: list[dict] = []
        all_scores: dict[tuple[int, str], dict] = {}

        progress = self._make_progress(show_progress)
        total_steps = sum(1 + len(judges) for _ in work_items)
        with progress:
            progress_task = progress.add_task("[cyan]Calibrating models", total=total_steps)
            for tier, task, model in work_items:
                progress.update(
                    progress_task,
                    description=f"[cyan]T{tier} {model.id} · generating response for {task.task_id}",
                )
                result = self._run_candidate(model, task.prompt)
                response_attempts += 1
                if not result.success:
                    response_failures += 1
                response_id = self.personal_store.record_response(
                    run_id=run_id,
                    tier=tier,
                    task_id=task.task_id,
                    model=model,
                    prompt=task.prompt,
                    result=result,
                )
                progress.advance(progress_task)

                bucket = all_scores.setdefault((tier, model.id), {
                    "tier": tier,
                    "provider": model.provider,
                    "model_id": model.id,
                    "quality_scores": [],
                    "latencies": [],
                    "costs": [],
                    "tasks": set(),
                    "judge_spreads": [],
                })
                bucket["tasks"].add(task.task_id)
                if result.success:
                    bucket["latencies"].append(float(result.latency_ms))
                    bucket["costs"].append(float(result.cost_usd))

                per_response_scores: list[float] = []
                for judge in judges:
                    progress.update(
                        progress_task,
                        description=f"[magenta]T{tier} {model.id} · judge={judge.family} · task={task.task_id}",
                    )
                    if not result.success:
                        self.personal_store.record_evaluation(
                            run_id=run_id,
                            response_id=response_id,
                            judge=judge,
                            judge_model=judge.default_model,
                            score=None,
                            reasoning=None,
                            success=False,
                            error_type=result.error_type,
                            error_message=result.error_message,
                        )
                        progress.advance(progress_task)
                        continue
                    score, reasoning, raw_json, error = self._evaluate_with_judge(judge, task, model, result.response_text, judges)
                    ok = score is not None and error is None
                    self.personal_store.record_evaluation(
                        run_id=run_id,
                        response_id=response_id,
                        judge=judge,
                        judge_model=judge.default_model,
                        score=score,
                        reasoning=reasoning,
                        success=ok,
                        error_type=None if ok else "judge_error",
                        error_message=error,
                        raw_json=raw_json,
                    )
                    if ok:
                        bucket["quality_scores"].append(float(score))
                        per_response_scores.append(float(score))
                        evaluation_count += 1
                    progress.advance(progress_task)
                if len(per_response_scores) > 1:
                    bucket["judge_spreads"].append(statistics.pstdev(per_response_scores))

        for tier in CALIBRATION_TIERS:
            tier_rows = []
            for (row_tier, _model_id), bucket in all_scores.items():
                if row_tier != tier or not bucket["quality_scores"]:
                    continue
                quality_score = statistics.fmean(bucket["quality_scores"]) / 100.0
                avg_latency_ms = statistics.fmean(bucket["latencies"]) if bucket["latencies"] else 0.0
                avg_cost_usd = statistics.fmean(bucket["costs"]) if bucket["costs"] else 0.0
                disagreement = statistics.fmean(bucket["judge_spreads"]) if bucket["judge_spreads"] else 0.0
                latency_budget_ms = float(self._latency_budget_ms(tier))
                latency_component = 1.0 - min(1.0, avg_latency_ms / latency_budget_ms) if latency_budget_ms > 0 else 0.5
                cost_component = 1.0 - min(1.0, avg_cost_usd / 0.05)
                aggregate_score = (quality_score * 0.80) + (latency_component * 0.15) + (cost_component * 0.05)
                tier_rows.append({
                    "tier": tier,
                    "provider": bucket["provider"],
                    "model_id": bucket["model_id"],
                    "quality_score": round(quality_score, 6),
                    "aggregate_score": round(aggregate_score, 6),
                    "avg_latency_ms": round(avg_latency_ms, 3),
                    "avg_cost_usd": round(avg_cost_usd, 6),
                    "disagreement": round(disagreement, 6),
                    "evaluation_count": len(bucket["quality_scores"]),
                    "task_count": len(bucket["tasks"]),
                })
            tier_rows.sort(key=lambda row: row["aggregate_score"], reverse=True)
            for index, row in enumerate(tier_rows, start=1):
                row["rank_position"] = index
                rankings.append(row)

        status = "completed" if response_failures == 0 else "partial"
        self.personal_store.replace_rankings(run_id, rankings)
        self.personal_store.finish_run(run_id, status=status, notes=None if rankings else "no successful evaluations")
        return CalibrationSummary(
            run_id=run_id,
            source="personal",
            judges=[judge.id for judge in judges],
            models_tested=len({model.id for models in candidates.values() for model in models}),
            evaluations=evaluation_count,
            response_attempts=response_attempts,
            response_failures=response_failures,
            best_models_by_tier=self.personal_store.best_models_by_tier(run_id),
            source_path=str(self.settings.calibration_personal_db_path),
        )

    def _make_progress(self, show_progress: bool) -> Progress:
        if not show_progress:
            return Progress(disable=True)
        return Progress(
            SpinnerColumn(),
            TextColumn("{task.description}"),
            BarColumn(bar_width=40),
            TaskProgressColumn(),
            MofNCompleteColumn(),
            TimeElapsedColumn(),
            console=self.console,
            transient=False,
        )

    def _select_judges(self, judge_ids: Optional[Sequence[str]]) -> list[JudgeInfo]:
        available = {judge.id: judge for judge in self.detector.available()}
        if not judge_ids:
            return list(available.values())
        selected = []
        for judge_id in judge_ids:
            normalized = judge_id.strip().lower()
            judge = available.get(normalized)
            if judge is not None:
                selected.append(judge)
        return selected

    def _eligible_candidates(self) -> dict[int, list[ModelCatalogEntry]]:
        candidates: dict[int, list[ModelCatalogEntry]] = {}
        for tier in CALIBRATION_TIERS:
            models = self.registry.list_available_for_router(tier=tier)
            models = [model for model in models if tier in (model.tier_eligibility or [])]
            models = sorted(models, key=lambda model: (not model.is_local, model.cost_per_million_tokens, model.id))
            candidates[tier] = models[: self.settings.calibration_max_models_per_tier]
        return candidates

    def _tasks_for_tier(self, tier: int) -> list[BenchmarkTask]:
        return [task for task in BENCHMARK_TASKS if task.tier == tier]

    def _run_candidate(self, model: ModelCatalogEntry, prompt: str) -> DriverResult:
        driver = self.drivers.get(model.provider)
        if driver is None:
            return DriverResult(
                success=False,
                response_text="",
                input_tokens=0,
                output_tokens=0,
                latency_ms=0,
                error_type="no_driver",
                error_message=f"no execution driver registered for {model.provider}",
            )
        bare_model = model.id.split("/", 1)[1] if "/" in model.id else model.id
        return driver.run(bare_model, prompt)

    def _evaluate_with_judge(
        self,
        judge: JudgeInfo,
        task: BenchmarkTask,
        model: ModelCatalogEntry,
        candidate_response: str,
        all_judges: Sequence[JudgeInfo],
    ) -> tuple[Optional[float], Optional[str], Optional[dict], Optional[str]]:
        driver = self._build_judge_driver(judge)
        if driver is None:
            return None, None, None, f"no judge driver registered for {judge.provider}"
        if len(all_judges) > 1 and judge.provider == model.provider:
            return None, None, None, "self-judging skipped because other judges were available"
        prompt = (
            "You are evaluating a model response for calibration. Reply with JSON only using keys "
            "score, reasoning, strengths, weaknesses. score must be an integer from 0 to 100.\n\n"
            f"Task tier: T{task.tier}\n"
            f"Rubric: {task.rubric}\n"
            f"Candidate model: {model.id}\n"
            f"Original task: {task.prompt}\n\n"
            f"Candidate response:\n{candidate_response}"
        )
        result = driver.run(judge.default_model, prompt)
        if not result.success:
            return None, None, result.raw, result.error_message or result.error_type or "judge execution failed"
        payload = _parse_json_payload(result.response_text)
        if payload is None:
            return None, None, result.raw, "judge did not return valid JSON"
        score = payload.get("score")
        try:
            parsed_score = float(score)
        except (TypeError, ValueError):
            return None, None, payload, "judge score missing or invalid"
        parsed_score = max(0.0, min(100.0, parsed_score))
        return parsed_score, str(payload.get("reasoning") or ""), payload, None

    def _build_judge_driver(self, judge: JudgeInfo):
        default_driver = self.drivers.get(judge.provider)
        if default_driver is not None:
            default_commands = {
                "agy": self.settings.agy_command,
                "claude": self.settings.claude_command,
                "codex": self.settings.codex_command,
            }
            if judge.command in {judge.provider, default_commands.get(judge.provider)}:
                return default_driver
        if judge.provider == "agy":
            return AgyDockerDriver(command=judge.command, timeout=self.settings.calibration_judge_timeout_seconds)
        if judge.provider == "claude":
            return ClaudeDockerDriver(command=judge.command, timeout=self.settings.calibration_judge_timeout_seconds)
        if judge.provider == "codex":
            return CodexDriver(command=judge.command, timeout=self.settings.calibration_judge_timeout_seconds)
        return default_driver

    def _latency_budget_ms(self, tier: int) -> int:
        budgets = {2: 15_000, 3: 30_000, 4: 45_000, 5: 90_000}
        return budgets.get(tier, 45_000)


def build_default_drivers_for_calibration(settings: Optional[Settings] = None) -> dict[str, object]:
    settings = settings or get_settings()
    from lib.engine.executor import build_default_drivers

    drivers = build_default_drivers(settings=settings)
    if "agy" not in drivers:
        drivers["agy"] = AgyDockerDriver(command=settings.agy_command, timeout=settings.calibration_judge_timeout_seconds)
    if "claude" not in drivers:
        drivers["claude"] = ClaudeDockerDriver(command=settings.claude_command, timeout=settings.calibration_judge_timeout_seconds)
    if "codex" not in drivers:
        drivers["codex"] = CodexDriver(command=settings.codex_command, timeout=settings.calibration_judge_timeout_seconds)
    return drivers


def _parse_json_payload(text: str) -> Optional[dict]:
    parsed = parse_json_or_none(text)
    if parsed is not None:
        return parsed
    start = text.find("{")
    end = text.rfind("}")
    if start == -1 or end == -1 or end <= start:
        return None
    return parse_json_or_none(text[start : end + 1])


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()
