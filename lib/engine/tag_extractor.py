from __future__ import annotations

import json
import re
import urllib.request
from typing import List, Optional, Set

_ENTITY_KEYWORDS = [
    "cortex", "hippocampus", "plane", "sqlite", "postgresql", "architecture",
    "database", "dal", "logs", "task", "project", "tenant", "memory", "user",
    "meeting", "audio", "pdf", "transcript", "document", "work", "admin",
]


class TagExtractor:
    """Hybrid Entity Tag Extractor (Regex + Local Ollama)."""

    def extract_tags(self, text: str, tenant_id: str = "default", ollama_url: str = "http://localhost:11434") -> List[str]:
        tags: Set[str] = set()

        # 1. Deterministic Regex / Keyword extraction
        lowered = text.lower()
        for kw in _ENTITY_KEYWORDS:
            if kw in lowered:
                tags.add(kw)

        # Extract capitalized proper nouns / entity names
        words = re.findall(r"\b[A-Z][a-zA-Z0-9_-]{2,}\b", text)
        for w in words:
            tags.add(w.lower())

        # 2. Local Ollama Tag Extraction (optional light scan)
        try:
            req_data = json.dumps({
                "model": "qwen2.5vl:7b",
                "prompt": f"Extraia até 3 tags de entidades (palavras-chave em minúsculas) do texto a seguir. Responda apenas as palavras separadas por vírgula:\n\n{text}",
                "stream": False,
            }).encode("utf-8")
            req = urllib.request.Request(f"{ollama_url}/api/generate", data=req_data, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=2.0) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                resp_tags = data.get("response", "").strip()
                for tag in resp_tags.split(","):
                    t_clean = tag.strip().lower()
                    if t_clean and len(t_clean) < 20:
                        tags.add(t_clean)
        except Exception:
            pass

        # 3. Always bind closed tenant scope tag
        tags.add(f"tenant:{tenant_id}")
        return sorted(list(tags))
