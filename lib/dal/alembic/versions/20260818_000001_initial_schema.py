"""Initial schema migration for Cortex tables

Revision ID: 20260818_000001
Revises: 
Create Date: 2026-08-18 15:00:00.000000
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "20260818_000001"
down_revision = None
branch_labels = None
depends_on = None


def upgrade() -> None:
    # 1. models table
    op.create_table(
        "models",
        sa.Column("id", sa.String(length=128), primary_key=True, nullable=False),
        sa.Column("provider", sa.String(length=64), nullable=False),
        sa.Column("display_name", sa.String(length=128), nullable=False),
        sa.Column("access_status", sa.String(length=32), nullable=False),
        sa.Column("status_reason", sa.String(length=256), nullable=True),
        sa.Column("parameter_size", sa.String(length=32), nullable=True),
        sa.Column("context_window", sa.Integer(), nullable=False, server_default="8192"),
        sa.Column("is_local", sa.Boolean(), nullable=False, server_default=sa.text("0")),
        sa.Column("tier_eligibility", sa.JSON(), nullable=False),
        sa.Column("capabilities", sa.JSON(), nullable=False),
        sa.Column("cost_per_million_tokens", sa.Float(), nullable=False, server_default="0.0"),
        sa.Column("is_enabled", sa.Boolean(), nullable=False, server_default=sa.text("1")),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_models_provider", "models", ["provider"])
    op.create_index("ix_models_access_status", "models", ["access_status"])

    # 2. telemetry_events table
    op.create_table(
        "telemetry_events",
        sa.Column("id", sa.String(length=64), primary_key=True, nullable=False),
        sa.Column("request_id", sa.String(length=64), nullable=False),
        sa.Column("execution_id", sa.String(length=64), nullable=False),
        sa.Column("strategy_id", sa.String(length=128), nullable=False),
        sa.Column("tier_requested", sa.Integer(), nullable=False),
        sa.Column("tier_executed", sa.Integer(), nullable=False),
        sa.Column("task_type", sa.String(length=64), nullable=False),
        sa.Column("provider", sa.String(length=64), nullable=False),
        sa.Column("model", sa.String(length=128), nullable=False),
        sa.Column("role", sa.String(length=32), nullable=False, server_default="primary"),
        sa.Column("input_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("output_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("total_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("started_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("finished_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("latency_seconds", sa.Float(), nullable=False, server_default="0.0"),
        sa.Column("latency_ms", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("success", sa.Boolean(), nullable=False, server_default=sa.text("1")),
        sa.Column("error_type", sa.String(length=128), nullable=True),
        sa.Column("retrieval_source", sa.String(length=32), nullable=True),
        sa.Column("retrieval_documents", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("retrieval_latency_ms", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("estimated_cost_usd", sa.Float(), nullable=False, server_default="0.0"),
        sa.Column("quality_score", sa.Float(), nullable=True),
        sa.Column("user_feedback", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_telemetry_events_request_id", "telemetry_events", ["request_id"])
    op.create_index("ix_telemetry_events_execution_id", "telemetry_events", ["execution_id"])
    op.create_index("ix_telemetry_events_strategy_id", "telemetry_events", ["strategy_id"])
    op.create_index("ix_telemetry_events_tier_requested", "telemetry_events", ["tier_requested"])
    op.create_index("ix_telemetry_events_task_type", "telemetry_events", ["task_type"])
    op.create_index("ix_telemetry_events_provider", "telemetry_events", ["provider"])
    op.create_index("ix_telemetry_events_model", "telemetry_events", ["model"])
    op.create_index("ix_telemetry_events_success", "telemetry_events", ["success"])
    op.create_index("idx_telemetry_strategy", "telemetry_events", ["strategy_id", "task_type", "success"])
    op.create_index("idx_telemetry_model", "telemetry_events", ["provider", "model", "started_at"])

    # 3. sliding_window_usage table
    op.create_table(
        "sliding_window_usage",
        sa.Column("id", sa.String(length=64), primary_key=True, nullable=False),
        sa.Column("provider", sa.String(length=64), nullable=False),
        sa.Column("model", sa.String(length=128), nullable=False),
        sa.Column("input_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("output_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("total_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("timestamp", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_sliding_window_usage_provider", "sliding_window_usage", ["provider"])
    op.create_index("ix_sliding_window_usage_timestamp", "sliding_window_usage", ["timestamp"])
    op.create_index("idx_usage_provider_time", "sliding_window_usage", ["provider", "timestamp"])

    # 4. routing_pins table
    op.create_table(
        "routing_pins",
        sa.Column("id", sa.String(length=64), primary_key=True, nullable=False),
        sa.Column("tier", sa.Integer(), nullable=False),
        sa.Column("task_type", sa.String(length=64), nullable=True),
        sa.Column("strategy_id", sa.String(length=128), nullable=True),
        sa.Column("model_id", sa.String(length=128), nullable=True),
        sa.Column("pinned_by", sa.String(length=128), nullable=True),
        sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.text("1")),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_routing_pins_tier", "routing_pins", ["tier"])
    op.create_index("ix_routing_pins_task_type", "routing_pins", ["task_type"])
    op.create_index("idx_pin_lookup", "routing_pins", ["tier", "task_type", "is_active"])

    # 5. tier_policies table
    op.create_table(
        "tier_policies",
        sa.Column("tier", sa.Integer(), primary_key=True, nullable=False),
        sa.Column("name", sa.String(length=64), nullable=False),
        sa.Column("max_latency_seconds", sa.Integer(), nullable=False),
        sa.Column("allow_multi_model", sa.Boolean(), nullable=False, server_default=sa.text("0")),
        sa.Column("allow_external", sa.Boolean(), nullable=False, server_default=sa.text("1")),
        sa.Column("retrieval_mode", sa.String(length=64), nullable=False, server_default="none"),
        sa.Column("require_verification", sa.Boolean(), nullable=False, server_default=sa.text("0")),
        sa.Column("max_model_calls", sa.Integer(), nullable=False, server_default="1"),
        sa.Column("allowed_models", sa.JSON(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )


def downgrade() -> None:
    op.drop_table("tier_policies")
    op.drop_table("routing_pins")
    op.drop_table("sliding_window_usage")
    op.drop_table("telemetry_events")
    op.drop_table("models")
