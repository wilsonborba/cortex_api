from __future__ import annotations

from typing import Any, Dict, List, Optional
from pydantic import BaseModel


class GraphNodeOut(BaseModel):
    id: str
    node_type: str  # memory | attachment | tag | entity | resource | cluster
    label: str
    subtitle: Optional[str] = None
    metadata: Dict[str, Any] = {}


class GraphEdgeOut(BaseModel):
    source_id: str
    target_id: str
    edge_type: str
    confidence: Optional[float] = None


class MemoryGraphOut(BaseModel):
    nodes: List[GraphNodeOut] = []
    edges: List[GraphEdgeOut] = []
    root_ids: List[str] = []
    truncated: bool = False


class NodeContextOut(BaseModel):
    node_id: str
    title: str = ""
    content: str = ""
    available: bool = True
