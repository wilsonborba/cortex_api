from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Optional

_RISK_PATTERNS = [
    re.compile(r"\brm\s+-rf\b", re.IGNORECASE),
    re.compile(r"\bsudo\b", re.IGNORECASE),
    re.compile(r"\bdd\s+if=", re.IGNORECASE),
    re.compile(r"\bmkfs\b", re.IGNORECASE),
    re.compile(r"\bchmod\s+777\b", re.IGNORECASE),
    re.compile(r"/etc/shadow", re.IGNORECASE),
    re.compile(r"ignore\s+previous\s+instructions", re.IGNORECASE),
]

_EXPLOIT_PATTERNS = [
    re.compile(r"system\s*:\s*you\s+are\s+now\s+unrestricted", re.IGNORECASE),
    re.compile(r"override\s+security\s+policy", re.IGNORECASE),
]


@dataclass(frozen=True)
class SecurityEvaluation:
    risk_flag_detected: bool
    is_blocked: bool
    error_type: Optional[str] = None
    reason: Optional[str] = None


class SecurityShield:
    """Multi-layer Security Shield & Prompt Injection Guardrail."""

    def detect_risk_flag(self, text: str) -> bool:
        """Layer 1: Non-blocking deterministic metric scanner."""
        return any(pattern.search(text) for pattern in _RISK_PATTERNS)

    def evaluate(self, prompt: str) -> SecurityEvaluation:
        """Layer 1 + Layer 2 Context & Intent Evaluation."""
        has_risk_flag = self.detect_risk_flag(prompt)

        # Check for explicit prompt injection exploit attempts
        for pattern in _EXPLOIT_PATTERNS:
            if pattern.search(prompt):
                return SecurityEvaluation(
                    risk_flag_detected=has_risk_flag,
                    is_blocked=True,
                    error_type="security_policy_violation",
                    reason="Prompt injection or exploit payload detected in request context.",
                )

        # Legitimate discussion or tutorial questions containing risk keywords are allowed!
        # Only block if combined with destructive command execution intent
        if has_risk_flag and re.search(r"\bexecute\s+immediately\b|\brun\s+this\s+shell\s+script\s+now\b", prompt, re.IGNORECASE):
            return SecurityEvaluation(
                risk_flag_detected=True,
                is_blocked=True,
                error_type="dangerous_command",
                reason="Destructive shell payload combined with immediate execution instruction.",
            )

        return SecurityEvaluation(risk_flag_detected=has_risk_flag, is_blocked=False)
