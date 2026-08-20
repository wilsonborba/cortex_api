from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

SCRIPTS_DIR = Path(__file__).resolve().parent.parent / "scripts"
DETECT_SCRIPT = SCRIPTS_DIR / "detect_hardware.sh"


def test_detect_hardware_script_exists_and_executable():
    assert DETECT_SCRIPT.exists()
    assert DETECT_SCRIPT.is_file()


def test_detect_hardware_json_output():
    result = subprocess.run(
        [str(DETECT_SCRIPT), "--json"],
        capture_output=True,
        text=True,
        check=True,
    )
    data = json.loads(result.stdout)
    assert "os" in data
    assert "type" in data["os"]
    assert "cpu" in data
    assert data["cpu"]["cores"] >= 1
    assert "ram" in data
    assert data["ram"]["total_mb"] > 0
    assert "gpu" in data
    assert data["recommended_profile"] in ("light", "medium", "complete")


def test_detect_hardware_profile_only():
    result = subprocess.run(
        [str(DETECT_SCRIPT), "--profile-only"],
        capture_output=True,
        text=True,
        check=True,
    )
    profile = result.stdout.strip()
    assert profile in ("light", "medium", "complete")
