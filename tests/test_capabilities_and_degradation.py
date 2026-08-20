from __future__ import annotations

import base64
import pytest
from fastapi.testclient import TestClient

from lib.core.capabilities import detect_runtime_capabilities
from lib.core.settings import Settings, get_settings
from lib.engine.attachments import Attachment, AttachmentIngestor
from lib.engine.video_ingest import VideoIngestor
from lib.presentation.api.app import create_app


def _test_settings(profile="light"):
    return get_settings().model_copy(
        update={
            "profile": profile,
            "api_sync_models_on_startup": False,
            "api_background_tasks_enabled": False,
            "whisper_local_enabled": False,
            "groq_api_key": None,
        }
    )


@pytest.fixture
def light_client():
    app = create_app(settings=_test_settings(profile="light"))
    with TestClient(app) as client_:
        yield client_


def test_capabilities_detection():
    settings = _test_settings(profile="light")
    caps = detect_runtime_capabilities(settings)
    assert caps.profile == "light"
    assert caps.core_orchestration is True
    assert caps.openai_facade is True
    data = caps.to_dict()
    assert "profile" in data
    assert "capabilities" in data
    assert data["capabilities"]["core_orchestration"] is True


def test_system_capabilities_endpoint(light_client: TestClient):
    response = light_client.get("/system/capabilities")
    assert response.status_code == 200
    data = response.json()
    assert data["profile"] == "light"
    assert "capabilities" in data
    assert "missing_optional_components" in data


def test_audio_transcription_graceful_degradation():
    settings = _test_settings(profile="light")
    ingestor = AttachmentIngestor(settings=settings)
    fake_audio = base64.b64encode(b"RIFF....WAVEfmt ....data....").decode()
    attachment = Attachment(filename="sample.wav", mime_type="audio/wav", data_base64=fake_audio)

    result = ingestor.ingest([attachment])
    assert not result.ok
    assert len(result.errors) == 1
    assert "audio transcription is unavailable under profile 'light'" in result.errors[0]
    assert "CORTEX_GROQ_API_KEY" in result.errors[0]


def test_video_processing_graceful_degradation(monkeypatch):
    settings = _test_settings(profile="light")
    ingestor = VideoIngestor(settings=settings)
    fake_video = b"\x00\x00\x00 ftypmp42\x00\x00\x00\x00"

    # Simulate ffmpeg failure
    monkeypatch.setattr("subprocess.run", lambda *args, **kwargs: type("Proc", (), {"returncode": 1, "stderr": "ffmpeg: not found"})())
    result = ingestor.process(fake_video, "demo.mp4")
    assert not result.ok
    assert any("extraction failed" in err for err in result.errors)
