from __future__ import annotations

from typing import List, Optional

from pydantic import BaseModel, Field


class ChatMessage(BaseModel):
    role: str
    content: str


class ChatCompletionRequest(BaseModel):
    model: str
    messages: List[ChatMessage]
    temperature: Optional[float] = None
    max_tokens: Optional[int] = None
    stream: bool = False
    normalize_prompt: bool = Field(
        default=True,
        description="Rewrite the flattened message prompt with the local text normalizer; false forwards it unchanged after security evaluation.",
    )
    thinking: bool = False
    needs_web: bool = False
    use_memory: bool = False
    auto_retrieval: bool = False
    memory_topic: Optional[str] = None


class ChatCompletionChoice(BaseModel):
    index: int = 0
    message: ChatMessage
    finish_reason: str = "stop"


class ChatCompletionUsage(BaseModel):
    prompt_tokens: int
    completion_tokens: int
    total_tokens: int


class ChatCompletionResponse(BaseModel):
    id: str
    object: str = "chat.completion"
    created: int
    model: str
    choices: List[ChatCompletionChoice]
    usage: ChatCompletionUsage


class ModelCard(BaseModel):
    id: str
    object: str = "model"
    created: int
    owned_by: str = "cortex"


class ModelList(BaseModel):
    object: str = "list"
    data: List[ModelCard]
