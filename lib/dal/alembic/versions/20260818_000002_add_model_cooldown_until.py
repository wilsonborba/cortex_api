"""Add cooldown_until to models

Revision ID: 20260818_000002
Revises: 20260818_000001
Create Date: 2026-08-18 16:00:00.000000
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "20260818_000002"
down_revision = "20260818_000001"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "models", sa.Column("cooldown_until", sa.DateTime(timezone=True), nullable=True)
    )


def downgrade() -> None:
    op.drop_column("models", "cooldown_until")
