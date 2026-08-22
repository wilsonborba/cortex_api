from __future__ import annotations

import os
from pathlib import Path
from typing import Iterator

import pytest
from sqlalchemy import create_engine
from sqlalchemy.orm import Session, sessionmaker

# 1. Set environment variable to isolated test DB BEFORE any database module is loaded
_TEST_DB_PATH = Path(__file__).resolve().parent.parent / "lib" / "dal" / "var" / "test_cortex.db"
os.environ.setdefault("CORTEX_DATABASE_URL", f"sqlite:///{_TEST_DB_PATH}")
# Tests must not inherit a personal runtime policy from the repository .env.
os.environ["CORTEX_DISABLED_PROVIDERS"] = ""

from alembic.command import upgrade  # noqa: E402
from alembic.config import Config  # noqa: E402

from lib.dal.local.database import Base, get_engine, set_engine_and_session  # noqa: E402
from lib.dal.migrations import ALEMBIC_INI_PATH  # noqa: E402
from lib.dal.repositories.model_repository import ModelRepository  # noqa: E402
from lib.dal.repositories.pin_repository import RoutingPinRepository  # noqa: E402
from lib.dal.repositories.quota_repository import QuotaRepository  # noqa: E402
from lib.dal.repositories.telemetry_repository import TelemetryRepository  # noqa: E402
from lib.dal.repositories.tier_policy_repository import TierPolicyRepository  # noqa: E402


def pytest_configure(config) -> None:
    # 2. Strict Safety Guardrail: Inspect engine database URL
    test_engine = get_engine(f"sqlite:///{_TEST_DB_PATH}")
    db_url = str(test_engine.url)
    if "lib/dal/var/cortex.db" in db_url or "test" not in db_url.lower():
        raise RuntimeError(
            f"Refusing to run tests against {db_url!r}: it looks like the real database, "
            "not an isolated test database. Guardrail aborted test execution."
        )

    # 3. Clean recreation of schema for total isolation
    if _TEST_DB_PATH.exists():
        _TEST_DB_PATH.unlink()

    _TEST_DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    alembic_config = Config(str(ALEMBIC_INI_PATH))
    alembic_config.set_main_option("sqlalchemy.url", f"sqlite:///{_TEST_DB_PATH}")
    upgrade(alembic_config, "head")

    set_engine_and_session(test_engine)


@pytest.fixture
def db_session() -> Iterator[Session]:
    engine = get_engine(f"sqlite:///{_TEST_DB_PATH}")
    TestingSessionLocal = sessionmaker(bind=engine, autoflush=False, autocommit=False, expire_on_commit=False, class_=Session)
    session = TestingSessionLocal()
    try:
        yield session
        session.commit()
    except Exception:
        session.rollback()
        raise
    finally:
        session.close()


@pytest.fixture
def model_repo(db_session: Session) -> ModelRepository:
    return ModelRepository(session_factory=lambda: db_session)


@pytest.fixture
def telemetry_repo(db_session: Session) -> TelemetryRepository:
    return TelemetryRepository(session_factory=lambda: db_session)


@pytest.fixture
def quota_repo(db_session: Session) -> QuotaRepository:
    return QuotaRepository(session_factory=lambda: db_session)


@pytest.fixture
def pin_repo(db_session: Session) -> RoutingPinRepository:
    return RoutingPinRepository(session_factory=lambda: db_session)


@pytest.fixture
def tier_repo(db_session: Session) -> TierPolicyRepository:
    return TierPolicyRepository(session_factory=lambda: db_session)
