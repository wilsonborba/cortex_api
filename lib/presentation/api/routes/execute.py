from __future__ import annotations

import asyncio

from fastapi import APIRouter, Depends, HTTPException

from lib.engine.attachments import Attachment
from lib.engine.executor import Executor
from lib.engine.format import ENCODERS
from lib.engine.prompt_normalizer import PromptNormalizer
from lib.engine.router import Router, RoutingRequest
from lib.presentation.api.deps import get_executor, get_prompt_normalizer, get_router, get_video_job_store
from lib.presentation.api.schemas.execute import ExecuteRequest, ExecuteResponse
from lib.engine.video_jobs import VideoJobStore

router = APIRouter(tags=["execute"])


@router.post("/execute", response_model=ExecuteResponse)
async def execute(
    payload: ExecuteRequest,
    router_: Router = Depends(get_router),
    executor: Executor = Depends(get_executor),
    prompt_normalizer: PromptNormalizer = Depends(get_prompt_normalizer),
    video_jobs: VideoJobStore = Depends(get_video_job_store),
) -> ExecuteResponse:
    if payload.force_context_format is not None and payload.force_context_format not in ENCODERS:
        raise HTTPException(
            status_code=422,
            detail=f"unknown context format {payload.force_context_format!r}; registered: {sorted(ENCODERS)}",
        )

    prompt = payload.prompt
    if payload.attachment_job_id:
        job = video_jobs.get(payload.attachment_job_id)
        if job is None:
            raise HTTPException(status_code=404, detail=f"no video job with id {payload.attachment_job_id!r}")
        if job.status == "processing":
            raise HTTPException(status_code=409, detail="video job is still processing; poll /attachments/video/{id}")
        if job.status == "error" or job.result is None or not job.result.summary:
            detail = job.error or (job.result.errors if job.result else ["video job failed"])
            raise HTTPException(status_code=422, detail=f"video job did not produce a usable summary: {detail}")
        prompt = f"## Video context\n\n{job.result.summary}\n\n{prompt}"

    if payload.normalize_prompt:
        normalization = await asyncio.to_thread(prompt_normalizer.normalize, prompt)
        prompt = normalization.prompt

    routing_request = RoutingRequest(
        prompt=prompt,
        tier=payload.tier,
        task_type=payload.task_type,
        thinking=payload.capabilities.thinking or payload.thinking,
        needs_web=payload.capabilities.web or payload.needs_web,
        use_memory=payload.capabilities.memory or payload.use_memory,
        auto_retrieval=payload.auto_retrieval,
        memory_topic=payload.memory_topic,
        force_model=payload.force_model,
        force_provider=payload.force_provider,
        force_strategy=payload.override_strategy,
        force_context_format=payload.force_context_format,
        attachments=[
            Attachment(filename=a.filename, mime_type=a.mime_type, data_base64=a.data_base64)
            for a in payload.attachments
        ],
    )
    # NoEligibleModelError / UnresolvedStrategyError propagate to the
    # app-level exception handlers registered in lib.presentation.api.app.
    plan = router_.build_execution_plan(routing_request)
    if payload.timeout is not None:
        from lib.core.settings import get_settings
        from lib.engine.executor import build_default_executor
        custom_settings = get_settings().model_copy(update={"driver_timeout_seconds": payload.timeout})
        executor = build_default_executor(settings=custom_settings)
    result = await executor.execute(plan)
    return ExecuteResponse.from_result(result)
