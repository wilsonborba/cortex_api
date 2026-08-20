from __future__ import annotations

from logging.config import fileConfig
from alembic import context

from lib.dal.local.database import Base, get_engine
from lib.dal.models import (
    ModelCatalogEntry,
    RoutingPin,
    SlidingWindowUsage,
    TelemetryEvent,
    TierPolicy,
)

_imported_for_autogenerate = (
    ModelCatalogEntry,
    RoutingPin,
    SlidingWindowUsage,
    TelemetryEvent,
    TierPolicy,
)

config = context.config

if config.config_file_name is not None:
    fileConfig(config.config_file_name)

target_metadata = Base.metadata


def run_migrations_offline() -> None:
    engine = get_engine()
    context.configure(
        url=str(engine.url),
        target_metadata=target_metadata,
        literal_binds=True,
    )
    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    engine = get_engine()
    with engine.connect() as connection:
        context.configure(connection=connection, target_metadata=target_metadata)
        with context.begin_transaction():
            context.run_migrations()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()
