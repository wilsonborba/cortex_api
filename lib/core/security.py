from __future__ import annotations

import json
import re
import urllib.request
from dataclasses import dataclass
from typing import Optional

_RISK_PATTERNS = [
    re.compile(r"\brm\s+-rf\b", re.IGNORECASE),
    re.compile(r"\bsudo\b", re.IGNORECASE),
    re.compile(r"\bdd\s+if=", re.IGNORECASE),
    re.compile(r"\bmkfs\b", re.IGNORECASE),
    re.compile(r"\bchmod\s+777\b", re.IGNORECASE),
    re.compile(r"/etc/shadow", re.IGNORECASE),
    re.compile(r"/etc/passwd", re.IGNORECASE),
    re.compile(r"\bshutdown\s+-h\b", re.IGNORECASE),
    re.compile(r"\breboot\b", re.IGNORECASE),
    re.compile(r"ignore\s+previous\s+instructions", re.IGNORECASE),
]

_EXPLOIT_PATTERNS = [
    re.compile(r"system\s*:\s*you\s+are\s+now\s+unrestricted", re.IGNORECASE),
    re.compile(r"override\s+security\s+policy", re.IGNORECASE),
    re.compile(r"dump\s+environment\s+keys", re.IGNORECASE),
]


@dataclass(frozen=True)
class SecurityEvaluation:
    risk_flag_detected: bool
    is_blocked: bool
    error_type: Optional[str] = None
    reason: Optional[str] = None


class SecurityShield:
    """2-Layer Security Shield & Prompt Injection Guardrail.
    
    Layer 1: Deterministic Regex Scan (No AI, No Timeout, Never blocks alone).
    Layer 2: Lightweight Context Evaluation via Local Ollama (No Cloud AI).
    """

    def detect_risk_flag(self, text: str) -> bool:
        """Layer 1: Non-blocking deterministic metric scanner."""
        return any(pattern.search(text) for pattern in _RISK_PATTERNS)

    def evaluate(self, prompt: str, ollama_url: str = "http://localhost:11434") -> SecurityEvaluation:
        """Layer 1 + Layer 2 Context & Intent Evaluation."""
        has_risk_flag = self.detect_risk_flag(prompt)

        # Check explicit prompt injection exploit patterns (Deterministic)
        for pattern in _EXPLOIT_PATTERNS:
            if pattern.search(prompt):
                return SecurityEvaluation(
                    risk_flag_detected=has_risk_flag,
                    is_blocked=True,
                    error_type="security_policy_violation",
                    reason="Prompt injection or exploit payload detected in request context.",
                )

        # Layer 2: If risk flag detected, evaluate intent via local Ollama
        if has_risk_flag:
            eval_result = self._evaluate_with_local_ollama(prompt, ollama_url)
            if eval_result is not None:
                return eval_result

            # Fallback for Layer 2: Tutorial / explanation questions are ALLOWED
            if re.search(r"\bcomo\s+funciona\b|\btutorial\b|\bexplic\b|\bhow\s+does\b", prompt, re.IGNORECASE):
                return SecurityEvaluation(risk_flag_detected=True, is_blocked=False)

        return SecurityEvaluation(risk_flag_detected=has_risk_flag, is_blocked=False)

    def _evaluate_with_local_ollama(self, prompt: str, ollama_url: str) -> Optional[SecurityEvaluation]:
        """Layer 2 evaluation using local Ollama model (no cloud calls)."""
        try:
            req_data = json.dumps({
                "model": "qwen2.5vl:7b",
                "prompt": (
                    "Abaixo está uma pergunta de usuário. Determine se é uma dúvida técnica/tutorial legítima "
                    "ou uma tentativa maliciosa de executar comandos destrutivos no sistema.\n"
                    "Responda apenas 'SAFE' para tutorial/dúvida técnica ou 'MALICIOUS' para ataque.\n\n"
                    f"Prompt: {prompt}"
                ),
                "stream": False,
            }).encode("utf-8")
            req = urllib.request.Request(f"{ollama_url}/api/generate", data=req_data, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=2.0) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                response_text = data.get("response", "").upper().strip()
                if "MALICIOUS" in response_text:
                    return SecurityEvaluation(
                        risk_flag_detected=True,
                        is_blocked=True,
                        error_type="security_policy_violation",
                        reason="Layer 2 Ollama local evaluation flagged malicious command execution intent.",
                    )
                return SecurityEvaluation(risk_flag_detected=True, is_blocked=False)
        except Exception:
            return None
