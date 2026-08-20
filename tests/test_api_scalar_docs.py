from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

from lib.core.settings import get_settings
from lib.presentation.api.app import create_app


def _test_settings():
    return get_settings().model_copy(
        update={"api_sync_models_on_startup": False, "api_background_tasks_enabled": False}
    )


@pytest.fixture
def client():
    app = create_app(settings=_test_settings())
    with TestClient(app) as client_:
        yield client_


def test_scalar_docs_endpoint(client: TestClient):
    response = client.get("/scalar")
    assert response.status_code == 200
    assert "text/html" in response.headers["content-type"]
    assert "Scalar" in response.text
    assert "@scalar/api-reference" in response.text


def test_docs_endpoint(client: TestClient):
    response = client.get("/docs")
    assert response.status_code == 200
    assert "text/html" in response.headers["content-type"]
    assert "Scalar" in response.text
    assert "@scalar/api-reference" in response.text


def test_docs_scalar_alias_endpoint(client: TestClient):
    response = client.get("/docs/scalar")
    assert response.status_code == 200
    assert "text/html" in response.headers["content-type"]
    assert "api-reference" in response.text


def test_openapi_tags_metadata(client: TestClient):
    response = client.get("/openapi.json")
    assert response.status_code == 200
    data = response.json()
    assert "openapi" in data
    assert "info" in data
    assert "paths" in data
    assert "/execute" in data["paths"]
    assert "/v1/chat/completions" in data["paths"]
    assert "/models" in data["paths"]
    assert "/quota" in data["paths"]
    assert "/tiers" in data["paths"]
    assert "/routing/pins" in data["paths"]
    assert "/telemetry/stats" in data["paths"]
    assert "/attachments/video" in data["paths"]
