"""Resolves the real per-user scope for Hippocampus reads/writes.

`api_for_apps`'s generic proxy (`apps_route.py:proxy_endpoint`) injects
`X-Uuid` with the authenticated user's real UUID on every request it
forwards to `cortex_api` -- but until now nothing here read it, so every
conversation/memory operation used a client-suppliable `tenant_id` that
defaulted to (and in practice always was) the literal string `"default"`,
shared by every user. This is the one place that stops trusting a
client-supplied tenant id and uses the proxy-verified identity instead.
"""
from __future__ import annotations

from typing import Optional

from fastapi import Request

X_UUID_HEADER = "x-uuid"
DEFAULT_TENANT_ID = "default"


def resolve_tenant_id(request: Request, client_supplied: Optional[str] = None) -> str:
    """The authoritative tenant/workspace id for this request.

    Prefers the real user UUID `api_for_apps`'s proxy verified and
    attached (present on every browser-originated, session-authenticated
    call, e.g. from `cortex`). Falls back to [client_supplied] when it's
    absent -- true server-to-server callers that never go through that
    proxy (e.g. `certifications_api` calling `/execute` directly on
    `localhost:8003` with its own `tenant_id="certifications"`) have no
    `x-uuid` to check and must keep working exactly as before; only
    proxied, end-user browser traffic gets the stronger per-user scoping.
    Falls back to `"default"` only when neither is available.
    """
    return request.headers.get(X_UUID_HEADER) or client_supplied or DEFAULT_TENANT_ID
