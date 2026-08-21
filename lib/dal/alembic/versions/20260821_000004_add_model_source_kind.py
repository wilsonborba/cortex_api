"""Add source_kind to models

Revision ID: 20260821_000004
Revises: 20260819_000003
Create Date: 2026-08-21 15:30:00.000000
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "20260821_000004"
down_revision = "20260819_000003"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("models", sa.Column("source_kind", sa.String(length=16), nullable=False, server_default="api"))
    op.execute("UPDATE models SET source_kind='local' WHERE is_local = 1")
    op.execute("UPDATE models SET source_kind='cli' WHERE provider IN ('agy', 'claude', 'codex') AND is_local = 0")
    op.execute("UPDATE models SET source_kind='api' WHERE source_kind NOT IN ('cli', 'local')")


def downgrade() -> None:
    op.drop_column("models", "source_kind")
