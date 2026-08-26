from __future__ import annotations

import asyncio
import json
import time
import uuid
from typing import Any, AsyncIterator, Dict, List, Optional, Tuple

from fastapi import APIRouter, Depends
from fastapi.responses import JSONResponse, StreamingResponse

from lib.engine.executor import Executor, ExecutionResult, UnresolvedStrategyError
from lib.engine.prompt_normalizer import PromptNormalizer
from lib.engine.router import NoEligibleModelError, Router, RoutingRequest
from lib.core.security import SecurityShield
from lib.presentation.api.deps import get_executor, get_prompt_normalizer, get_router, get_security_shield
from lib.presentation.api.schemas.openai_facade import (
    ChatCompletionChoice,
    ChatCompletionRequest,
    ChatCompletionResponse,
    ChatCompletionUsage,
    ChatMessage,
    ModelCard,
    ModelList,
)

# Isolated under /v1, alongside (not instead of) the native routes, per
# docs/openai-compatible-api.md's architectural split.
router = APIRouter(prefix="/v1", tags=["openai-facade"])

VIRTUAL_MODELS = ["cortex-auto", "cortex-t0", "cortex-t1", "cortex-t2", "cortex-t3", "cortex-t4", "cortex-t5"]
_STREAM_WORDS_PER_CHUNK = 5


@router.get("/models", response_model=ModelList)
def list_virtual_models() -> ModelList:
    now = int(time.time())
    return ModelList(data=[ModelCard(id=model_id, created=now) for model_id in VIRTUAL_MODELS])


@router.post("/chat/completions")
async def chat_completions(
    payload: ChatCompletionRequest,
    router_: Router = Depends(get_router),
    executor: Executor = Depends(get_executor),
    prompt_normalizer: PromptNormalizer = Depends(get_prompt_normalizer),
    security_shield: SecurityShield = Depends(get_security_shield),
):
    tier, force_strategy = _translate_model(payload.model)
    prompt = _messages_to_prompt(payload.messages)
    security_eval = await asyncio.to_thread(security_shield.evaluate, prompt)
    if security_eval.is_blocked:
        message = "Are you kidding me, clown?" if security_eval.error_type == "security_policy_violation" else (security_eval.reason or "Blocked by Security Shield")
        return JSONResponse(status_code=403, content=_openai_error(message, security_eval.error_type or "security_policy_violation"))
    if payload.normalize_prompt:
        normalization = await asyncio.to_thread(prompt_normalizer.normalize, prompt)
        if not normalization.success:
            return JSONResponse(status_code=503, content=_openai_error("Local prompt normalizer is unavailable.", normalization.error_type or "normalizer_failed"))
        prompt = normalization.prompt
    routing_request = RoutingRequest(
        prompt=prompt,
        tier=tier,
        force_strategy=force_strategy,
        thinking=payload.thinking,
        needs_web=payload.needs_web,
        use_memory=payload.use_memory,
        auto_retrieval=payload.auto_retrieval,
        memory_topic=payload.memory_topic,
    )

    try:
        plan = router_.build_execution_plan(routing_request)
        result = await executor.execute(plan)
    except NoEligibleModelError as exc:
        return JSONResponse(status_code=409, content=_openai_error(str(exc), "no_eligible_model"))
    except UnresolvedStrategyError as exc:
        return JSONResponse(status_code=501, content=_openai_error(str(exc), "unresolved_strategy"))

    if payload.stream:
        return StreamingResponse(_stream_response(payload.model, result), media_type="text/event-stream")

    return ChatCompletionResponse(
        id=_new_completion_id(),
        created=int(time.time()),
        model=payload.model,
        choices=[
            ChatCompletionChoice(
                message=ChatMessage(role="assistant", content=result.response_text),
                finish_reason="stop" if result.success else "error",
            )
        ],
        usage=ChatCompletionUsage(
            prompt_tokens=result.total_input_tokens,
            completion_tokens=result.total_output_tokens,
            total_tokens=result.total_input_tokens + result.total_output_tokens,
        ),
    )


# -- model name translation ---------------------------------------------------------


def _translate_model(model: str) -> Tuple[Optional[str], Optional[str]]:
    """Virtual model name -> (tier, force_strategy), per the mapping table
    in docs/openai-compatible-api.md section 4."""
    if model == "cortex-auto":
        return "auto", None
    if model.startswith("cortex-pin:"):
        return "auto", model.split(":", 1)[1]
    tier_suffix = model.removeprefix("cortex-t")
    if tier_suffix.isdigit():
        return tier_suffix, None
    # Unrecognized model string: most OpenAI-compatible clients let a user
    # type any model name, so fall back to auto-classification rather than
    # rejecting the call outright.
    return "auto", None


def _messages_to_prompt(messages: List[ChatMessage]) -> str:
    """Flattens the whole chat history into one transcript.

    Cortex's execution drivers (#5) take a single prompt string -- there's
    no per-provider system-prompt channel yet -- so multi-turn history and
    any system message are folded in as "Role: content" lines rather than
    silently dropped.
    """
    return "\n\n".join(f"{m.role.capitalize()}: {m.content}" for m in messages)


def _new_completion_id() -> str:
    return f"chatcmpl-{uuid.uuid4().hex[:24]}"


def _openai_error(message: str, error_type: str) -> Dict[str, Any]:
    return {"error": {"message": message, "type": error_type, "param": None, "code": None}}


# -- streaming ---------------------------------------------------------------------


async def _stream_response(model: str, result: ExecutionResult) -> AsyncIterator[bytes]:
    """Server-Sent Events in the `chat.completion.chunk` wire format.

    This is simulated streaming: none of Cortex's execution drivers (#5)
    expose token-level streaming from the underlying provider, so the full
    response is already in hand by the time this generator starts. It's
    chunked into a handful of SSE frames -- rather than sent as one giant
    frame -- purely so streaming-aware clients still render incrementally;
    it is not token-by-token generation streaming.
    """
    completion_id = _new_completion_id()
    created = int(time.time())

    def frame(delta: Dict[str, Any], finish_reason: Optional[str] = None) -> bytes:
        chunk = {
            "id": completion_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}],
        }
        return f"data: {json.dumps(chunk)}\n\n".encode("utf-8")

    yield frame({"role": "assistant"})

    words = result.response_text.split(" ") if result.response_text else []
    for i in range(0, len(words), _STREAM_WORDS_PER_CHUNK):
        piece = " ".join(words[i : i + _STREAM_WORDS_PER_CHUNK])
        if i + _STREAM_WORDS_PER_CHUNK < len(words):
            piece += " "
        yield frame({"content": piece})

    yield frame({}, finish_reason="stop" if result.success else "error")
    yield b"data: [DONE]\n\n"
