from __future__ import annotations

from datetime import datetime, timezone


def ensure_utc(value: datetime) -> datetime:
    """Normalizes a datetime to be UTC-aware.

    SQLite has no native timezone-aware column type: SQLAlchemy writes tz-aware
    values fine, but reads them back naive. Anything read from the DB needs
    this before it's compared against a fresh `datetime.now(timezone.utc)`.
    """
    if value.tzinfo is None:
        return value.replace(tzinfo=timezone.utc)
    return value
