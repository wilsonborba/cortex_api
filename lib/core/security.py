from __future__ import annotations

import json
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
    re.compile(r"/etc/passwd", re.IGNORECASE),
    re.compile(r"\bshutdown\s+-h\b", re.IGNORECASE),
    re.compile(r"\breboot\b", re.IGNORECASE),
]

_EXPLOIT_PATTERNS = [
    re.compile(r"system\s*:\s*you\s+are\s+now\s+unrestricted", re.IGNORECASE),
    re.compile(r"override\s+security\s+policy", re.IGNORECASE),
    re.compile(r"dump\s+environment\s+keys", re.IGNORECASE),
    re.compile(r"ignore\s+previous\s+instructions", re.IGNORECASE),
]

_TUTORIAL_PATTERNS = [
    re.compile(r"\bcomo\s+funciona\b", re.IGNORECASE),
    re.compile(r"\btutorial\b", re.IGNORECASE),
    re.compile(r"\bexplic\b", re.IGNORECASE),
    re.compile(r"\bhow\s+does\b", re.IGNORECASE),
    re.compile(r"\bexplain\b", re.IGNORECASE),
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
    Layer 2: Instant Context & Intent Evaluation.
    """

    def detect_risk_flag(self, text: str) -> bool:
        """Layer 1: Non-blocking deterministic metric scanner."""
        return any(pattern.search(text) for pattern in _RISK_PATTERNS)

    def evaluate(self, prompt: str) -> SecurityEvaluation:
        """Layer 1 + Layer 2 Context & Intent Evaluation in < 1ms."""
        has_risk_flag = self.detect_risk_flag(prompt)

        # Layer 2 Check: Explicit prompt injection exploit patterns -> BLOCKED
        for pattern in _EXPLOIT_PATTERNS:
            if pattern.search(prompt):
                return SecurityEvaluation(
                    risk_flag_detected=has_risk_flag,
                    is_blocked=True,
                    error_type="security_policy_violation",
                    reason="Prompt injection or exploit payload detected in request context.",
                )

        # Layer 2 Check: If risk flag detected but prompt is a tutorial/explanation -> ALLOWED
        if has_risk_flag:
            for pattern in _TUTORIAL_PATTERNS:
                if pattern.search(prompt):
                    return SecurityEvaluation(risk_flag_detected=True, is_blocked=False)

        return SecurityEvaluation(risk_flag_detected=has_risk_flag, is_blocked=False)
