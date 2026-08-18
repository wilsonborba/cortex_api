from __future__ import annotations

from fastapi import APIRouter, Depends

from lib.engine.executor import Executor
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
    )
    # NoEligibleModelError / UnresolvedStrategyError propagate to the
    # app-level exception handlers registered in lib.presentation.api.app.
    plan = router_.build_execution_plan(routing_request)
    result = await executor.execute(plan)
    return ExecuteResponse.from_result(result)
