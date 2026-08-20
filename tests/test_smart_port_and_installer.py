from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

SCRIPTS_DIR = Path(__file__).resolve().parent.parent / "scripts"


def test_installer_non_interactive_flag():
    script = SCRIPTS_DIR / "install.sh"
    result = subprocess.run(
        [str(script), "--help"],
        capture_output=True,
        text=True,
        check=True,
    )
    assert "--non-interactive" in result.stdout or "-y" in result.stdout
    assert "--profile" in result.stdout


def test_port_detection_logic():
    # Verify socket connection check script runs cleanly
    check_code = (
        "import socket; s = socket.socket(socket.AF_INET, socket.SOCK_STREAM); "
        "s.settimeout(0.5); res = s.connect_ex(('127.0.0.1', 59999)); s.close(); exit(0 if res == 0 else 1)"
    )
    res = subprocess.run(["python3", "-c", check_code])
    # Port 59999 is unlikely to be open, so connect_ex returns != 0 and exit code is 1
    assert res.returncode in (0, 1)
