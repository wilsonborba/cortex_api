from __future__ import annotations

import typer

from lib.core.settings import get_settings
from lib.engine.calibration import CalibrationEngine, CalibrationResolver, JudgeDetector, SUPPORTED_CALIBRATION_PROFILES
from lib.presentation.cli.output import emit, error_exit, print_table

app = typer.Typer(help="Inspect and run model auto-calibration.")


@app.command("status")
def status(ctx: typer.Context, top: int = typer.Option(3, "--top", help="Top N rankings to display per tier")) -> None:
    resolver = CalibrationResolver(settings=get_settings())
    source = resolver.status(top_n=top)
    payload = {
        "source": source.source,
        "path": str(source.path) if source.path else None,
        "exists": source.exists,
        "updated_at": source.updated_at,
        "judges": source.judges,
        "evaluation_count": source.evaluation_count,
        "response_count": source.response_count,
        "best_models_by_tier": source.best_models_by_tier,
        "rankings_by_tier": source.rankings_by_tier,
    }

    def _table() -> None:
        print_table(
            "Calibration",
            ["SOURCE", "PATH", "UPDATED", "JUDGES", "EVALS", "RESPONSES"],
            [[source.source, source.path or "-", source.updated_at or "-", source.judges or "-", source.evaluation_count, source.response_count]],
        )
        ranking_rows = []
        for tier, rows in sorted(source.rankings_by_tier.items()):
            for row in rows:
                ranking_rows.append([
                    tier,
                    row["rank"],
                    row["model_id"],
                    row["provider"],
                    round(row["aggregate_score"], 4),
                    round(row["quality_score"], 4),
                    round(row["avg_latency_ms"], 1),
                ])
        print_table(
            f"Top {top} Per Tier",
            ["TIER", "RANK", "MODEL", "PROVIDER", "AGG", "QUALITY", "LATENCY MS"],
            ranking_rows,
        )

    emit(ctx, json_data=payload, table=_table)


@app.command("judges")
def judges(ctx: typer.Context) -> None:
    detected = JudgeDetector(settings=get_settings()).detect()
    payload = [
        {
            "id": judge.id,
            "label": judge.label,
            "family": judge.family,
            "provider": judge.provider,
            "command": judge.command,
            "available": judge.available,
            "reason": judge.reason,
            "default_model": judge.default_model,
        }
        for judge in detected
    ]
    emit(
        ctx,
        json_data=payload,
        table=lambda: print_table(
            "Judges",
            ["ID", "FAMILY", "AVAILABLE", "COMMAND", "DEFAULT MODEL", "DETAIL"],
            [[j["id"], j["family"], j["available"], j["command"], j["default_model"], j["reason"]] for j in payload],
        ),
    )


@app.command("run")
def run(
    ctx: typer.Context,
    profile: str = typer.Option("complete", "--profile", help="medium or complete"),
    judges: str = typer.Option("all", "--judges", help="comma-separated judge ids or 'all'"),
    no_progress: bool = typer.Option(False, "--no-progress", help="Disable the progress bar"),
) -> None:
    normalized_profile = profile.lower()
    if normalized_profile not in SUPPORTED_CALIBRATION_PROFILES:
        error_exit("Calibration is supported only for profiles 'medium' and 'complete'.")
    detector = JudgeDetector(settings=get_settings())
    available = detector.available()
    selected_ids = [judge.id for judge in available] if judges.lower() == "all" else [item.strip().lower() for item in judges.split(",") if item.strip()]
    engine = CalibrationEngine(settings=get_settings())
    summary = engine.run(profile=normalized_profile, judge_ids=selected_ids, show_progress=not no_progress)
    payload = {
        "run_id": summary.run_id,
        "source": summary.source,
        "source_path": summary.source_path,
        "judges": summary.judges,
        "models_tested": summary.models_tested,
        "evaluations": summary.evaluations,
        "response_attempts": summary.response_attempts,
        "response_failures": summary.response_failures,
        "best_models_by_tier": summary.best_models_by_tier,
    }
    emit(
        ctx,
        json_data=payload,
        table=lambda: print_table(
            "Calibration Run",
            ["RUN", "JUDGES", "MODELS", "EVALS", "RESPONSES", "FAILURES", "BEST"],
            [[summary.run_id or "skipped", summary.judges or "-", summary.models_tested, summary.evaluations, summary.response_attempts, summary.response_failures, summary.best_models_by_tier or "-"]],
        ),
    )
