from __future__ import annotations

import base64

import pytest
from fastapi.testclient import TestClient

from lib.core.settings import get_settings
from lib.engine.executor import ExecutionResult, StepResult
from lib.engine.router import ExecutionPlan, ModelSelection, RoutingRequest
from lib.engine.video_ingest import VideoIngestResult
from lib.presentation.api.app import create_app
from lib.presentation.api.deps import get_executor, get_router, get_video_ingestor


class _FakeIngestor:
    def __init__(self, result: VideoIngestResult) -> None:
        self._result = result

    def process(self, video_bytes: bytes, filename: str) -> VideoIngestResult:
        return self._result


@pytest.fixture
def app_and_client():
    settings = get_settings().model_copy(
        update={"api_sync_models_on_startup": False, "api_background_tasks_enabled": False}
    )
    app = create_app(settings=settings)
    with TestClient(app) as client:
        yield app, client


def _attachment_payload() -> dict:
    return {"filename": "clip.mp4", "mime_type": "video/mp4", "data_base64": base64.b64encode(b"fake").decode()}


def test_submit_video_returns_processing_status_immediately(app_and_client):
    app, client = app_and_client
    app.dependency_overrides[get_video_ingestor] = lambda: _FakeIngestor(
        VideoIngestResult(transcript="hi", summary="a summary")
    )

    response = client.post("/attachments/video", json=_attachment_payload())

    assert response.status_code == 200
    body = response.json()
    assert "attachment_id" in body
    assert body["status"] in ("processing", "done")  # TestClient runs the background task inline


def test_video_job_status_reflects_completed_summary(app_and_client):
    app, client = app_and_client
    app.dependency_overrides[get_video_ingestor] = lambda: _FakeIngestor(
        VideoIngestResult(transcript="hi", summary="a summary")
    )

    attachment_id = client.post("/attachments/video", json=_attachment_payload()).json()["attachment_id"]
    status = client.get(f"/attachments/video/{attachment_id}").json()

    assert status["status"] == "done"
    assert status["summary"] == "a summary"


def test_video_job_status_reports_errors_when_ingestion_fails(app_and_client):
    app, client = app_and_client
    app.dependency_overrides[get_video_ingestor] = lambda: _FakeIngestor(
        VideoIngestResult(errors=["no audio, no frames"])
    )

    attachment_id = client.post("/attachments/video", json=_attachment_payload()).json()["attachment_id"]
    status = client.get(f"/attachments/video/{attachment_id}").json()

    assert status["status"] == "error"
    assert "no audio, no frames" in status["errors"]


def test_unknown_video_job_id_is_404(app_and_client):
    _, client = app_and_client
    response = client.get("/attachments/video/does-not-exist")
    assert response.status_code == 404


class _FakeRouter:
    def __init__(self, plan):
        self._plan = plan
        self.last_request = None

    def build_execution_plan(self, request: RoutingRequest) -> ExecutionPlan:
        self.last_request = request
        return self._plan


class _FakeExecutor:
    def __init__(self, result: ExecutionResult) -> None:
        self._result = result

    async def execute(self, plan: ExecutionPlan) -> ExecutionResult:
        return self._result


def _plan() -> ExecutionPlan:
    return ExecutionPlan(
        tier=1, task_type="general", strategy_id="general_t1_dynamic", prompt="hi",
        original_prompt="hi",
        tenant_id="default",
        conversation_id=None,
        selections=[ModelSelection(model_id="ollama/model", provider="ollama", role="primary")],
        allow_multi_model=False, retrieval_mode="none", needs_web=False, use_memory=False, memory_topic=None,
        require_verification=False, max_latency_seconds=10, max_model_calls=1, source="dynamic", reason="test",
    )


def _result() -> ExecutionResult:
    return ExecutionResult(
        request_id="req-1", tier_requested=1, tier_executed=1, strategy_id="general_t1_dynamic",
        task_type="general", success=True, response_text="ok",
        steps=[StepResult(
            role="primary", provider="ollama", model_id="ollama/model", success=True, response_text="ok",
            input_tokens=1, output_tokens=1, latency_ms=1, cost_usd=0.0, error_type=None, error_message=None,
            attempts=1,
        )],
        total_input_tokens=1, total_output_tokens=1, total_cost_usd=0.0, latency_ms=1, error_type=None,
    )


def test_execute_with_attachment_job_id_injects_video_summary_into_prompt(app_and_client):
    app, client = app_and_client
    app.dependency_overrides[get_video_ingestor] = lambda: _FakeIngestor(
        VideoIngestResult(summary="a cat plays with a red ball")
    )
    router = _FakeRouter(plan=_plan())
    app.dependency_overrides[get_router] = lambda: router
    app.dependency_overrides[get_executor] = lambda: _FakeExecutor(result=_result())

    attachment_id = client.post("/attachments/video", json=_attachment_payload()).json()["attachment_id"]

    response = client.post("/execute", json={"prompt": "what happens?", "attachment_job_id": attachment_id})

    assert response.status_code == 200
    assert "a cat plays with a red ball" in router.last_request.prompt
    assert "what happens?" in router.last_request.prompt


def test_execute_with_unfinished_attachment_job_id_returns_409(app_and_client):
    app, client = app_and_client

    class _NeverFinishesIngestor:
        def process(self, video_bytes, filename):
            raise RuntimeError("should not run in this test")

    # Directly create a "processing" job via the store, bypassing submit_video.
    from lib.presentation.api.deps import get_video_job_store

    store = get_video_job_store()
    job = store.create(filename="clip.mp4")

    response = client.post("/execute", json={"prompt": "hi", "attachment_job_id": job.id})

    assert response.status_code == 409


def test_execute_with_unknown_attachment_job_id_returns_404(app_and_client):
    _, client = app_and_client
    response = client.post("/execute", json={"prompt": "hi", "attachment_job_id": "does-not-exist"})
    assert response.status_code == 404
