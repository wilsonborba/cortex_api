from __future__ import annotations

import uuid
from datetime import datetime, timezone
from enum import Enum
from typing import Any, List, Optional

from sqlalchemy import (
    JSON,
    Boolean,
    DateTime,
    Float,
    Index,
    Integer,
    String,
    Text,
)
from sqlalchemy.orm import Mapped, mapped_column

from lib.dal.local.database import Base, TimestampMixin


class AccessStatus(str, Enum):
    AVAILABLE = "AVAILABLE"
    REQUIRES_SUBSCRIPTION = "REQUIRES_SUBSCRIPTION"
    DISABLED_MANUALLY = "DISABLED_MANUALLY"
    COOLING_DOWN = "COOLING_DOWN"
    OFFLINE = "OFFLINE"


class ModelCatalogEntry(Base, TimestampMixin):
    __tablename__ = "models"

    id: Mapped[str] = mapped_column(String(128), primary_key=True)
    provider: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    display_name: Mapped[str] = mapped_column(String(128), nullable=False)
    access_status: Mapped[str] = mapped_column(
        String(32), default=AccessStatus.AVAILABLE.value, nullable=False, index=True
    )
    status_reason: Mapped[Optional[str]] = mapped_column(String(256), nullable=True)
    parameter_size: Mapped[Optional[str]] = mapped_column(String(32), nullable=True)
    context_window: Mapped[int] = mapped_column(Integer, default=8192, nullable=False)
    is_local: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    tier_eligibility: Mapped[List[int]] = mapped_column(JSON, default=list, nullable=False)
    capabilities: Mapped[dict[str, Any]] = mapped_column(JSON, default=dict, nullable=False)
    cost_per_million_tokens: Mapped[float] = mapped_column(Float, default=0.0, nullable=False)
    is_enabled: Mapped[bool] = mapped_column(Boolean, default=True, nullable=False)
    cooldown_until: Mapped[Optional[datetime]] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    # Internal context-format communication (issue #15): layer 1 is
    # `context_format_computed` (the evaluation algorithm's current best
    # guess, None means "no data yet, default to TOON"); layer 2 is
    # `context_format_pin` (a manual override, optionally time-limited via
    # `context_format_pin_expires_at`); layer 3 (request-level force) never
    # touches these columns, it's resolved per-call in the Executor (#16).
    context_format_computed: Mapped[Optional[str]] = mapped_column(String(16), nullable=True)
    context_format_pin: Mapped[Optional[str]] = mapped_column(String(16), nullable=True)
    context_format_pin_expires_at: Mapped[Optional[datetime]] = mapped_column(
        DateTime(timezone=True), nullable=True
    )


class TelemetryEvent(Base, TimestampMixin):
    __tablename__ = "telemetry_events"

    id: Mapped[str] = mapped_column(
        String(64), primary_key=True, default=lambda: str(uuid.uuid4())
    )
    request_id: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    execution_id: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    strategy_id: Mapped[str] = mapped_column(String(128), nullable=False, index=True)
    tier_requested: Mapped[int] = mapped_column(Integer, nullable=False, index=True)
    tier_executed: Mapped[int] = mapped_column(Integer, nullable=False)
    task_type: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    provider: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    model: Mapped[str] = mapped_column(String(128), nullable=False, index=True)
    role: Mapped[str] = mapped_column(String(32), default="primary", nullable=False)
    input_tokens: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    output_tokens: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    total_tokens: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    started_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    finished_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    latency_seconds: Mapped[float] = mapped_column(Float, default=0.0, nullable=False)
    latency_ms: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    success: Mapped[bool] = mapped_column(Boolean, default=True, nullable=False, index=True)
    error_type: Mapped[Optional[str]] = mapped_column(String(128), nullable=True)
    retrieval_source: Mapped[Optional[str]] = mapped_column(String(32), nullable=True)
    retrieval_documents: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    retrieval_latency_ms: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    estimated_cost_usd: Mapped[float] = mapped_column(Float, default=0.0, nullable=False)
    quality_score: Mapped[Optional[float]] = mapped_column(Float, nullable=True)
    user_feedback: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    # Which internal communication format this step's context injection
    # used, and what kind of context it was -- null/null for a step with no
    # web/memory context at all. Feeds issue #15's evaluation algorithm.
    context_format: Mapped[Optional[str]] = mapped_column(String(16), nullable=True)
    context_type: Mapped[Optional[str]] = mapped_column(String(16), nullable=True)

    __table_args__ = (
        Index("idx_telemetry_strategy", "strategy_id", "task_type", "success"),
        Index("idx_telemetry_model", "provider", "model", "started_at"),
    )


class SlidingWindowUsage(Base, TimestampMixin):
    __tablename__ = "sliding_window_usage"

    id: Mapped[str] = mapped_column(
        String(64), primary_key=True, default=lambda: str(uuid.uuid4())
    )
    provider: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    model: Mapped[str] = mapped_column(String(128), nullable=False, index=True)
    input_tokens: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    output_tokens: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    total_tokens: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    timestamp: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False, index=True
    )

    __table_args__ = (
        Index("idx_usage_provider_time", "provider", "timestamp"),
    )


class RoutingPin(Base, TimestampMixin):
    __tablename__ = "routing_pins"

    id: Mapped[str] = mapped_column(
        String(64), primary_key=True, default=lambda: str(uuid.uuid4())
    )
    tier: Mapped[int] = mapped_column(Integer, nullable=False, index=True)
    task_type: Mapped[Optional[str]] = mapped_column(String(64), nullable=True, index=True)
    strategy_id: Mapped[Optional[str]] = mapped_column(String(128), nullable=True)
    model_id: Mapped[Optional[str]] = mapped_column(String(128), nullable=True)
    pinned_by: Mapped[Optional[str]] = mapped_column(String(128), nullable=True)
    is_active: Mapped[bool] = mapped_column(Boolean, default=True, nullable=False)

    __table_args__ = (
        Index("idx_pin_lookup", "tier", "task_type", "is_active"),
    )


class TierPolicy(Base, TimestampMixin):
    __tablename__ = "tier_policies"

    tier: Mapped[int] = mapped_column(Integer, primary_key=True)
    name: Mapped[str] = mapped_column(String(64), nullable=False)
    max_latency_seconds: Mapped[int] = mapped_column(Integer, nullable=False)
    allow_multi_model: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    allow_external: Mapped[bool] = mapped_column(Boolean, default=True, nullable=False)
    retrieval_mode: Mapped[str] = mapped_column(String(64), default="none", nullable=False)
    require_verification: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    max_model_calls: Mapped[int] = mapped_column(Integer, default=1, nullable=False)
    allowed_models: Mapped[Optional[List[str]]] = mapped_column(JSON, nullable=True)
