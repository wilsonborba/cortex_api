from __future__ import annotations

from lib.engine.video_ingest import VideoIngestResult
from lib.engine.video_jobs import VideoJobStore, run_video_job


class _FakeIngestor:
    def __init__(self, result=None, raises: Exception | None = None) -> None:
        self._result = result
        self._raises = raises

    def process(self, video_bytes: bytes, filename: str) -> VideoIngestResult:
        if self._raises:
            raise self._raises
        return self._result


def test_job_starts_processing_and_get_returns_none_for_unknown_id():
    store = VideoJobStore()
    job = store.create(filename="clip.mp4")

    assert job.status == "processing"
    assert store.get(job.id) is job
    assert store.get("does-not-exist") is None


def test_run_video_job_marks_done_on_success():
    store = VideoJobStore()
    job = store.create(filename="clip.mp4")
    result = VideoIngestResult(transcript="hi", summary="a summary")

    run_video_job(store, job.id, b"bytes", "clip.mp4", _FakeIngestor(result=result))

    updated = store.get(job.id)
    assert updated.status == "done"
    assert updated.result.summary == "a summary"


def test_run_video_job_marks_error_when_ingestor_result_has_errors():
    store = VideoJobStore()
    job = store.create(filename="clip.mp4")
    result = VideoIngestResult(errors=["nothing extracted"])

    run_video_job(store, job.id, b"bytes", "clip.mp4", _FakeIngestor(result=result))

    updated = store.get(job.id)
    assert updated.status == "error"


def test_run_video_job_marks_error_on_exception():
    store = VideoJobStore()
    job = store.create(filename="clip.mp4")

    run_video_job(store, job.id, b"bytes", "clip.mp4", _FakeIngestor(raises=RuntimeError("boom")))

    updated = store.get(job.id)
    assert updated.status == "error"
    assert "boom" in updated.error
