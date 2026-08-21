from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Optional, Protocol


@dataclass(frozen=True)
class DiscoveredModel:
    """A model observed from a live provider probe, not yet persisted.

    Mirrors the shape of `lib.dal.models.ModelCatalogEntry` but stays decoupled
    from the ORM so discovery adapters have no dependency on SQLAlchemy.
    """

    id: str
    provider: str
    display_name: str
    access_status: str
    status_reason: Optional[str] = None
    parameter_size: Optional[str] = None
    context_window: int = 8192
    is_local: bool = False
    source_kind: str = "api"
    tier_eligibility: list[int] = field(default_factory=list)
    capabilities: dict[str, Any] = field(default_factory=dict)
    cost_per_million_tokens: float = 0.0


class ProviderDiscoveryError(RuntimeError):
    """Raised when a provider cannot be probed at all (unreachable, CLI missing, not signed in).

    Distinct from a model simply being `REQUIRES_SUBSCRIPTION` or gated: that's still a
    successful discovery. This error means the adapter has no live signal to report.
    """

    def __init__(self, provider: str, message: str) -> None:
        super().__init__(message)
        self.provider = provider


class ProviderDiscovery(Protocol):
    """Contract every discovery adapter implements."""

    provider: str

    def discover(self) -> list[DiscoveredModel]:
        """Return the models currently visible for this provider.

        Raises `ProviderDiscoveryError` if the provider itself could not be reached/probed.
        """
        ...
