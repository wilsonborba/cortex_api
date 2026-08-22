from __future__ import annotations

from typing import List, Optional, Union

from pydantic import BaseModel, Field

from lib.engine.executor import ExecutionResult, StepResult
from lib.presentation.api.schemas.attachments import Attachment


class ExecuteRequest(BaseModel):
    prompt: str
    tier: Optional[Union[int, str]] = Field(default=None, description="0-5, 'auto', or omitted for auto")
    task_type: str = "general"
    normalize_prompt: bool = True
    thinking: bool = False
    needs_web: bool = False
    use_memory: bool = False
    auto_retrieval: bool = False
    memory_topic: Optional[str] = None
    force_model: Optional[str] = None
    force_provider: Optional[str] = None
    override_strategy: Optional[str] = None
    force_context_format: Optional[str] = Field(
        default=None, description="'toon' or 'json' -- overrides the model's auto/pinned preference for this call"
    )
    attachments: List[Attachment] = Field(default_factory=list)
    attachment_job_id: Optional[str] = Field(
        default=None, description="id of a finished /attachments/video job (see #23) whose summary to inject as context"
    )
    timeout: Optional[float] = Field(
        default=None, description="Optional driver timeout in seconds for this execution (overrides default)"
    )


class StepOut(BaseModel):
    role: str
    provider: str
    model_id: str
    success: bool
    input_tokens: int
    output_tokens: int
    latency_ms: int
    cost_usd: float
    error_type: Optional[str] = None

    @classmethod
    def from_step(cls, step: StepResult) -> "StepOut":
        return cls(
            role=step.role, provider=step.provider, model_id=step.model_id, success=step.success,
            input_tokens=step.input_tokens, output_tokens=step.output_tokens, latency_ms=step.latency_ms,
            cost_usd=step.cost_usd, error_type=step.error_type,
        )


class ExecuteResponse(BaseModel):
    request_id: str
    tier_requested: int
    tier_executed: int
    strategy_id: str
    task_type: str
    success: bool
    response: str
    input_tokens: int
    output_tokens: int
    total_tokens: int
    cost_usd: float
    latency_ms: int
    error_type: Optional[str] = None
    steps: List[StepOut] = []

    @classmethod
    def from_result(cls, result: ExecutionResult) -> "ExecuteResponse":
        return cls(
            request_id=result.request_id,
            tier_requested=result.tier_requested,
            tier_executed=result.tier_executed,
            strategy_id=result.strategy_id,
            task_type=result.task_type,
            success=result.success,
            response=result.response_text,
            input_tokens=result.total_input_tokens,
            output_tokens=result.total_output_tokens,
            total_tokens=result.total_input_tokens + result.total_output_tokens,
            cost_usd=result.total_cost_usd,
            latency_ms=result.latency_ms,
            error_type=result.error_type,
            steps=[StepOut.from_step(s) for s in result.steps],
        )
