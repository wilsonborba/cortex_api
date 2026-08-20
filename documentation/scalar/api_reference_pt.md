# Referência da API Cortex (Especificação Técnica Scalar)

O Cortex fornece uma API unificada REST, WebSocket e compatível com OpenAI para orquestração de pipelines multi-modelo entre mais de 14 provedores de IA (Ollama local, Groq, Google AI Studio, OpenRouter, Mistral, Cohere, Cloudflare, NVIDIA, SambaNova, HuggingFace, etc.), com controle automático de quota, envelopes de latência (Tiers T0 a T5) e roteamento baseado em evidências.

* **URL Base:** `http://localhost:8003` (ou `http://0.0.0.0:8003`)
* **Interface Interativa Scalar:** `http://localhost:8003/scalar`
* **Esquema OpenAPI 3.1 JSON:** `http://localhost:8003/openapi.json`

---

## Índice

1. [Motor de Execução (`POST /execute`)](#1-motor-de-execução)
2. [Fachada Compatível com OpenAI (`/v1/chat/completions`, `/v1/models`)](#2-fachada-compatível-com-openai)
3. [Catálogo de Modelos (`/models`, `/models/sync`, `/models/{model_id}`)](#3-catálogo-de-modelos)
4. [Rastreamento de Quota e Tokens (`/quota`, `/quota/{provider}`)](#4-rastreamento-de-quota-e-tokens)
5. [Envelopes de Execução e Tiers (`/tiers`, `/tiers/{tier}`)](#5-envelopes-de-execução-e-tiers)
6. [Pinos de Roteamento (`/routing/pins`)](#6-pinos-de-roteamento)
7. [Telemetria e Auditoria (`/telemetry/stats`, `/telemetry/events`)](#7-telemetria-e-auditoria)
8. [Transmissão de Logs ao Vivo (`WS /logs/stream`)](#8-transmissão-de-logs-ao-vivo)
9. [Ingestão Multimodal de Vídeo (`/attachments/video`)](#9-ingestão-multimodal-de-vídeo)

---

## 1. Motor de Execução

### `POST /execute`
Executa um prompt através do motor de orquestração multi-tier do Cortex. Avalia a complexidade da tarefa, seleciona os modelos ideais com base em pontuações dinâmicas e quotas, executa pipelines de múltiplos estágios (Primário $\rightarrow$ Refinador $\rightarrow$ Crítico) e recupera contexto web/memória se necessário.

#### Corpo da Requisição (`application/json`)
| Campo | Tipo | Obrigatório | Padrão | Descrição |
| :--- | :--- | :--- | :--- | :--- |
| `prompt` | string | **Sim** | — | Instrução ou prompt principal a ser executado. |
| `tier` | integer \| string | Não | `null` | Tier explícito (`0` a `5`) ou `"auto"` para classificação automática de complexidade. |
| `task_type` | string | Não | `"general"` | Categoria da tarefa: `"general"`, `"coding"`, `"reasoning"`, `"creative"`, `"retrieval"`. |
| `needs_web` | boolean | Não | `false` | Quando `true`, consulta SearXNG/DuckDuckGo e injeta resultados da web. |
| `use_memory` | boolean | Não | `false` | Quando `true`, pesquisa na memória vetorial do Hippocampus por tópicos relevantes. |
| `memory_topic` | string | Não | `null` | Tópico específico para busca/gravação na memória do Hippocampus. |
| `force_model` | string | Não | `null` | Override de Camada 3: Força a execução por um modelo específico (ex: `groq/llama-3.3-70b-versatile`). |
| `force_provider`| string | Não | `null` | Override de Camada 3: Força o uso de qualquer modelo disponível do provedor informado. |
| `override_strategy` | string | Não | `null` | Override de Camada 3: Especifica uma estratégia ou pipeline multi-modelo personalizado. |
| `force_context_format` | string | Não | `null` | Força o formato de serialização de contexto RAG: `"toon"` (TOON) ou `"json"`. |
| `attachments` | array[object] | Não | `[]` | Anexos multimodais. Cada objeto possui `{ "filename": str, "mime_type": str, "data_base64": str }`. |
| `attachment_job_id` | string (UUID) | Não | `null` | ID de um job de vídeo concluído (`/attachments/video/{id}`) para injetar transcrição e resumo visual. |

#### Exemplo de Requisição
```json
{
  "prompt": "Explique as diferenças arquiteturais entre o modo WAL do SQLite e o journal tradicional.",
  "tier": 3,
  "task_type": "coding",
  "needs_web": false,
  "use_memory": false,
  "force_context_format": "toon"
}
```

#### Resposta (`200 OK`)
```json
{
  "request_id": "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d",
  "tier_requested": 3,
  "tier_executed": 3,
  "strategy_id": "coding_t3_groq_dynamic",
  "task_type": "coding",
  "success": true,
  "response_text": "No SQLite, o modo Write-Ahead Logging (WAL) altera fundamentalmente a forma como as transações são gravadas...",
  "steps": [
    {
      "role": "primary",
      "provider": "groq",
      "model_id": "groq/llama-3.3-70b-versatile",
      "success": true,
      "response_text": "No SQLite, o modo WAL...",
      "input_tokens": 420,
      "output_tokens": 850,
      "latency_ms": 1120,
      "cost_usd": 0.0,
      "error_type": null,
      "error_message": null,
      "attempts": 1
    }
  ],
  "total_input_tokens": 420,
  "total_output_tokens": 850,
  "total_cost_usd": 0.0,
  "latency_ms": 1120,
  "error_type": null
}
```

#### Códigos de Status e Tratamento de Erros
* `200 OK`: Execução bem-sucedida.
* `409 Conflict` (`NoEligibleModelError`): Nenhum modelo com status `AVAILABLE` atende aos requisitos do Tier ou provedor forçado.
* `422 Unprocessable Entity`: Formato de anexo base64 inválido, codificador desconhecido ou job de vídeo inacabado/com falha.
* `500 Internal Server Error`: Erro interno no servidor.

---

## 2. Fachada Compatível com OpenAI

O Cortex disponibiliza uma rota `/v1` compatível com o protocolo OpenAI para integração direta com ferramentas de desenvolvimento (OpenCode, Claude Code, Cursor, Continue, Roo Code, Aider) e SDKs oficiais.

### `GET /v1/models`
Retorna os identificadores de modelos virtuais mapeados para os Tiers de esforço do Cortex.

#### Resposta (`200 OK`)
```json
{
  "object": "list",
  "data": [
    { "id": "cortex-auto", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t0", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t1", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t2", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t3", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t4", "object": "model", "created": 1740000000, "owned_by": "cortex" },
    { "id": "cortex-t5", "object": "model", "created": 1740000000, "owned_by": "cortex" }
  ]
}
```

### `POST /v1/chat/completions`
Interface padrão de chat completions suportando modo síncrono e streaming via Server-Sent Events (SSE).

#### Exemplo de Requisição
```json
{
  "model": "cortex-t3",
  "messages": [
    { "role": "system", "content": "Você é um especialista em arquitetura de software." },
    { "role": "user", "content": "Refatore esta consulta SQL para obter melhor desempenho." }
  ],
  "stream": false
}
```

---

## 3. Catálogo de Modelos

### `GET /models`
Lista todos os modelos catalogados no banco de dados local SQLite com latência de resposta sub-milissegundo.
* **Parâmetros de Consulta:**
  * `provider` (string, opcional): Filtrar por provedor (`groq`, `ollama`, `openrouter`, etc.).
  * `tier` (int, opcional): Filtrar por elegibilidade de Tier (`0` a `5`).
  * `status` (string, opcional): Filtrar por status (`AVAILABLE`, `OFFLINE`, `COOLING_DOWN`, `DISABLED_MANUALLY`, `REQUIRES_SUBSCRIPTION`).

### `POST /models/sync`
Realiza varredura ativa (*Live Probe*) em todos os adaptadores de provedores configurados e atualiza o catálogo persistido.

### `GET /models/{model_id}`
Consulta os detalhes e capacidades de um modelo específico (ex: `/models/groq/llama-3.3-70b-versatile`).

### `PATCH /models/{model_id}`
Altera configurações persistentes do modelo (Tiers permitidos, ativação/desativação e pin de formato de contexto).

---

## 4. Rastreamento de Quota e Tokens

### `GET /quota`
Retorna o resumo de consumo de tokens na janela deslizante, saldo restante e status operacional de cada provedor.

### `GET /quota/{provider}`
Retorna os dados de quota detalhados de um provedor específico.

---

## 5. Envelopes de Execução e Tiers

### `GET /tiers`
Lista as políticas e envelopes configurados para todos os Tiers (**T0** a **T5**).

| Tier | Nome | Latência Máx | Multi-Modelo / Pipeline | Verificação / Crítica | Contexto RAG |
| :---: | :--- | :---: | :---: | :---: | :---: |
| **T0** | Basic | $\le 5$s | Não (Único modelo) | Não | Nenhum |
| **T1** | Light | $\le 10$s | Não (Único modelo) | Não | Simples |
| **T2** | Standard | $\le 20$s | Opcional (1-2 modelos) | Não | Vetorial + Rerank |
| **T3** | Advanced | $\le 45$s | Sim (Gerador $\rightarrow$ Refinador) | Não | Completo |
| **T4** | High | $\le 90$s | Sim (Multi-modelo) | Opcional | Profundo |
| **T5** | Ultra | $\le 180$s | Sim (Gerador $\rightarrow$ Refinador $\rightarrow$ Revisor) | Obrigatório (Loop Crítico) | Profundo + Web |

### `PATCH /tiers/{tier}`
Atualiza os limites e parâmetros de política de um Tier (latência máxima, modelos permitidos, etc.).

---

## 6. Pinos de Roteamento

### `GET /routing/pins`
Lista todos os pinos de roteamento persistentes ativos.

### `POST /routing/pins`
Fixa um modelo ou estratégia para um determinado Tier ou tipo de tarefa.

### `DELETE /routing/pins?tier=3&task=coding`
Remove o pino de roteamento configurado (`204 No Content`).

---

## 7. Telemetria e Auditoria

### `GET /telemetry/stats`
Retorna estatísticas agregadas de desempenho, latência média, total de tokens gastos e taxa de sucesso por Tier ou tarefa.

### `GET /telemetry/events`
Retorna os eventos detalhados de cada execução realizada para auditoria e observabilidade.

---

## 8. Transmissão de Logs ao Vivo

### `WS /logs/stream`
Conecta via WebSocket e transmite as linhas de log do sistema em tempo real à medida que são registradas.

---

## 9. Ingestão Multimodal de Vídeo

### `POST /attachments/video`
Envia um arquivo de vídeo codificado em base64 para processamento assíncrono em segundo plano (extração de áudio via Whisper + keyframes via Vision LLM).

### `GET /attachments/video/{attachment_id}`
Consulta o status e recupera o resumo visual e a transcrição do vídeo processado.
