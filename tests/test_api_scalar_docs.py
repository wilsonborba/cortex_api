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


def test_localized_scalar_docs_switching(client: TestClient):
    # Portuguese Docs
    res_pt = client.get("/docs?lang=pt")
    assert res_pt.status_code == 200
    assert "data-url=\"/openapi.json?lang=pt\"" in res_pt.text
    assert "Português" in res_pt.text

    # Thai Docs
    res_th = client.get("/docs?lang=th")
    assert res_th.status_code == 200
    assert "data-url=\"/openapi.json?lang=th\"" in res_th.text
    assert "ไทย" in res_th.text


def test_localized_openapi_specs(client: TestClient):
    # Portuguese Spec
    res_pt = client.get("/openapi.json?lang=pt")
    assert res_pt.status_code == 200
    data_pt = res_pt.json()
    assert "API de Orquestração Multi-Modelo Cortex" in data_pt["info"]["title"]
    assert "Executar Prompt com Roteamento Adaptativo" in data_pt["paths"]["/execute"]["post"]["summary"]

    # Thai Spec
    res_th = client.get("/openapi.json?lang=th")
    assert res_th.status_code == 200
    data_th = res_th.json()
    assert "Cortex API การจัดการ AI หลายโมเดลแบบรวมศูนย์" in data_th["info"]["title"]
    assert "ประมวลผล Prompt ด้วยการเลือกเส้นทางแบบปรับตัว" in data_th["paths"]["/execute"]["post"]["summary"]

    # Direct lang alias endpoints
    res_direct_pt = client.get("/openapi-pt.json")
    assert res_direct_pt.status_code == 200
    assert "Orquestração" in res_direct_pt.json()["info"]["title"]

    res_direct_th = client.get("/openapi-th.json")
    assert res_direct_th.status_code == 200
    assert "การจัดการ" in res_direct_th.json()["info"]["title"]
