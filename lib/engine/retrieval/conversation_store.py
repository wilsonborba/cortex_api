from __future__ import annotations

import asyncio
from datetime import datetime, timezone
import json
from typing import Any, Dict, List, Optional
import httpx

from lib.core.logs import get_logger
from lib.engine.retrieval.hippocampus import HippocampusClient

logger = get_logger(__name__)


async def record_conversation_turn(
    hippocampus: HippocampusClient,
    conversation_id: str,
    user_prompt: str,
    assistant_response: str,
    tenant_id: str = "default",
    title: Optional[str] = None,
) -> bool:
    """Stores a completed conversation turn to Hippocampus under tags
    `conversation:{id}` and `tenant:{tenant_id}`, and (this is the part
    that actually gets enforced at the repository level, unlike tags,
    which Hippocampus never even echoes back on read) `workspace_id`, so
    `MemoryRepository.get`/`list`'s real workspace filtering applies to
    every turn this backend ever writes."""
    turn_data = {
        "user": user_prompt,
        "assistant": assistant_response,
        "timestamp": datetime.now(timezone.utc).isoformat(),
    }
    content = f"User: {user_prompt}\n\nAssistant: {assistant_response}"
    tags = [f"conversation:{conversation_id}", f"tenant:{tenant_id}", "type:conversation_turn"]
    body = {
        "content": content,
        "title": title or f"Turn in {conversation_id}",
        "tags": tags,
        "workspace_id": tenant_id,
        "metadata": {
            "conversation_id": conversation_id,
            "tenant_id": tenant_id,
            "turn": turn_data,
        },
    }
    try:
        async with hippocampus._client_factory() as client:
            resp = await client.post(
                f"{hippocampus._base_url}/api/v1/memories",
                json=body,
                headers=hippocampus._headers(),
            )
            resp.raise_for_status()
            return True
    except Exception as exc:
        logger.warning("Failed to record conversation turn to Hippocampus: %s", exc)
        return False
