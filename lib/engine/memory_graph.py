from __future__ import annotations

from typing import Any, Dict, List, Optional

from lib.dal.repositories.conversation_repository import ConversationRepository
from lib.engine.retrieval.hippocampus import HippocampusClient

ATTACHMENT_TITLE_PREFIX = "attachment:"
_CONVERSATION_TURN_TITLE_PREFIX = "Turn in "
_TURN_LABEL_MAX = 60
_CONVERSATION_TAG_PREFIX = "conversation:"
_CONVERSATION_LABEL_ID_MAX = 18
# Pure bookkeeping tags this API writes on every conversation-turn memory
# (see `record_conversation_turn`) that carry no useful information at all:
# `tenant:*` is the same single value for literally every node in a given
# tenant's whole graph (a universal hub, not a topic), and
# `type:conversation-turn` is shared by every turn regardless of which
# conversation it's in. `conversation:*` is different -- unlike those two,
# it genuinely varies per conversation and is the *only* thing that links a
# conversation's turns together in this graph (no memory-to-memory
# relationships exist between them), so it's turned into a proper cluster
# node (see `_conversation_cluster_node`) instead of being dropped: without
# it, the graph was just a field of fully disconnected cards.
_NOISE_TAG_PREFIXES = ("tenant:", "type:conversation")


def _is_noise_tag_node(node: Dict[str, Any]) -> bool:
    if node.get("node_type") != "tag":
        return False
    label = node.get("label") or ""
    return label.startswith(_NOISE_TAG_PREFIXES)


def _is_conversation_tag_node(node: Dict[str, Any]) -> bool:
    return node.get("node_type") == "tag" and (node.get("label") or "").startswith(_CONVERSATION_TAG_PREFIX)


def _conversation_cluster_node(
    node: Dict[str, Any], tenant_id: str, conversation_repo: Optional[ConversationRepository]
) -> Dict[str, Any]:
    """Turns a `conversation:{id}` tag node into a `cluster` node labeled
    with that conversation's real (possibly user-renamed) title when this
    API has a local metadata row for it, so the hub a conversation's turns
    all connect to reads as "TESTEEEE1234124" or "pdf", not a raw,
    guess-what-this-is conversation id."""
    conversation_id = (node.get("label") or "")[len(_CONVERSATION_TAG_PREFIX):]
    title = None
    if conversation_repo is not None and conversation_id:
        row = conversation_repo.get(conversation_id, tenant_id=tenant_id)
        if row is not None:
            title = row.title
    if not title:
        short_id = (
            conversation_id if len(conversation_id) <= _CONVERSATION_LABEL_ID_MAX
            else conversation_id[:_CONVERSATION_LABEL_ID_MAX] + "…"
        )
        title = f"Conversation {short_id}"
    return {**node, "node_type": "cluster", "label": title, "subtitle": "conversation"}


def _clean_turn_label(node: Dict[str, Any]) -> Dict[str, Any]:
    """Every conversation-turn memory this API ever writes gets the exact
    same generic title (`f"Turn in {conversation_id}"`, see
    `record_conversation_turn`) -- needed so `list_conversations`/
    `get_conversation` can recover `conversation_id` from it (hippocampus
    never echoes tags/metadata back on read, title is the only channel that
    reliably round-trips), but it means every one of these nodes' graph
    label looks identical and tells the user nothing about what's actually
    in it. Hippocampus's own node label always prefers that generic title
    over the memory's real content (correctly, for callers with real
    human-written titles), so it now also exposes a raw `content_preview`
    in metadata (see `MemoryGraphService.build_graph`) this rebuilds a
    useful label from instead, without touching the title contract
    `list_conversations`/`get_conversation` depend on."""
    label = node.get("label") or ""
    if not label.startswith(_CONVERSATION_TURN_TITLE_PREFIX):
        return node
    preview = (node.get("metadata") or {}).get("content_preview")
    if not preview:
        return node
    # `record_conversation_turn` always writes content as
    # `f"User: {user_prompt}\n\nAssistant: {assistant_response}"`; the user's
    # own words are the useful "keyword" for a label, the "User: " marker
    # and the assistant's reply are just noise here.
    text = preview
    if text.startswith("User: "):
        text = text[len("User: "):]
    text = text.split("\n\nAssistant:", 1)[0].strip()
    if not text:
        return node
    cleaned = text if len(text) <= _TURN_LABEL_MAX else text[: _TURN_LABEL_MAX - 1] + "…"
    return {**node, "label": cleaned}


def _relabel_attachment_node(node: Dict[str, Any]) -> Dict[str, Any]:
    """Hippocampus has no dedicated "attachment" concept: `Executor` stores a
    conversation attachment's extracted text as a plain memory whose `title`
    is `f"attachment:{filename}"` (see `executor.py`'s `store_event` call).
    A memory-graph node built from one of those looks, node-type-wise,
    identical to any other memory node -- detect the convention here so the
    frontend can render/link it distinctly, instead of teaching hippocampus
    (which has no reason to know Cortex's attachment convention) about it."""
    label = node.get("label") or ""
    if node.get("node_type") == "memory" and label.startswith(ATTACHMENT_TITLE_PREFIX):
        filename = label[len(ATTACHMENT_TITLE_PREFIX):].strip() or label
        return {**node, "node_type": "attachment", "label": filename, "subtitle": "attachment"}
    return _clean_turn_label(node)


async def build_workspace_memory_graph(
    hippocampus: HippocampusClient,
    tenant_id: str,
    seed_limit: int = 40,
    depth: int = 1,
    max_total_nodes: int = 400,
    conversation_repo: Optional[ConversationRepository] = None,
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
    do any cross-tenant filtering. [conversation_repo] is only used to look
    up a conversation cluster node's real (possibly user-renamed) title --
    entirely optional, a missing/None repo just falls back to a generic
    "Conversation {id}" label.
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
            if not node_id or node_id in nodes_by_id or _is_noise_tag_node(node):
                continue
            if len(nodes_by_id) >= max_total_nodes:
                truncated = True
                break
            if _is_conversation_tag_node(node):
                nodes_by_id[node_id] = _conversation_cluster_node(node, tenant_id, conversation_repo)
            else:
                nodes_by_id[node_id] = _relabel_attachment_node(node)
        for edge in subgraph.get("edges", []):
            key = (edge.get("source_id"), edge.get("target_id"), edge.get("edge_type"))
            if all(key) and key not in edges_by_key:
                edges_by_key[key] = edge

    edges = [
        e for e in edges_by_key.values()
        if e.get("source_id") in nodes_by_id and e.get("target_id") in nodes_by_id
    ]

    # Give each conversation cluster its member ids (frontend shows a count
    # and, when expanded, the members themselves), computed from the edges
    # that point at it rather than trusted from hippocampus's own payload.
    cluster_members: Dict[str, List[str]] = {}
    for e in edges:
        target = e.get("target_id")
        if target in nodes_by_id and nodes_by_id[target].get("node_type") == "cluster":
            cluster_members.setdefault(target, []).append(e.get("source_id"))
    nodes = [
        {**n, "metadata": {**(n.get("metadata") or {}), "cluster_of": cluster_members[node_id]}}
        if node_id in cluster_members else n
        for node_id, n in nodes_by_id.items()
    ]

    return {
        "nodes": nodes,
        "edges": edges,
        "root_ids": seed_ids,
        "truncated": truncated,
    }
