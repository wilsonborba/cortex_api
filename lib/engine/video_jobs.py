"""In-process video-job registry.

Deliberately process-local (a plain dict, not a DB table): video ingestion
is heavy enough to run as a background job, but light enough on volume that
a durable job table would be scope well beyond what was asked here. A
restart loses in-flight/finished jobs -- acceptable for a single-process
dev service; revisit with a real table if this needs to survive restarts
or run across multiple workers.
"""

from __future__ import annotations

import threading
import uuid
from dataclasses import dataclass, field
from typing import Dict, Optional

from lib.core.logs import get_logger
from lib.engine.video_ingest import VideoIngestResult

logger = get_logger(__name__)


@dataclass
class VideoJob:
    id: str
    filename: str
    status: str = "processing"  # "processing" | "done" | "error"
    result: Optional[VideoIngestResult] = None
    error: Optional[str] = None


class VideoJobStore:
    def __init__(self) -> None:
        self._jobs: Dict[str, VideoJob] = {}
        self._lock = threading.Lock()

    def create(self, filename: str) -> VideoJob:
        job = VideoJob(id=str(uuid.uuid4()), filename=filename)
        with self._lock:
            self._jobs[job.id] = job
        return job

    def get(self, job_id: str) -> Optional[VideoJob]:
        with self._lock:
            return self._jobs.get(job_id)

    def mark_done(self, job_id: str, result: VideoIngestResult) -> None:
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None:
                return
            job.result = result
            job.status = "error" if result.errors and not result.ok else "done"

    def mark_error(self, job_id: str, error: str) -> None:
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None:
                return
            job.status = "error"
            job.error = error


_default_store = VideoJobStore()


def get_default_video_job_store() -> VideoJobStore:
    return _default_store


def run_video_job(store: VideoJobStore, job_id: str, video_bytes: bytes, filename: str, ingestor) -> None:
    """Synchronous worker body -- run via `BackgroundTasks` (a thread), not `asyncio.create_task`."""
    try:
        result = ingestor.process(video_bytes, filename)
        store.mark_done(job_id, result)
    except Exception as exc:
        logger.exception("video job %s failed", job_id)
        store.mark_error(job_id, str(exc))
