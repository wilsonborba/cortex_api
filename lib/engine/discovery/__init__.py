from __future__ import annotations

from lib.engine.discovery.aion_labs import AionLabsDiscovery
from lib.engine.discovery.antigravity import AntigravityDiscovery
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscovery, ProviderDiscoveryError
from lib.engine.discovery.claude_docker import ClaudeDockerDiscovery
from lib.engine.discovery.cloudflare import CloudflareDiscovery
from lib.engine.discovery.codex import CodexDiscovery
from lib.engine.discovery.cohere import CohereDiscovery
from lib.engine.discovery.google_ai_studio import GoogleAIStudioDiscovery
from lib.engine.discovery.groq import GroqDiscovery
from lib.engine.discovery.huggingface import HuggingFaceDiscovery
from lib.engine.discovery.inference_net import InferenceNetDiscovery
from lib.engine.discovery.mistral import MistralDiscovery
from lib.engine.discovery.nvidia import NvidiaDiscovery
from lib.engine.discovery.ollama import OllamaDiscovery
from lib.engine.discovery.ollama_cloud import OllamaCloudDiscovery
from lib.engine.discovery.openai_compatible import OpenAICompatibleDiscovery
from lib.engine.discovery.openrouter import OpenRouterDiscovery
from lib.engine.discovery.requesty import RequestyDiscovery
from lib.engine.discovery.sambanova import SambaNovaDiscovery
from lib.engine.discovery.siliconflow import SiliconFlowDiscovery
from lib.engine.discovery.zai import ZaiDiscovery

__all__ = [
    "AionLabsDiscovery",
    "AntigravityDiscovery",
    "ClaudeDockerDiscovery",
    "CloudflareDiscovery",
    "CodexDiscovery",
    "CohereDiscovery",
    "DiscoveredModel",
    "GoogleAIStudioDiscovery",
    "GroqDiscovery",
    "HuggingFaceDiscovery",
    "InferenceNetDiscovery",
    "MistralDiscovery",
    "NvidiaDiscovery",
    "OllamaCloudDiscovery",
    "OllamaDiscovery",
    "OpenAICompatibleDiscovery",
    "OpenRouterDiscovery",
    "ProviderDiscovery",
    "ProviderDiscoveryError",
    "RequestyDiscovery",
    "SambaNovaDiscovery",
    "SiliconFlowDiscovery",
    "ZaiDiscovery",
]
