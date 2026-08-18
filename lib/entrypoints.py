from __future__ import annotations


def cli_entrypoint() -> None:
    from lib.presentation.cli.main import app

    app()


def api_entrypoint() -> None:
    import uvicorn
    from lib.core.settings import get_settings

    settings = get_settings()
    uvicorn.run(
        "lib.presentation.api.app:create_app",
        factory=True,
        host=settings.api_host,
        port=settings.api_port,
        reload=False,
    )
