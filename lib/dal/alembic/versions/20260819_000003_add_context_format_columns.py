"""Add context-format preference columns (models + telemetry_events)

Revision ID: 20260819_000003
Revises: 20260818_000002
Create Date: 2026-08-19 12:00:00.000000
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "20260819_000003"
down_revision = "20260818_000002"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("models", sa.Column("context_format_computed", sa.String(length=16), nullable=True))
    op.add_column("models", sa.Column("context_format_pin", sa.String(length=16), nullable=True))
    op.add_column(
        "models", sa.Column("context_format_pin_expires_at", sa.DateTime(timezone=True), nullable=True)
    )
    op.add_column("telemetry_events", sa.Column("context_format", sa.String(length=16), nullable=True))
    op.add_column("telemetry_events", sa.Column("context_type", sa.String(length=16), nullable=True))


def downgrade() -> None:
    op.drop_column("telemetry_events", "context_type")
    op.drop_column("telemetry_events", "context_format")
    op.drop_column("models", "context_format_pin_expires_at")
    op.drop_column("models", "context_format_pin")
    op.drop_column("models", "context_format_computed")
