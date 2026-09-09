from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query, Request

from lib.core.logs import get_logger
from lib.core.tenant import resolve_tenant_id
from lib.engine.memory_graph import build_workspace_memory_graph
from lib.engine.retrieval.hippocampus import HippocampusClient
from lib.presentation.api.deps import get_hippocampus_client
from lib.presentation.api.schemas.memory_graph import MemoryGraphOut, NodeContextOut

router = APIRouter(prefix="/memory-graph", tags=["memory-graph"])
logger = get_logger(__name__)

_TAG_PREFIX = "tag:"
_ENTITY_PREFIX = "entity:"
_RESOURCE_PREFIX = "resource:"


@router.get("", response_model=MemoryGraphOut)
async def get_workspace_memory_graph(
    request: Request,
    limit: int = Query(40, ge=1, le=150, description="how many of the tenant's own memories seed the graph"),
    depth: int = Query(1, ge=1, le=3),
    hippocampus: HippocampusClient = Depends(get_hippocampus_client),
) -> MemoryGraphOut:
    """Whole-workspace "second brain" overview: memories, tags, entities,
    resources and conversation attachments belonging to *only* the
    requesting tenant, assembled from hippocampus's per-memory graph route
    (see `build_workspace_memory_graph` for why this has to be aggregated
    client-side rather than requested in one call)."""
    tenant_id = resolve_tenant_id(request)
    graph = await build_workspace_memory_graph(hippocampus, tenant_id, seed_limit=limit, depth=depth)
    return MemoryGraphOut.model_validate(graph)


@router.get("/nodes/{node_id}/context", response_model=NodeContextOut)
async def get_node_context(
    node_id: str,
    request: Request,
    hippocampus: HippocampusClient = Depends(get_hippocampus_client),
) -> NodeContextOut:
    """Resolves a graph node back to the text used to seed a brand-new
    conversation when the user clicks it. Memory/attachment nodes carry
    their own bare hippocampus memory id (fetched tenant-scoped, so a
    guessed/observed id belonging to another tenant 404s here just like
    everywhere else in this API); tag/entity/resource nodes have no content
    of their own, so a short recall query against that tag/entity/resource
    name is used instead."""
    tenant_id = resolve_tenant_id(request)

    if node_id.startswith(_TAG_PREFIX) or node_id.startswith(_ENTITY_PREFIX) or node_id.startswith(_RESOURCE_PREFIX):
        _, _, name = node_id.partition(":")
        chunks = await hippocampus.search_memory(name, name, limit=5, tenant_id=tenant_id)
        if not chunks:
            raise HTTPException(status_code=404, detail=f"No memories found for {node_id!r}")
        content = "\n\n---\n\n".join(c.content for c in chunks)
        return NodeContextOut(node_id=node_id, title=name, content=content, available=True)

    context = await hippocampus.resolve_document_context(node_id, workspace_id=tenant_id)
    if not context.available:
        raise HTTPException(status_code=404, detail=f"Memory {node_id!r} not found")
    return NodeContextOut(
        node_id=node_id,
        title=context.title,
        content="\n\n".join(context.chunks),
        available=True,
    )
