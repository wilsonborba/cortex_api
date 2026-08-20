from __future__ import annotations

from pathlib import Path

from alembic.command import downgrade as alembic_downgrade
from alembic.command import upgrade as alembic_upgrade
from alembic.config import Config

from lib.core.settings import get_settings
from lib.dal.local.database import _prepare_sqlite_path

ALEMBIC_INI_PATH = Path(__file__).resolve().parent / "alembic.ini"


def get_alembic_config(database_url: str | None = None) -> Config:
    config = Config(str(ALEMBIC_INI_PATH))
    url = database_url or get_settings().database_url
    _prepare_sqlite_path(url)
    config.set_main_option("sqlalchemy.url", url)
    return config


def upgrade_db(database_url: str | None = None, revision: str = "head") -> None:
    config = get_alembic_config(database_url)
    alembic_upgrade(config, revision)


def downgrade_db(database_url: str | None = None, revision: str = "base") -> None:
    config = get_alembic_config(database_url)
    alembic_downgrade(config, revision)
