from __future__ import annotations

import asyncio

from fastapi import APIRouter, Depends, HTTPException

from lib.engine.attachments import Attachment
from lib.engine.executor import Executor
from lib.engine.format import ENCODERS
from lib.engine.prompt_normalizer import PromptNormalizer
from lib.engine.router import Router, RoutingRequest
from lib.presentation.api.deps import get_executor, get_prompt_normalizer, get_router, get_security_shield, get_video_job_store
from lib.presentation.api.schemas.execute import ExecuteRequest, ExecuteResponse
from lib.engine.video_jobs import VideoJobStore

from lib.core.security import SecurityShield

router = APIRouter(tags=["execute"])


@router.post("/execute", response_model=ExecuteResponse)
async def execute(
    payload: ExecuteRequest,
    router_: Router = Depends(get_router),
    executor: Executor = Depends(get_executor),
    prompt_normalizer: PromptNormalizer = Depends(get_prompt_normalizer),
    security_shield: SecurityShield = Depends(get_security_shield),
    video_jobs: VideoJobStore = Depends(get_video_job_store),
) -> ExecuteResponse:
    if not payload.skip_security:
        security_eval = await asyncio.to_thread(security_shield.evaluate, payload.prompt)
        if security_eval.is_blocked:
            response_text = "Are you kidding me, clown?" if security_eval.error_type == "security_policy_violation" else (security_eval.reason or "Blocked by Security Shield")
            return ExecuteResponse(
                request_id="sec-blocked",
                tier_requested=0,
                tier_executed=0,
                strategy_id="security_shield",
                task_type="security",
                success=False,
                response=response_text,
                input_tokens=0,
                output_tokens=0,
                total_tokens=0,
                cost_usd=0.0,
                latency_ms=0,
                error_type=security_eval.error_type,
                steps=[],
            )

    # PARTE 1: Proxy Direto (Passthrough) para Memórias e Tarefas (Sem Travamento por IA)
    if payload.capabilities.memory or payload.capabilities.tasks:
        import urllib.request, json
        if payload.capabilities.memory:
            try:
                req_data = json.dumps({"query": payload.prompt, "tags": [f"tenant:{payload.tenant_id}"], "limit": 5}).encode("utf-8")
                req = urllib.request.Request("http://127.0.0.1:8001/api/v1/recall", data=req_data, headers={"Content-Type": "application/json"}, method="POST")
                with urllib.request.urlopen(req, timeout=3.0) as resp:
                    mem_data = json.loads(resp.read().decode("utf-8"))
                    return ExecuteResponse(
                        request_id="proxy-memory",
                        tier_requested=payload.tier or 0,
                        tier_executed=0,
                        strategy_id="hippocampus_proxy_passthrough",
                        task_type="memory_recall",
                        success=True,
                        response=f"[Proxy Passthrough] Memórias recuperadas do Hippocampus: {json.dumps(mem_data)[:300]}",
                        input_tokens=0,
                        output_tokens=0,
                        total_tokens=0,
                        cost_usd=0.0,
                        latency_ms=15,
                        steps=[],
                    )
            except Exception:
                pass
        if payload.capabilities.tasks:
            try:
                req = urllib.request.Request("http://127.0.0.1:8011/api/v1/workspaces/default/projects/00000000-0000-0000-0000-000000000001/issues/", headers={"X-Api-Key": "cortex-test-key"}, method="GET")
                with urllib.request.urlopen(req, timeout=3.0) as resp:
                    task_data = json.loads(resp.read().decode("utf-8"))
                    return ExecuteResponse(
                        request_id="proxy-tasks",
                        tier_requested=payload.tier or 0,
                        tier_executed=0,
                        strategy_id="plane_slim_proxy_passthrough",
                        task_type="task_management",
                        success=True,
                        response=f"[Proxy Passthrough] Tarefas recuperadas do plane-slim: {json.dumps(task_data)[:300]}",
                        input_tokens=0,
                        output_tokens=0,
                        total_tokens=0,
                        cost_usd=0.0,
                        latency_ms=15,
                        steps=[],
                    )
            except Exception:
                pass

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
        if not normalization.success:
            return ExecuteResponse(
                request_id="normalizer-failed", tier_requested=payload.tier or 0, tier_executed=0,
                strategy_id="prompt_normalizer", task_type=payload.task_type, success=False,
                response="Local prompt normalizer is unavailable.", input_tokens=0, output_tokens=0,
                total_tokens=0, cost_usd=0.0, latency_ms=0, error_type=normalization.error_type, steps=[],
            )
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
