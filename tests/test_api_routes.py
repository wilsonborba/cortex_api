from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

from lib.core.settings import get_settings
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.presentation.api.app import create_app


def _test_settings():
    # Same test DB conftest.py already migrated; just keep the API from
    # doing real network/subprocess discovery or spawning a background loop.
    return get_settings().model_copy(
        update={"api_sync_models_on_startup": False, "api_background_tasks_enabled": False}
    )


@pytest.fixture
def client():
    app = create_app(settings=_test_settings())
    with TestClient(app) as c:
        yield c


@pytest.fixture
def model_repo_() -> ModelRepository:
    return ModelRepository()


def _seed_model(repo: ModelRepository, **overrides) -> ModelCatalogEntry:
    defaults = dict(
        provider="api-test-provider", display_name="model", access_status=AccessStatus.AVAILABLE.value,
        context_window=8192, is_local=True, tier_eligibility=[0, 1, 2], capabilities={},
        cost_per_million_tokens=0.0, is_enabled=True,
    )
    defaults.update(overrides)
    return repo.upsert(ModelCatalogEntry(**defaults))


# --- OpenAPI / general -----------------------------------------------------------


def test_openapi_documents_every_route(client):
    response = client.get("/openapi.json")
    assert response.status_code == 200
    paths = response.json()["paths"]
    for path in [
        "/execute", "/models", "/models/sync", "/models/{model_id}", "/quota", "/quota/{provider}",
        "/tiers", "/tiers/{tier}", "/routing/pins", "/telemetry/stats", "/telemetry/events",
    ]:
        assert path in paths


def test_websocket_logs_stream_connects(client):
    with client.websocket_connect("/logs/stream"):
        pass  # connecting without error is the contract issue #2 already tests in depth


# --- /models -----------------------------------------------------------------------


def test_list_models_returns_seeded_model(client, model_repo_):
    _seed_model(model_repo_, id="api-test-provider/api-list-model", display_name="API List Model")

    response = client.get("/models")

    assert response.status_code == 200
    ids = {m["id"] for m in response.json()}
    assert "api-test-provider/api-list-model" in ids


def test_get_model_by_id(client, model_repo_):
    _seed_model(model_repo_, id="api-test-provider/api-get-model", display_name="API Get Model")

    response = client.get("/models/api-test-provider/api-get-model")

    assert response.status_code == 200
    assert response.json()["display_name"] == "API Get Model"


def test_get_model_404_when_missing(client):
    response = client.get("/models/api-test-provider/does-not-exist")
    assert response.status_code == 404


def test_configure_model_updates_tier_eligibility_and_enabled(client, model_repo_):
    _seed_model(model_repo_, id="api-test-provider/api-configure-model")

    response = client.patch("/models/api-test-provider/api-configure-model", json={"tier_eligibility": [0, 1], "is_enabled": False})

    assert response.status_code == 200
    body = response.json()
    assert body["tier_eligibility"] == [0, 1]
    assert body["is_enabled"] is False
    assert body["access_status"] == AccessStatus.DISABLED_MANUALLY.value  # ModelRegistryService side effect


def test_configure_model_404_when_missing(client):
    response = client.patch("/models/api-test-provider/does-not-exist", json={"is_enabled": False})
    assert response.status_code == 404


def test_configure_model_sets_context_format_pin_with_ttl(client, model_repo_):
    _seed_model(model_repo_, id="api-test-provider/pin-model")

    response = client.patch(
        "/models/api-test-provider/pin-model", json={"context_format_pin": "toon", "context_format_pin_ttl_seconds": 3600}
    )

    assert response.status_code == 200
    body = response.json()
    assert body["context_format_pin"] == "toon"
    assert body["context_format_pin_expires_at"] is not None

    fetched = client.get("/models/api-test-provider/pin-model")
    assert fetched.json()["context_format_pin"] == "toon"  # GET and PATCH agree (parity)


def test_configure_model_clears_context_format_pin(client, model_repo_):
    _seed_model(model_repo_, id="api-test-provider/pin-clear-model")
    client.patch("/models/api-test-provider/pin-clear-model", json={"context_format_pin": "json"})

    response = client.patch("/models/api-test-provider/pin-clear-model", json={"context_format_pin": "none"})

    assert response.status_code == 200
    assert response.json()["context_format_pin"] is None


def test_configure_model_rejects_unknown_context_format(client, model_repo_):
    _seed_model(model_repo_, id="api-test-provider/bad-format-model")

    response = client.patch("/models/api-test-provider/bad-format-model", json={"context_format_pin": "yaml"})

    assert response.status_code == 422


def test_sync_models_returns_a_list(client):
    response = client.post("/models/sync")
    assert response.status_code == 200
    assert isinstance(response.json(), list)


# --- /quota ------------------------------------------------------------------------


def test_quota_summary_is_a_list(client):
    response = client.get("/quota")
    assert response.status_code == 200
    assert isinstance(response.json(), list)


def test_quota_for_local_provider_is_unbounded(client):
    response = client.get("/quota/ollama")

    assert response.status_code == 200
    body = response.json()
    assert body["provider"] == "ollama"
    assert body["window_limit_tokens"] is None
    assert body["quota_factor"] == 1.0


# --- /tiers ------------------------------------------------------------------------


def test_list_tiers_includes_all_six_factory_presets(client):
    response = client.get("/tiers")

    assert response.status_code == 200
    tiers = {t["tier"] for t in response.json()}
    assert tiers == {0, 1, 2, 3, 4, 5}


def test_configure_tier_updates_latency_and_allowed_models(client):
    response = client.patch(
        "/tiers/2", json={"max_latency_seconds": 25, "allowed_models": ["ollama/api-tier-model"]}
    )

    assert response.status_code == 200
    body = response.json()
    assert body["max_latency_seconds"] == 25
    assert body["allowed_models"] == ["ollama/api-tier-model"]


def test_configure_tier_404_for_unknown_tier(client):
    response = client.patch("/tiers/9", json={"max_latency_seconds": 10})
    assert response.status_code == 404


# --- /routing/pins -----------------------------------------------------------------


def test_pin_lifecycle_create_list_delete(client):
    create = client.post(
        "/routing/pins",
        json={"tier": 3, "task": "api-pin-task", "strategy_id": "api_pin_v1", "pinned_by": "api-test"},
    )
    assert create.status_code == 201
    assert create.json()["strategy_id"] == "api_pin_v1"

    listed = client.get("/routing/pins")
    assert any(p["tier"] == 3 and p["task"] == "api-pin-task" for p in listed.json())

    deleted = client.delete("/routing/pins", params={"tier": 3, "task": "api-pin-task"})
    assert deleted.status_code == 204

    listed_after = client.get("/routing/pins")
    assert not any(p["tier"] == 3 and p["task"] == "api-pin-task" for p in listed_after.json())


def test_delete_pin_404_when_nothing_to_remove(client):
    response = client.delete("/routing/pins", params={"tier": 3, "task": "no-such-pin-task"})
    assert response.status_code == 404


# --- /telemetry ----------------------------------------------------------------------


def test_telemetry_events_is_a_list(client):
    response = client.get("/telemetry/events", params={"limit": 5})
    assert response.status_code == 200
    assert isinstance(response.json(), list)


def test_telemetry_stats_is_a_list(client):
    response = client.get("/telemetry/stats")
    assert response.status_code == 200
    assert isinstance(response.json(), list)
