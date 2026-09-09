from __future__ import annotations

from typing import Any, Dict, List

from lib.engine.retrieval.hippocampus import HippocampusClient

_ATTACHMENT_TITLE_PREFIX = "attachment:"


def _relabel_attachment_node(node: Dict[str, Any]) -> Dict[str, Any]:
    """Hippocampus has no dedicated "attachment" concept: `Executor` stores a
    conversation attachment's extracted text as a plain memory whose `title`
    is `f"attachment:{filename}"` (see `executor.py`'s `store_event` call).
    A memory-graph node built from one of those looks, node-type-wise,
    identical to any other memory node -- detect the convention here so the
    frontend can render/link it distinctly, instead of teaching hippocampus
    (which has no reason to know Cortex's attachment convention) about it."""
    label = node.get("label") or ""
    if node.get("node_type") == "memory" and label.startswith(_ATTACHMENT_TITLE_PREFIX):
        filename = label[len(_ATTACHMENT_TITLE_PREFIX):].strip() or label
        return {**node, "node_type": "attachment", "label": filename, "subtitle": "attachment"}
    return node


async def build_workspace_memory_graph(
    hippocampus: HippocampusClient,
    tenant_id: str,
    seed_limit: int = 40,
    depth: int = 1,
    max_total_nodes: int = 400,
) -> Dict[str, Any]:
    """Assembles a whole-workspace memory graph for the "second brain"
    overview screen by seeding from the tenant's most relevant memories and
    merging each one's bounded single-root subgraph (hippocampus's real
    `/graph` route only ever accepts one root per call -- see
    `HippocampusClient.get_memory_subgraph`).

    Every underlying call is already workspace-scoped (`list_memories`,
    `get_memory_subgraph`), so a seed or a neighbor belonging to another
    tenant can never enter `nodes`/`edges` here -- this function only
    merges/dedupes what those calls already returned, it does not itself
    do any cross-tenant filtering.
    """
    seeds = await hippocampus.list_memories(tenant_id, limit=seed_limit)
    seed_ids = [m.get("id") for m in seeds if m.get("id")]

    nodes_by_id: Dict[str, Dict[str, Any]] = {}
    edges_by_key: Dict[tuple, Dict[str, Any]] = {}
    truncated = False

    for memory_id in seed_ids:
        if len(nodes_by_id) >= max_total_nodes:
            truncated = True
            break
        subgraph = await hippocampus.get_memory_subgraph(
            memory_id, tenant_id, depth=depth, max_nodes=max_total_nodes
        )
        for node in subgraph.get("nodes", []):
            node_id = node.get("id")
            if not node_id or node_id in nodes_by_id:
                continue
            if len(nodes_by_id) >= max_total_nodes:
                truncated = True
                break
            nodes_by_id[node_id] = _relabel_attachment_node(node)
        for edge in subgraph.get("edges", []):
            key = (edge.get("source_id"), edge.get("target_id"), edge.get("edge_type"))
            if all(key) and key not in edges_by_key:
                edges_by_key[key] = edge

    edges = [
        e for e in edges_by_key.values()
        if e.get("source_id") in nodes_by_id and e.get("target_id") in nodes_by_id
    ]

    return {
        "nodes": list(nodes_by_id.values()),
        "edges": edges,
        "root_ids": seed_ids,
        "truncated": truncated,
    }
