#!/usr/bin/env python3
"""test_tiers.py: Helper script to test all Cortex tiers (0-5) via API."""

import json
import time
import urllib.request

tiers = [
    (0, "Qual é a capital da França?"),
    (1, "Escreva uma função simples em Python que calcula o fatorial."),
    (2, "Explique a diferença entre sincronismo e assincronismo em duas frases."),
    (3, "Escreva uma função em Python para busca binária em uma lista ordenada com doctests."),
    (4, "Analise a complexidade de tempo e espaço do QuickSort e o impacto do pivô."),
    (5, "Projete a arquitetura de um rate limiter distribuído tolerante a falhas para 100k req/s."),
]

def main() -> None:
    print("========================================================")
    print(" 🚀 Testing Cortex Tiers (0-5) via API (http://127.0.0.1:8003)")
    print("========================================================\n")

    for tier, prompt in tiers:
        start = time.time()
        req = urllib.request.Request(
            "http://127.0.0.1:8003/v1/chat/completions",
            data=json.dumps({"model": f"tier-{tier}", "messages": [{"role": "user", "content": prompt}]}).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                elapsed = time.time() - start
                content = data["choices"][0]["message"]["content"].strip()
                non_empty = [line.strip() for line in content.splitlines() if line.strip()]
                if non_empty:
                    first_line = non_empty[0]
                    snippet = (first_line[:120] + "...") if len(first_line) > 120 else first_line
                    print(f"✅ Tier {tier} ({elapsed:.1f}s):\n   {snippet}\n")
                else:
                    print(f"⚠️  Tier {tier} ({elapsed:.1f}s): Returned empty text (Execution failed/rerouted)\n")
        except Exception as exc:
            print(f"❌ Tier {tier} error ({time.time() - start:.1f}s): {exc}\n")

if __name__ == "__main__":
    main()
