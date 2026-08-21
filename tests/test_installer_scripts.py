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


def test_install_script_contains_explicit_service_environment_contract():
    script = (SCRIPTS_DIR / "install.sh").read_text(encoding="utf-8")
    assert "Environment=HOME=" in script
    assert "Environment=PATH=" in script
    assert "configure_external_cli_paths" in script


def test_install_script_uses_invoking_user_when_run_via_sudo():
    script = (SCRIPTS_DIR / "install.sh").read_text(encoding="utf-8")
    assert "SUDO_USER" in script
    assert "service_account_user" in script


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


def test_install_script_mentions_optional_calibration_flow():
    script = (SCRIPTS_DIR / "install.sh").read_text(encoding="utf-8")
    assert "maybe_run_calibration" in script
    assert "cortex --json calibration judges" in script
    assert "cortex calibration run" in script
    assert "progress bar shows model/task/judge stage" in script
