"""Add conversations table for CRUD metadata (title/pin/soft-delete)."""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "20260909_000006"
down_revision = "20260822_000005"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "conversations",
        sa.Column("id", sa.String(length=64), primary_key=True, nullable=False),
        sa.Column("tenant_id", sa.String(length=64), nullable=False, server_default="default"),
        sa.Column("title", sa.String(length=256), nullable=False, server_default="New Conversation"),
        sa.Column("is_pinned", sa.Boolean(), nullable=False, server_default=sa.text("0")),
        sa.Column("deleted_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_conversations_tenant_id", "conversations", ["tenant_id"])
    op.create_index("idx_conversation_lookup", "conversations", ["tenant_id", "deleted_at"])


def downgrade() -> None:
    op.drop_index("idx_conversation_lookup", table_name="conversations")
    op.drop_index("ix_conversations_tenant_id", table_name="conversations")
    op.drop_table("conversations")
