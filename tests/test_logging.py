from __future__ import annotations

import logging
from pathlib import Path

import pytest

from lib.core.logs import (
    CliFilter,
    LogTarget,
    StructuredFormatter,
    _resolve_level,
    _should_use_color,
    configure_logging,
    get_logger,
)


def test_structured_formatter_no_color():
    formatter = StructuredFormatter(use_color=False)
    record = logging.LogRecord(
        name="test_logger",
        level=logging.INFO,
        pathname=__file__,
        lineno=42,
        msg="Hello %s",
        args=("world",),
        exc_info=None,
    )
    record.funcName = "test_func"
    formatted = formatter.format(record)
    assert "INFO" in formatted
    assert "test_logger" in formatted
    assert "test_func:42" in formatted
    assert "Hello world" in formatted
    assert "\033[" not in formatted


def test_structured_formatter_color():
    formatter = StructuredFormatter(use_color=True)
    record = logging.LogRecord(
        name="test_logger",
        level=logging.ERROR,
        pathname=__file__,
        lineno=10,
        msg="Critical failure",
        args=(),
        exc_info=None,
    )
    record.funcName = "error_func"
    formatted = formatter.format(record)
    assert "ERROR" in formatted
    assert "\033[" in formatted


def test_cli_filter():
    f = CliFilter(verbose=False)
    info_record = logging.LogRecord("test", logging.INFO, "test.py", 1, "msg", (), None)
    warn_record = logging.LogRecord("test", logging.WARNING, "test.py", 1, "msg", (), None)
    error_record = logging.LogRecord("test", logging.ERROR, "test.py", 1, "msg", (), None)

    assert not f.filter(info_record)
    assert f.filter(warn_record)
    assert f.filter(error_record)

    f_verbose = CliFilter(verbose=True)
    assert f_verbose.filter(info_record)
    assert f_verbose.filter(warn_record)


def test_configure_logging_with_file(tmp_path: Path):
    log_file = tmp_path / "test_run.log"
    configure_logging(debug=True, verbose=True, target=LogTarget.CLI, log_file=log_file)
    logger = get_logger("cortex.test")
    logger.info("Test message for file logging")

    assert log_file.exists()
    content = log_file.read_text(encoding="utf-8")
    assert "Test message for file logging" in content
