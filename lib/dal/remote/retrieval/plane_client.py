from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional

import httpx

from lib.core.logs import get_logger
from lib.core.settings import Settings, get_settings

logger = get_logger(__name__)


@dataclass(frozen=True)
class PlaneTask:
    id: str
    name: str
    description: str = ""
    state: str = "open"
    priority: str = "medium"
    workspace_slug: str = ""
    project_id: str = ""


class PlaneClient:
    """Async client for plane-slim task management integration."""

    def __init__(
        self,
        base_url: str,
        timeout: float = 10.0,
        api_key: Optional[str] = None,
        client_factory: Optional[Callable[[], httpx.AsyncClient]] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._api_key = api_key
        self._client_factory = client_factory or (lambda: httpx.AsyncClient(timeout=self._timeout))

    def _headers(self) -> Dict[str, str]:
        return {"X-API-Key": self._api_key} if self._api_key else {}

    async def list_tasks(self, workspace_slug: str = "default", project_id: str = "00000000-0000-0000-0000-000000000001") -> List[PlaneTask]:
        try:
            async with self._client_factory() as client:
                resp = await client.get(
                    f"{self._base_url}/api/v1/workspaces/{workspace_slug}/projects/{project_id}/issues/",
                    headers=self._headers(),
                )
                resp.raise_for_status()
                data = resp.json()
                results = data if isinstance(data, list) else data.get("results", [])
                return [
                    PlaneTask(
                        id=str(item.get("id", "")),
                        name=str(item.get("name", "")),
                        description=str(item.get("description_html", "") or item.get("description", "")),
                        state=str(item.get("state", "open")),
                        workspace_slug=workspace_slug,
                        project_id=project_id,
                    )
                    for item in results
                ]
        except (httpx.HTTPError, ValueError) as exc:
            logger.warning("plane_client.list_tasks failed: %s", exc)
            return []

    async def create_task(self, workspace_slug: str = "default", project_id: str = "00000000-0000-0000-0000-000000000001", name: str = "", description: str = "") -> Optional[PlaneTask]:
        try:
            async with self._client_factory() as client:
                resp = await client.post(
                    f"{self._base_url}/api/v1/workspaces/{workspace_slug}/projects/{project_id}/issues/",
                    json={"name": name, "description": description},
                    headers=self._headers(),
                )
                resp.raise_for_status()
                item = resp.json()
                return PlaneTask(
                    id=str(item.get("id", "")),
                    name=str(item.get("name", "")),
                    description=description,
                    workspace_slug=workspace_slug,
                    project_id=project_id,
                )
        except (httpx.HTTPError, ValueError) as exc:
            logger.warning("plane_client.create_task failed: %s", exc)
            return None


def build_default_plane_client(settings: Optional[Settings] = None) -> PlaneClient:
    settings = settings or get_settings()
    return PlaneClient(
        base_url=getattr(settings, "plane_url", "http://localhost:8011"),
        timeout=getattr(settings, "plane_timeout_seconds", 10.0),
        api_key=getattr(settings, "plane_api_key", "cortex-test-key") or "cortex-test-key",
    )
