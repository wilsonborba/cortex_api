from __future__ import annotations

import shutil
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from lib.core.settings import Settings, get_settings


@dataclass(frozen=True)
class CapabilitiesConfig:
    memory: bool = False
    tasks: bool = False
    thinking: bool = False
    web: bool = False
    temporary: bool = False

    def to_dict(self) -> Dict[str, bool]:
        return {
            "memory": self.memory,
            "tasks": self.tasks,
            "thinking": self.thinking,
            "web": self.web,
            "temporary": self.temporary,
        }

    @classmethod
    def from_dict(cls, data: Optional[Dict[str, Any]]) -> CapabilitiesConfig:
        if not data:
            return cls()
        return cls(
            memory=bool(data.get("memory", False)),
            tasks=bool(data.get("tasks", False)),
            thinking=bool(data.get("thinking", False)),
            web=bool(data.get("web", False)),
            temporary=bool(data.get("temporary", False)),
        )


@dataclass(frozen=True)
class RuntimeCapabilities:
    profile: str
    core_orchestration: bool = True
    openai_facade: bool = True
    cloud_providers_configured: int = 0
    local_whisper_available: bool = False
    cloud_transcription_available: bool = False
    video_ingestion_available: bool = False
    crawler_available: bool = False
    postgres_available: bool = False
    missing_optional_components: List[str] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return {
            "profile": self.profile,
            "capabilities": {
                "core_orchestration": self.core_orchestration,
                "openai_facade": self.openai_facade,
                "cloud_providers_configured": self.cloud_providers_configured,
                "local_whisper": self.local_whisper_available,
                "cloud_transcription": self.cloud_transcription_available,
                "video_ingestion": self.video_ingestion_available,
                "crawler": self.crawler_available,
                "postgres": self.postgres_available,
            },
            "missing_optional_components": self.missing_optional_components,
        }


def detect_runtime_capabilities(settings: Optional[Settings] = None) -> RuntimeCapabilities:
    settings = settings or get_settings()
    profile = settings.profile.lower()

    cloud_keys = [
        settings.groq_api_key,
        settings.google_ai_studio_api_key,
        settings.openrouter_api_key,
        settings.cloudflare_api_key,
        settings.cohere_api_key,
        settings.mistral_api_key,
        settings.nvidia_api_key,
        settings.zai_api_key,
        settings.requesty_api_key,
        settings.huggingface_api_key,
        settings.ollama_cloud_api_key,
        settings.aion_labs_api_key,
        settings.siliconflow_api_key,
        settings.inference_net_api_key,
        settings.sambanova_api_key,
    ]
    cloud_count = sum(1 for k in cloud_keys if k)

    local_whisper = False
    if settings.whisper_local_enabled:
        try:
            import faster_whisper  # noqa: F401
            local_whisper = True
        except ImportError:
            local_whisper = False

    cloud_transcription = bool(settings.groq_api_key)
    has_ffmpeg = bool(shutil.which("ffmpeg"))
    video_ingestion = has_ffmpeg and (local_whisper or cloud_transcription)

    try:
        import crawl4ai  # noqa: F401
        crawler = True
    except ImportError:
        crawler = False

    try:
        import psycopg  # noqa: F401
        postgres = True
    except ImportError:
        postgres = False

    missing: List[str] = []
    if not local_whisper:
        missing.append("local_whisper (faster-whisper not installed)")
    if not cloud_transcription:
        missing.append("cloud_transcription (CORTEX_GROQ_API_KEY not set)")
    if not has_ffmpeg:
        missing.append("video_processing (ffmpeg binary not in PATH)")
    if not crawler:
        missing.append("crawler (crawl4ai not installed)")
    if not postgres:
        missing.append("postgres (psycopg not installed)")

    return RuntimeCapabilities(
        profile=profile,
        core_orchestration=True,
        openai_facade=True,
        cloud_providers_configured=cloud_count,
        local_whisper_available=local_whisper,
        cloud_transcription_available=cloud_transcription,
        video_ingestion_available=video_ingestion,
        crawler_available=crawler,
        postgres_available=postgres,
        missing_optional_components=missing,
    )
