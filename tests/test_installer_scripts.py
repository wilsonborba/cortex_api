from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

SCRIPTS_DIR = Path(__file__).resolve().parent.parent / "scripts"


def test_install_script_help():
    script = SCRIPTS_DIR / "install.sh"
    assert script.exists()
    result = subprocess.run(
        [str(script), "--help"],
        capture_output=True,
        text=True,
        check=True,
    )
    assert "Usage:" in result.stdout
    assert "--profile" in result.stdout
    assert "light" in result.stdout
    assert "medium" in result.stdout
    assert "complete" in result.stdout


def test_service_script_help():
    script = SCRIPTS_DIR / "service.sh"
    assert script.exists()
    result = subprocess.run(
        [str(script), "help"],
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0 or "Usage:" in result.stdout or "Usage:" in result.stderr
    assert "start" in result.stdout or "start" in result.stderr


def test_uninstall_script_help():
    script = SCRIPTS_DIR / "uninstall.sh"
    assert script.exists()
    result = subprocess.run(
        [str(script), "--help"],
        capture_output=True,
        text=True,
        check=True,
    )
    assert "Usage:" in result.stdout
    assert "--purge-data" in result.stdout
