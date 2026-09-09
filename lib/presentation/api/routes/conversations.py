from __future__ import annotations

import uuid
from typing import List, Optional
from fastapi import APIRouter, Depends, HTTPException, Query, Request

from lib.core.logs import get_logger
from lib.core.tenant import resolve_tenant_id
from lib.dal.repositories.conversation_repository import ConversationIdConflict, ConversationRepository
from lib.engine.retrieval.hippocampus import HippocampusClient
from lib.presentation.api.deps import get_conversation_repo, get_hippocampus_client
from lib.presentation.api.schemas.conversations import (
    ConversationCreateIn,
    ConversationDetailOut,
    ConversationPatchIn,
    ConversationSummaryOut,
    ConversationTurnOut,
)

router = APIRouter(prefix="/conversations", tags=["conversations"])
logger = get_logger(__name__)


async def _fetch_hippocampus_turns(hippocampus: HippocampusClient, tag: str, workspace_id: str, limit: int) -> list:
    try:
        async with hippocampus._client_factory() as client:
            resp = await client.get(
                f"{hippocampus._base_url}/api/v1/memories",
                params={"tag": tag, "workspace_id": workspace_id, "limit": limit},
                headers=hippocampus._headers(),
            )
            resp.raise_for_status()
            return resp.json().get("data", [])
    except Exception:
        return []


@router.get("", response_model=List[ConversationSummaryOut])
async def list_conversations(
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    hippocampus: HippocampusClient = Depends(get_hippocampus_client),
    conversation_repo: ConversationRepository = Depends(get_conversation_repo),
) -> List[ConversationSummaryOut]:
    """Lists conversations for the authenticated user: metadata (title/pin)
    from the local store, message stats (count/preview/last activity)
    derived from Hippocampus turns tagged `conversation_turn` for them."""
    tenant_id = resolve_tenant_id(request)
    data = await _fetch_hippocampus_turns(hippocampus, f"tenant:{tenant_id}", tenant_id, limit * 4)

    # Group memory turns by conversation_id. Hippocampus never echoes back
    # `metadata`/`tags` on read (see `get_conversation`'s equivalent
    # comment), so the only thing that reliably survives the round-trip is
    # `title`, always written as `f"Turn in {conversation_id}"` by
    # `record_conversation_turn` -- parse it back out of that instead.
    convo_map = {}
    for m in data:
        title_raw = m.get("title") or ""
        convo_id = title_raw[len("Turn in "):] if title_raw.startswith("Turn in ") else None
        if not convo_id:
            continue

        created_at = m.get("created_at") or m.get("observed_at") or ""
        updated_at = m.get("updated_at") or created_at
        content = m.get("content") or ""
        title = f"Conversation {convo_id}"

        if convo_id not in convo_map:
            convo_map[convo_id] = {
                "id": convo_id,
                "title": title,
                "created_at": created_at,
                "updated_at": updated_at,
                "message_count": 0,
                "preview": content[:120],
            }
        convo = convo_map[convo_id]
        convo["message_count"] += 2  # user + assistant pair in each turn
        if updated_at and updated_at > convo["updated_at"]:
            convo["updated_at"] = updated_at

    # Any conversation with turns but no local metadata row yet (legacy
    # data, or a turn recorded before the conversation was ever created
    # through this API) is lazily adopted into the local store so it
    # becomes rename/pin/delete-able going forward. Hippocampus has no
    # delete verb, so its turns for a soft-deleted conversation never go
    # away -- `conversation_repo.get` (unlike `list_active`) returns a
    # soft-deleted row too, so it's checked explicitly here to skip
    # re-adopting one: without this, "delete"/"clear all" would silently
    # resurrect on the very next listing, which used to actually happen.
    local_rows = {c.id: c for c in conversation_repo.list_active(tenant_id=tenant_id)}
    for convo_id, derived in convo_map.items():
        if convo_id in local_rows:
            continue
        existing = conversation_repo.get(convo_id, tenant_id=tenant_id)
        if existing is not None and existing.deleted_at is not None:
            continue  # intentionally deleted; never resurrect it just because Hippocampus still has its turns
        try:
            local_rows[convo_id] = conversation_repo.create(
                conversation_id=convo_id, tenant_id=tenant_id, title=derived["title"]
            )
        except ConversationIdConflict:
            # This id collides with a *different* tenant's real
            # conversation row (only possible if some client sent a
            # colliding conversation_id to /execute or
            # /v1/chat/completions). Skip adopting it locally rather
            # than 500ing the whole listing; the derived stats for it
            # are simply left out of `results` below.
            logger.warning("conversation id %r collides with another tenant, skipping adoption", convo_id)

    results = []
    for convo_id, row in local_rows.items():
        derived = convo_map.get(convo_id)
        results.append(
            ConversationSummaryOut(
                id=convo_id,
                title=row.title,
                created_at=(derived["created_at"] if derived else row.created_at.isoformat()),
                updated_at=(derived["updated_at"] if derived else row.updated_at.isoformat()),
                message_count=derived["message_count"] if derived else 0,
                preview=derived["preview"] if derived else "",
                is_pinned=row.is_pinned,
            )
        )

    results.sort(key=lambda c: c.updated_at, reverse=True)
    return results[:limit]


@router.post("", response_model=ConversationSummaryOut)
async def create_conversation(
    request: Request,
    payload: ConversationCreateIn,
    conversation_repo: ConversationRepository = Depends(get_conversation_repo),
) -> ConversationSummaryOut:
    """Creates a new, empty conversation's metadata row. No message turns
    exist yet, so it will not appear from Hippocampus alone until the first
    turn is recorded, this row is what makes it visible in the meantime."""
    tenant_id = resolve_tenant_id(request)
    conversation_id = payload.id or str(uuid.uuid4())
    try:
        row = conversation_repo.create(
            conversation_id=conversation_id, tenant_id=tenant_id, title=payload.title
        )
    except ConversationIdConflict:
        raise HTTPException(status_code=409, detail=f"Conversation id {conversation_id!r} is already in use")
    return ConversationSummaryOut(
        id=row.id,
        title=row.title,
        created_at=row.created_at.isoformat(),
        updated_at=row.updated_at.isoformat(),
        message_count=0,
        preview="",
        is_pinned=row.is_pinned,
    )


@router.patch("/{conversation_id}", response_model=ConversationSummaryOut)
async def patch_conversation(
    conversation_id: str,
    payload: ConversationPatchIn,
    request: Request,
    conversation_repo: ConversationRepository = Depends(get_conversation_repo),
) -> ConversationSummaryOut:
    """Renames and/or pins/unpins a conversation. Adopts (creates) the local
    metadata row on first write if it doesn't exist yet."""
    tenant_id = resolve_tenant_id(request)
    row = None
    try:
        if payload.title is not None:
            row = conversation_repo.rename(conversation_id, payload.title, tenant_id=tenant_id)
        if payload.is_pinned is not None:
            row = conversation_repo.set_pinned(conversation_id, payload.is_pinned, tenant_id=tenant_id)
    except ConversationIdConflict:
        raise HTTPException(status_code=409, detail=f"Conversation id {conversation_id!r} is already in use")
    if row is None:
        row = conversation_repo.get(conversation_id, tenant_id=tenant_id)
    if row is None:
        raise HTTPException(status_code=404, detail=f"Conversation {conversation_id} not found")

    return ConversationSummaryOut(
        id=row.id,
        title=row.title,
        created_at=row.created_at.isoformat(),
        updated_at=row.updated_at.isoformat(),
        message_count=0,
        preview="",
        is_pinned=row.is_pinned,
    )


@router.delete("/{conversation_id}", status_code=204)
async def delete_conversation(
    conversation_id: str,
    request: Request,
    conversation_repo: ConversationRepository = Depends(get_conversation_repo),
) -> None:
    """Soft-deletes a conversation: its metadata row is marked deleted and it
    stops appearing in `list_conversations`. Message turns already recorded
    in Hippocampus are left untouched (Hippocampus has no delete verb)."""
    conversation_repo.soft_delete(conversation_id, tenant_id=resolve_tenant_id(request))


@router.get("/{conversation_id}", response_model=ConversationDetailOut)
async def get_conversation(
    conversation_id: str,
    request: Request,
    hippocampus: HippocampusClient = Depends(get_hippocampus_client),
    conversation_repo: ConversationRepository = Depends(get_conversation_repo),
) -> ConversationDetailOut:
    """Retrieves full conversation turn history for conversation_id from Hippocampus."""
    tenant_id = resolve_tenant_id(request)
    row = conversation_repo.get(conversation_id, tenant_id=tenant_id)
    if row is not None and row.deleted_at is not None:
        # Same "never resurrect a deleted conversation" rule as
        # `list_conversations`: Hippocampus's turns for it are still there
        # (it has no delete verb), but that must not make a directly-fetched
        # deleted conversation's detail come back either.
        raise HTTPException(status_code=404, detail=f"Conversation {conversation_id} not found")

    data = await _fetch_hippocampus_turns(hippocampus, f"conversation:{conversation_id}", tenant_id, 100)

    if not data:
        if row is None:
            raise HTTPException(status_code=404, detail=f"Conversation {conversation_id} not found")
        return ConversationDetailOut(
            id=row.id,
            title=row.title,
            created_at=row.created_at.isoformat(),
            updated_at=row.updated_at.isoformat(),
            messages=[],
            is_pinned=row.is_pinned,
        )

    # Sort turns chronologically
    sorted_turns = sorted(data, key=lambda m: m.get("created_at") or "")
    first_turn = sorted_turns[0]
    last_turn = sorted_turns[-1]

    messages: List[ConversationTurnOut] = []
    for m in sorted_turns:
        created_at = m.get("created_at") or ""
        mem_id = m.get("id", "")
        content = m.get("content") or ""
        # Hippocampus's `GET /api/v1/memories` (and `/recall`) never echoes
        # back the `metadata`/`tags` a memory was created with (see its
        # `MemoryOut` schema, no such fields) even though `record_conversation_turn`
        # sends them on write -- so `metadata.turn` is never actually
        # retrievable here, only `content` reliably round-trips. Parse the
        # known `"User: {u}\n\nAssistant: {a}"` format `record_conversation_turn`
        # always writes instead of relying on metadata that can't come back.
        marker = "\n\nAssistant: "
        if content.startswith("User: ") and marker in content:
            user_text, _, assistant_text = content.partition(marker)
            user_text = user_text[len("User: "):]
            messages.append(ConversationTurnOut(id=f"{mem_id}-u", role="user", content=user_text, created_at=created_at))
            messages.append(ConversationTurnOut(id=f"{mem_id}-a", role="assistant", content=assistant_text, created_at=created_at))
        else:
            messages.append(ConversationTurnOut(id=mem_id, role="user", content=content, created_at=created_at))

    if row is None:
        try:
            row = conversation_repo.create(
                conversation_id=conversation_id,
                tenant_id=tenant_id,
                title=first_turn.get("title") or f"Conversation {conversation_id}",
            )
        except ConversationIdConflict:
            # Hippocampus already scoped `data` to this tenant's own
            # workspace_id above, so reaching here would mean the id
            # collides with a different tenant's *local* row specifically
            # -- surface the messages without a persisted local row rather
            # than fail the whole read.
            logger.warning("conversation id %r collides with another tenant's local row", conversation_id)
            return ConversationDetailOut(
                id=conversation_id,
                title=first_turn.get("title") or f"Conversation {conversation_id}",
                created_at=first_turn.get("created_at") or "",
                updated_at=last_turn.get("updated_at") or last_turn.get("created_at") or "",
                messages=messages,
                is_pinned=False,
            )

    return ConversationDetailOut(
        id=conversation_id,
        title=row.title,
        created_at=first_turn.get("created_at") or "",
        updated_at=last_turn.get("updated_at") or last_turn.get("created_at") or "",
        messages=messages,
        is_pinned=row.is_pinned,
    )
