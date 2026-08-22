"""Add structured audit fields to telemetry events."""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "20260822_000005"
down_revision = "20260821_000004"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("telemetry_events", sa.Column("phase", sa.String(length=32), nullable=False, server_default="model"))
    op.add_column("telemetry_events", sa.Column("attempt_index", sa.Integer(), nullable=False, server_default="1"))
    op.add_column("telemetry_events", sa.Column("deadline_remaining_ms", sa.Integer(), nullable=True))
    op.add_column("telemetry_events", sa.Column("audit_details", sa.Text(), nullable=True))
    op.create_index("ix_telemetry_events_phase", "telemetry_events", ["phase"])


def downgrade() -> None:
    op.drop_index("ix_telemetry_events_phase", table_name="telemetry_events")
    op.drop_column("telemetry_events", "audit_details")
    op.drop_column("telemetry_events", "deadline_remaining_ms")
    op.drop_column("telemetry_events", "attempt_index")
    op.drop_column("telemetry_events", "phase")
