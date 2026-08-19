from __future__ import annotations

import base64

from fastapi import APIRouter, BackgroundTasks, Depends, HTTPException

from lib.engine.video_ingest import VideoIngestor
from lib.engine.video_jobs import VideoJobStore, run_video_job
from lib.presentation.api.deps import get_video_ingestor, get_video_job_store
from lib.presentation.api.schemas.attachments import Attachment
from lib.presentation.api.schemas.video import VideoJobCreated, VideoJobStatus

router = APIRouter(tags=["video"])


@router.post("/attachments/video", response_model=VideoJobCreated)
async def submit_video(
    payload: Attachment,
    background_tasks: BackgroundTasks,
    store: VideoJobStore = Depends(get_video_job_store),
    ingestor: VideoIngestor = Depends(get_video_ingestor),
) -> VideoJobCreated:
    try:
        video_bytes = base64.b64decode(payload.data_base64)
    except Exception as exc:
        raise HTTPException(status_code=422, detail=f"invalid base64: {exc}") from exc

    job = store.create(filename=payload.filename)
    background_tasks.add_task(run_video_job, store, job.id, video_bytes, payload.filename, ingestor)
    return VideoJobCreated(attachment_id=job.id, status=job.status)


@router.get("/attachments/video/{attachment_id}", response_model=VideoJobStatus)
async def get_video_job(attachment_id: str, store: VideoJobStore = Depends(get_video_job_store)) -> VideoJobStatus:
    job = store.get(attachment_id)
    if job is None:
        raise HTTPException(status_code=404, detail=f"no video job with id {attachment_id!r}")

    result = job.result
    return VideoJobStatus(
        attachment_id=job.id,
        status=job.status,
        summary=result.summary if result else None,
        transcript=result.transcript if result else None,
        errors=(result.errors if result else []) or ([job.error] if job.error else []),
    )
