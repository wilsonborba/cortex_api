from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException

from lib.engine.attachments import Attachment
from lib.engine.executor import Executor
from lib.engine.format import ENCODERS
from lib.engine.router import Router, RoutingRequest
from lib.presentation.api.deps import get_executor, get_router
from lib.presentation.api.schemas.execute import ExecuteRequest, ExecuteResponse

router = APIRouter(tags=["execute"])


@router.post("/execute", response_model=ExecuteResponse)
async def execute(
    payload: ExecuteRequest,
    router_: Router = Depends(get_router),
    executor: Executor = Depends(get_executor),
) -> ExecuteResponse:
    if payload.force_context_format is not None and payload.force_context_format not in ENCODERS:
        raise HTTPException(
            status_code=422,
            detail=f"unknown context format {payload.force_context_format!r}; registered: {sorted(ENCODERS)}",
        )
    routing_request = RoutingRequest(
        prompt=payload.prompt,
        tier=payload.tier,
        task_type=payload.task_type,
        needs_web=payload.needs_web,
        use_memory=payload.use_memory,
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
    result = await executor.execute(plan)
    return ExecuteResponse.from_result(result)
