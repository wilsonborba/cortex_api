from __future__ import annotations

import copy
from typing import Any, Dict, Optional
from fastapi import FastAPI
from fastapi.openapi.utils import get_openapi


I18N_METADATA: Dict[str, Dict[str, Any]] = {
    "en": {
        "title": "Cortex Multi-Model AI Orchestration API",
        "description": (
            "Unified dynamic AI orchestration engine with sliding-window quota tracking, "
            "evidence-based model routing (T0 to T5), OpenAI-compatible facade, multi-modal attachments, "
            "and resilience fallback across 14+ AI cloud providers and local offline engines."
        ),
        "tags": {
            "execute": "Dynamic multi-tier execution engine across providers and pipelines.",
            "models": "Model registry, live provider discovery, status inspection, and configuration.",
            "openai-facade": "OpenAI-compatible /v1/chat/completions and /v1/models protocol facade.",
            "tiers": "Tier envelopes (T0 to T5) policy configuration and latency bounds.",
            "pins": "Routing pins for pinning specific models or strategies to tiers/tasks.",
            "quota": "Sliding-window token quota usage, budget tracking, and cooldown status.",
            "telemetry": "Execution telemetry stats, aggregated metrics, and event audit trail.",
            "logs": "Real-time WebSocket streaming of live system logs.",
            "video": "Async video ingest, multi-modal frame extraction, and transcription jobs.",
            "system": "System runtime profile, hardware capabilities introspection, and feature status.",
        },
        "paths": {
            "/execute": {
                "post": {
                    "summary": "Execute Prompt with Adaptive Routing",
                    "description": (
                        "Routes the prompt to the optimal model based on effort tier (T0-T5), "
                        "evidence scoring, active provider quotas, and context formatting rules. "
                        "Supports video job context and multi-modal attachments."
                    ),
                    "responses": {
                        "200": "Successful inference execution with telemetry and token usage metadata.",
                        "409": "No eligible model available for the requested tier or constraints.",
                        "422": "Validation error or invalid context format/attachment payload.",
                        "500": "Internal execution failure with fallback details.",
                        "501": "Unresolved execution strategy.",
                    },
                }
            },
            "/models": {
                "get": {
                    "summary": "List Available Models",
                    "description": "Returns the registry catalog of supported models with live availability status, provider tags, and assigned tiers.",
                    "responses": {
                        "200": "List of registered models.",
                    },
                    "parameters": {
                        "provider": "Filter models by provider name (e.g. groq, mistral, gemini, ollama).",
                        "tier": "Filter models by assigned effort tier (0 to 5).",
                        "status": "Filter models by access status (e.g. AVAILABLE, OFFLINE, COOLDOWN).",
                    },
                }
            },
            "/models/sync": {
                "post": {
                    "summary": "Trigger Provider Model Discovery",
                    "description": "Queries all configured cloud and local providers in real-time to discover available models and update registry catalog.",
                    "responses": {
                        "200": "Refreshed list of discovered models across all providers.",
                    },
                }
            },
            "/models/{model_id}": {
                "get": {
                    "summary": "Get Model Details",
                    "description": "Retrieves comprehensive metadata, tier eligibility, context limits, and token pricing for a specific model ID.",
                    "responses": {
                        "200": "Model metadata and operational status.",
                        "404": "Model ID not found in registry.",
                    },
                    "parameters": {
                        "model_id": "Unique identifier of the model (e.g. groq/llama-3.3-70b-versatile).",
                    },
                },
                "patch": {
                    "summary": "Update Model Configuration",
                    "description": "Enables/disables a model, modifies assigned tier eligibility, or configures context format pin.",
                    "responses": {
                        "200": "Updated model configuration.",
                        "404": "Model ID not found in registry.",
                        "422": "Invalid configuration payload.",
                    },
                    "parameters": {
                        "model_id": "Unique identifier of the model to update.",
                    },
                },
            },
            "/v1/models": {
                "get": {
                    "summary": "List OpenAI Virtual Models",
                    "description": "Returns OpenAI-compatible model list exposing virtual tier models (cortex-auto, cortex-t0 to cortex-t5).",
                    "responses": {
                        "200": "OpenAI-formatted model list.",
                    },
                }
            },
            "/v1/chat/completions": {
                "post": {
                    "summary": "OpenAI-Compatible Chat Completions",
                    "description": (
                        "Drop-in OpenAI protocol endpoint for IDEs, coding agents, and SDKs. "
                        "Translates virtual models (cortex-t0..t5) into adaptive multi-tier routing with optional SSE streaming."
                    ),
                    "responses": {
                        "200": "Standard OpenAI chat completion response or SSE stream.",
                        "409": "No eligible model available.",
                        "500": "Inference failure.",
                        "501": "Unresolved strategy.",
                    },
                }
            },
            "/tiers": {
                "get": {
                    "summary": "List Tier Policy Envelopes",
                    "description": "Returns operational policy envelopes for all tiers (T0 to T5), including latency bounds and allowed models.",
                    "responses": {
                        "200": "List of tier policy envelopes.",
                    },
                }
            },
            "/tiers/{tier}": {
                "patch": {
                    "summary": "Configure Tier Policy Envelope",
                    "description": "Updates latency bounds, multi-model consensus requirements, and model access lists for a specific tier.",
                    "responses": {
                        "200": "Updated tier policy envelope.",
                        "404": "Tier number not found.",
                    },
                    "parameters": {
                        "tier": "Tier index to configure (0 to 5).",
                    },
                }
            },
            "/routing/pins": {
                "get": {
                    "summary": "List Routing Pins",
                    "description": "Returns explicit model/strategy pins assigned to specific tiers or task types.",
                    "responses": {
                        "200": "List of active routing pins.",
                    },
                },
                "post": {
                    "summary": "Create Routing Pin",
                    "description": "Pins a specific model or strategy ID to a tier and task type, overriding dynamic evidence-based selection.",
                    "responses": {
                        "201": "Routing pin created successfully.",
                    },
                },
                "delete": {
                    "summary": "Remove Routing Pin",
                    "description": "Deletes an existing routing pin to restore autonomous dynamic model selection.",
                    "responses": {
                        "204": "Routing pin removed successfully.",
                        "404": "Matching routing pin not found.",
                    },
                    "parameters": {
                        "tier": "Tier index of the pin to remove.",
                        "task": "Optional task type filter.",
                    },
                },
            },
            "/quota": {
                "get": {
                    "summary": "Get Quota Usage Summary",
                    "description": "Returns sliding-window token usage, request counts, rate limits, and remaining budget across all providers.",
                    "responses": {
                        "200": "Aggregated quota summary by provider.",
                    },
                }
            },
            "/quota/{provider}": {
                "get": {
                    "summary": "Get Provider Quota",
                    "description": "Returns token usage, sliding-window limits, and cooldown status for a specific provider.",
                    "responses": {
                        "200": "Provider quota status.",
                    },
                    "parameters": {
                        "provider": "Provider identifier (e.g. groq, mistral, google).",
                    },
                }
            },
            "/telemetry/stats": {
                "get": {
                    "summary": "Get Telemetry Statistics",
                    "description": "Returns aggregated execution statistics including success rates, average latency, and token throughput.",
                    "responses": {
                        "200": "Aggregated telemetry metrics.",
                    },
                    "parameters": {
                        "tier": "Filter stats by effort tier.",
                        "task": "Filter stats by task type.",
                        "strategy_id": "Filter stats by strategy ID.",
                    },
                }
            },
            "/telemetry/events": {
                "get": {
                    "summary": "List Telemetry Audit Events",
                    "description": "Returns audit logs of recent execution events with routing rationale, model calls, and latency breakdowns.",
                    "responses": {
                        "200": "List of telemetry events.",
                    },
                    "parameters": {
                        "limit": "Maximum number of events to return (default: 50).",
                        "offset": "Pagination offset.",
                        "strategy_id": "Filter events by strategy ID.",
                        "task": "Filter events by task type.",
                        "tier": "Filter events by tier.",
                    },
                }
            },
            "/attachments/video": {
                "post": {
                    "summary": "Submit Video for Processing",
                    "description": "Submits a base64 video payload for asynchronous multi-modal frame extraction and audio transcription.",
                    "responses": {
                        "200": "Video job created with unique tracking ID.",
                        "422": "Invalid base64 payload or unsupported video container.",
                    },
                }
            },
            "/attachments/video/{attachment_id}": {
                "get": {
                    "summary": "Get Video Job Status",
                    "description": "Polls status, structured visual summary, and audio transcript of a submitted video job.",
                    "responses": {
                        "200": "Video job status and processed results.",
                        "404": "Video job ID not found.",
                    },
                    "parameters": {
                        "attachment_id": "Unique video job identifier.",
                    },
                }
            },
            "/system/capabilities": {
                "get": {
                    "summary": "Get System Runtime Capabilities",
                    "description": "Returns installed Cortex profile (Light/Medium/Complete), hardware capabilities, and availability of optional extensions.",
                    "responses": {
                        "200": "Runtime profile and capability matrix.",
                    },
                }
            },
            "/logs/stream": {
                "get": {
                    "summary": "Real-time Log WebSocket Stream",
                    "description": "WebSocket endpoint streaming live structured system logs, routing events, and inference traces in real-time.",
                    "responses": {
                        "101": "WebSocket protocol upgrade successful.",
                    },
                }
            },
        },
    },
    "pt": {
        "title": "API de Orquestração Multi-Modelo Cortex",
        "description": (
            "Motor unificado de orquestração dinâmica de IA com rastreamento de cotas em janela deslizante, "
            "roteamento baseado em evidências (T0 a T5), fachada compatível com OpenAI, suporte a anexos multimodais "
            "e fallback automático de resiliência em mais de 14 provedores de nuvem e motores locais offline."
        ),
        "tags": {
            "execute": "Motor de execução dinâmica multi-nível em provedores e pipelines.",
            "models": "Catálogo de modelos, descoberta de provedores em tempo real, inspeção e configuração.",
            "openai-facade": "Fachada de protocolo compatível com OpenAI em /v1/chat/completions e /v1/models.",
            "tiers": "Configuração de políticas de envelopes de esforço (T0 a T5) e limites de latência.",
            "pins": "Fixações de roteamento para vincular modelos ou estratégias específicas a tiers/tarefas.",
            "quota": "Uso de cota de tokens em janela deslizante, controle de orçamento e status de cooldown.",
            "telemetry": "Estatísticas de telemetria de execução, métricas agregadas e trilha de auditoria.",
            "logs": "Transmissão em tempo real de logs do sistema via WebSocket.",
            "video": "Processamento assíncrono de vídeo, extração multimodal de frames e transcrição.",
            "system": "Perfil de instalação, introspecção de hardware e status de capacidades em runtime.",
        },
        "paths": {
            "/execute": {
                "post": {
                    "summary": "Executar Prompt com Roteamento Adaptativo",
                    "description": (
                        "Roteia o prompt para o modelo ideal com base no nível de esforço (T0-T5), "
                        "pontuação por evidências, cotas ativas dos provedores e regras de formatação de contexto. "
                        "Suporta contexto de jobs de vídeo e anexos multimodais."
                    ),
                    "responses": {
                        "200": "Execução de inferência bem-sucedida com telemetria e metadados de consumo de tokens.",
                        "409": "Nenhum modelo elegível disponível para o tier ou restrições solicitadas.",
                        "422": "Erro de validação ou formato de contexto/anexo inválido.",
                        "500": "Falha interna de execução com detalhes de fallback.",
                        "501": "Estratégia de execução não resolvida.",
                    },
                }
            },
            "/models": {
                "get": {
                    "summary": "Listar Modelos Disponíveis",
                    "description": "Retorna o catálogo de modelos suportados com status de disponibilidade em tempo real, tags de provedor e tiers atribuídos.",
                    "responses": {
                        "200": "Lista de modelos registrados no catálogo.",
                    },
                    "parameters": {
                        "provider": "Filtrar modelos por provedor (ex: groq, mistral, gemini, ollama).",
                        "tier": "Filtrar modelos pelo nível de esforço atribuído (0 a 5).",
                        "status": "Filtrar modelos pelo status de acesso (ex: AVAILABLE, OFFLINE, COOLDOWN).",
                    },
                }
            },
            "/models/sync": {
                "post": {
                    "summary": "Sincronizar e Descobrir Modelos nos Provedores",
                    "description": "Consulta todos os provedores em nuvem e locais em tempo real para descobrir modelos disponíveis e atualizar o catálogo.",
                    "responses": {
                        "200": "Lista atualizada de modelos descobertos em todos os provedores.",
                    },
                }
            },
            "/models/{model_id}": {
                "get": {
                    "summary": "Obter Detalhes do Modelo",
                    "description": "Recupera metadados completos, elegibilidade de tier, limites de contexto e precificação de tokens para um ID de modelo específico.",
                    "responses": {
                        "200": "Metadados do modelo e status operacional.",
                        "404": "ID do modelo não encontrado no catálogo.",
                    },
                    "parameters": {
                        "model_id": "Identificador único do modelo (ex: groq/llama-3.3-70b-versatile).",
                    },
                },
                "patch": {
                    "summary": "Atualizar Configuração do Modelo",
                    "description": "Habilita/desabilita um modelo, altera elegibilidade de tier ou configura fixação de formato de contexto.",
                    "responses": {
                        "200": "Configuração do modelo atualizada com sucesso.",
                        "404": "ID do modelo não encontrado no catálogo.",
                        "422": "Payload de configuração inválido.",
                    },
                    "parameters": {
                        "model_id": "Identificador único do modelo a ser atualizado.",
                    },
                },
            },
            "/v1/models": {
                "get": {
                    "summary": "Listar Modelos Virtuais Padrão OpenAI",
                    "description": "Retorna a lista de modelos no formato OpenAI expondo os modelos virtuais por tier (cortex-auto, cortex-t0 a cortex-t5).",
                    "responses": {
                        "200": "Lista de modelos formatada no padrão OpenAI.",
                    },
                }
            },
            "/v1/chat/completions": {
                "post": {
                    "summary": "Chat Completions Compatível com OpenAI",
                    "description": (
                        "Endpoint de protocolo OpenAI pronto para uso em IDEs, agentes autônomos e SDKs. "
                        "Traduz modelos virtuais (cortex-t0..t5) em roteamento adaptativo multi-nível com suporte a streaming SSE."
                    ),
                    "responses": {
                        "200": "Resposta de chat padrão OpenAI ou fluxo de streaming SSE.",
                        "409": "Nenhum modelo elegível disponível.",
                        "500": "Falha de inferência.",
                        "501": "Estratégia não resolvida.",
                    },
                }
            },
            "/tiers": {
                "get": {
                    "summary": "Listar Políticas de Envelopes de Esforço (Tiers)",
                    "description": "Retorna os envelopes operacionais de todos os tiers (T0 a T5), incluindo limites de latência e modelos permitidos.",
                    "responses": {
                        "200": "Lista de políticas de envelopes de esforço.",
                    },
                }
            },
            "/tiers/{tier}": {
                "patch": {
                    "summary": "Configurar Envelope de Política do Tier",
                    "description": "Atualiza limites de latência, requisitos de consenso multi-modelo e lista de modelos permitidos para um tier específico.",
                    "responses": {
                        "200": "Política do tier atualizada com sucesso.",
                        "404": "Nível de tier não encontrado.",
                    },
                    "parameters": {
                        "tier": "Índice do tier a ser configurado (0 a 5).",
                    },
                }
            },
            "/routing/pins": {
                "get": {
                    "summary": "Listar Fixações de Roteamento (Pins)",
                    "description": "Retorna fixações explícitas de modelos ou estratégias atribuídas a tiers ou tarefas específicas.",
                    "responses": {
                        "200": "Lista de fixações ativas de roteamento.",
                    },
                },
                "post": {
                    "summary": "Criar Fixação de Roteamento (Pin)",
                    "description": "Fixa um modelo ou ID de estratégia específico a um tier e tarefa, sobrepondo a seleção dinâmica automática.",
                    "responses": {
                        "201": "Fixação de roteamento criada com sucesso.",
                    },
                },
                "delete": {
                    "summary": "Remover Fixação de Roteamento (Pin)",
                    "description": "Remove uma fixação existente para restaurar a seleção autônoma e dinâmica de modelos.",
                    "responses": {
                        "204": "Fixação de roteamento removida com sucesso.",
                        "404": "Fixação correspondente não encontrada.",
                    },
                    "parameters": {
                        "tier": "Índice do tier da fixação a remover.",
                        "task": "Filtro opcional por tipo de tarefa.",
                    },
                },
            },
            "/quota": {
                "get": {
                    "summary": "Resumo de Uso de Cotas",
                    "description": "Retorna o consumo de tokens em janela deslizante, total de requisições, limites de taxa e orçamento restante em todos os provedores.",
                    "responses": {
                        "200": "Resumo agregado de cotas por provedor.",
                    },
                }
            },
            "/quota/{provider}": {
                "get": {
                    "summary": "Consultar Cota de um Provedor",
                    "description": "Retorna o uso de tokens, limites em janela deslizante e status de cooldown de um provedor específico.",
                    "responses": {
                        "200": "Status de cota do provedor.",
                    },
                    "parameters": {
                        "provider": "Identificador do provedor (ex: groq, mistral, google).",
                    },
                }
            },
            "/telemetry/stats": {
                "get": {
                    "summary": "Obter Estatísticas de Telemetria",
                    "description": "Retorna métricas agregadas de execução, incluindo taxas de sucesso, latência média e vazão de tokens.",
                    "responses": {
                        "200": "Métricas agregadas de telemetria.",
                    },
                    "parameters": {
                        "tier": "Filtrar estatísticas por nível de tier.",
                        "task": "Filtrar estatísticas por tipo de tarefa.",
                        "strategy_id": "Filtrar estatísticas por ID de estratégia.",
                    },
                }
            },
            "/telemetry/events": {
                "get": {
                    "summary": "Listar Eventos de Auditoria de Telemetria",
                    "description": "Retorna histórico detalhado de execuções com justificativa de roteamento, chamadas a modelos e detalhamento de latência.",
                    "responses": {
                        "200": "Lista de eventos de telemetria.",
                    },
                    "parameters": {
                        "limit": "Número máximo de eventos a retornar (padrão: 50).",
                        "offset": "Deslocamento de paginação.",
                        "strategy_id": "Filtrar eventos por ID de estratégia.",
                        "task": "Filtrar eventos por tipo de tarefa.",
                        "tier": "Filtrar eventos por tier.",
                    },
                }
            },
            "/attachments/video": {
                "post": {
                    "summary": "Enviar Vídeo para Processamento",
                    "description": "Envia um payload de vídeo em base64 para extração assíncrona de frames e transcrição de áudio.",
                    "responses": {
                        "200": "Job de vídeo criado com ID exclusivo de rastreamento.",
                        "422": "Payload base64 inválido ou formato de vídeo não suportado.",
                    },
                }
            },
            "/attachments/video/{attachment_id}": {
                "get": {
                    "summary": "Consultar Status do Processamento de Vídeo",
                    "description": "Consulta o progresso, resumo visual estruturado e transcrição de áudio de um job de vídeo enviado.",
                    "responses": {
                        "200": "Status do job de vídeo e resultados processados.",
                        "404": "ID do job de vídeo não encontrado.",
                    },
                    "parameters": {
                        "attachment_id": "Identificador exclusivo do job de vídeo.",
                    },
                }
            },
            "/system/capabilities": {
                "get": {
                    "summary": "Consultar Capacidades do Sistema em Runtime",
                    "description": "Retorna o perfil instalado do Cortex (Light/Medium/Complete), recursos de hardware detectados e disponibilidade de extensões opcionais.",
                    "responses": {
                        "200": "Perfil em execução e matriz de capacidades.",
                    },
                }
            },
            "/logs/stream": {
                "get": {
                    "summary": "Transmissão de Logs em Tempo Real (WebSocket)",
                    "description": "Endpoint WebSocket para streaming ao vivo de logs estruturados, eventos de roteamento e traces de inferência.",
                    "responses": {
                        "101": "Upgrade de protocolo WebSocket realizado com sucesso.",
                    },
                }
            },
        },
    },
    "th": {
        "title": "Cortex API การจัดการ AI หลายโมเดลแบบรวมศูนย์",
        "description": (
            "ระบบจัดสรรและควบคุม AI อัจฉริยะแบบไดนามิก พร้อมระบบติดตามโควตาโทเค็นแบบหน้าต่างเลื่อน (Sliding-Window), "
            "การกำหนดเส้นทางตามหลักฐาน (T0 ถึง T5), ส่วนต่อประสานมาตรฐาน OpenAI, รองรับไฟล์แนบมัลติโมดอล "
            "และระบบสำรองกู้คืนอัตโนมัติครอบคลุมผู้ให้บริการคลาวด์กว่า 14 รายและโมเดลออฟไลน์ในเครื่อง"
        ),
        "tags": {
            "execute": "กลไกการประมวลผลหลายระดับแบบไดนามิกผ่านผู้ให้บริการและไปป์ไลน์",
            "models": "สารบบโมเดล การค้นพบโมเดลสด การตรวจสอบสถานะ และการกำหนดค่า",
            "openai-facade": "ส่วนต่อประสานมาตรฐาน OpenAI ที่ /v1/chat/completions และ /v1/models",
            "tiers": "การกำหนดค่านโยบายระดับการประมวลผล (T0 ถึง T5) และขีดจำกัดความหน่วง",
            "pins": "การปักหมุดเส้นทางเพื่อกำหนดโมเดลหรือกลยุทธ์เฉพาะให้กับระดับ/ประเภทงาน",
            "quota": "การใช้งานโควตาโทเค็นแบบหน้าต่างเลื่อน การติดตามงบประมาณ และสถานะคูลดาวน์",
            "telemetry": "สถิติการวัดและตรวจสอบการทำงาน ตัวชี้วัดรวม และบันทึกการตรวจสอบ",
            "logs": "การสตรีมบันทึกระบบสดแบบเรียลไทม์ผ่าน WebSocket",
            "video": "การประมวลผลวิดีโอแบบอะซิงโครนัส การดึงเฟรมมัลติโมดอล และการถอดเสียง",
            "system": "โปรไฟล์การติดตั้ง การตรวจจับฮาร์ดแวร์ และสถานะความสามารถของระบบ",
        },
        "paths": {
            "/execute": {
                "post": {
                    "summary": "ประมวลผล Prompt ด้วยการเลือกเส้นทางแบบปรับตัว",
                    "description": (
                        "จัดส่งคำสั่งไปยังโมเดลที่เหมาะสมที่สุดโดยอิงจากระดับความยาก (T0-T5), "
                        "คะแนนหลักฐาน โควตาของผู้ให้บริการ และรูปแบบบริบท รองรับวิดีโอและไฟล์แนบมัลติโมดอล"
                    ),
                    "responses": {
                        "200": "ประมวลผลสำเร็จพร้อมข้อมูลการวัดผลและปริมาณโทเค็นที่ใช้",
                        "409": "ไม่มีโมเดลที่ตรงตามเงื่อนไขในระดับที่ร้องขอ",
                        "422": "ข้อมูลคำขอไม่ถูกต้องหรือรูปแบบบริบทไม่รองรับ",
                        "500": "เกิดข้อผิดพลาดในการประมวลผลภายในระบบ",
                        "501": "ไม่พบกลยุทธ์การประมวลผลที่เหมาะสม",
                    },
                }
            },
            "/models": {
                "get": {
                    "summary": "แสดงรายการโมเดลที่พร้อมใช้งาน",
                    "description": "ส่งคืนสารบบโมเดลทั้งหมดพร้อมสถานะความพร้อมใช้งานสด แท็กผู้ให้บริการ และระดับที่กำหนด",
                    "responses": {
                        "200": "รายการโมเดลที่ลงทะเบียนในระบบ",
                    },
                    "parameters": {
                        "provider": "กรองโมเดลตามผู้ให้บริการ (เช่น groq, mistral, gemini, ollama)",
                        "tier": "กรองโมเดลตามระดับการประมวลผล (0 ถึง 5)",
                        "status": "กรองโมเดลตามสถานะการเข้าถึง (เช่น AVAILABLE, OFFLINE, COOLDOWN)",
                    },
                }
            },
            "/models/sync": {
                "post": {
                    "summary": "ซิงค์และค้นพบโมเดลจากผู้ให้บริการ",
                    "description": "ตรวจสอบผู้ให้บริการคลาวด์และโมเดลในเครื่องแบบเรียลไทม์เพื่ออัปเดตสารบบโมเดลล่าสุด",
                    "responses": {
                        "200": "รายการโมเดลที่อัปเดตใหม่จากผู้ให้บริการทั้งหมด",
                    },
                }
            },
            "/models/{model_id}": {
                "get": {
                    "summary": "ดูรายละเอียดโมเดล",
                    "description": "ดึงข้อมูลเมทาดาทา ระดับที่รองรับ ขีดจำกัดบริบท และราคาโทเค็นของโมเดลที่ระบุ",
                    "responses": {
                        "200": "ข้อมูลรายละเอียดและสถานะการทำงานของโมเดล",
                        "404": "ไม่พบรหัสโมเดลในระบบ",
                    },
                    "parameters": {
                        "model_id": "รหัสเฉพาะของโมเดล (เช่น groq/llama-3.3-70b-versatile)",
                    },
                },
                "patch": {
                    "summary": "อัปเดตการตั้งค่าโมเดล",
                    "description": "เปิด/ปิดการใช้งานโมเดล แก้ไขระดับที่รองรับ หรือกำหนดรูปแบบบริบทเฉพาะ",
                    "responses": {
                        "200": "อัปเดตการตั้งค่าโมเดลสำเร็จ",
                        "404": "ไม่พบรหัสโมเดลในระบบ",
                        "422": "ข้อมูลการตั้งค่าไม่ถูกต้อง",
                    },
                    "parameters": {
                        "model_id": "รหัสเฉพาะของโมเดลที่ต้องการอัปเดต",
                    },
                },
            },
            "/v1/models": {
                "get": {
                    "summary": "แสดงรายการโมเดลเสมือนมาตรฐาน OpenAI",
                    "description": "ส่งคืนรายการโมเดลตามรูปแบบ OpenAI ที่แสดงโมเดลเสมือนตามระดับ (cortex-auto, cortex-t0 ถึง cortex-t5)",
                    "responses": {
                        "200": "รายการโมเดลในรูปแบบมาตรฐาน OpenAI",
                    },
                }
            },
            "/v1/chat/completions": {
                "post": {
                    "summary": "Chat Completions มาตรฐาน OpenAI",
                    "description": (
                        "ส่วนต่อประสานที่เข้ากันได้กับ OpenAI สำหรับใช้งานใน IDE, เอเจนต์อัตโนมัติ และ SDK "
                        "แปลงโมเดลเสมือน (cortex-t0..t5) เป็นการเลือกเส้นทางแบบไดนามิกพร้อมการสตรีมแบบ SSE"
                    ),
                    "responses": {
                        "200": "ผลลัพธ์การสนทนามาตรฐาน OpenAI หรือสตรีม SSE",
                        "409": "ไม่มีโมเดลที่พร้อมใช้งาน",
                        "500": "เกิดข้อผิดพลาดในการประมวลผล",
                        "501": "ไม่พบกลยุทธ์ที่รองรับ",
                    },
                }
            },
            "/tiers": {
                "get": {
                    "summary": "แสดงนโยบายระดับการประมวลผล (Tiers)",
                    "description": "ส่งคืนนโยบายและขอบเขตการทำงานของทุกระดับ (T0 ถึง T5) รวมถึงขีดจำกัดความหน่วงและโมเดลที่อนุญาต",
                    "responses": {
                        "200": "รายการนโยบายระดับการประมวลผล",
                    },
                }
            },
            "/tiers/{tier}": {
                "patch": {
                    "summary": "กำหนดค่านโยบายระดับการประมวลผล",
                    "description": "อัปเดตขีดจำกัดความหน่วง เงื่อนไขการลงมติหลายโมเดล และรายชื่อโมเดลที่อนุญาตสำหรับระดับที่ระบุ",
                    "responses": {
                        "200": "อัปเดตนโยบายระดับการประมวลผลสำเร็จ",
                        "404": "ไม่พบระดับที่ระบุ",
                    },
                    "parameters": {
                        "tier": "หมายเลขระดับที่ต้องการกำหนดค่า (0 ถึง 5)",
                    },
                }
            },
            "/routing/pins": {
                "get": {
                    "summary": "แสดงรายการปักหมุดเส้นทาง",
                    "description": "ส่งคืนการปักหมุดโมเดลหรือกลยุทธ์ที่กำหนดไว้สำหรับระดับหรือประเภทงานเฉพาะ",
                    "responses": {
                        "200": "รายการการปักหมุดเส้นทางที่ใช้งานอยู่",
                    },
                },
                "post": {
                    "summary": "สร้างการปักหมุดเส้นทาง",
                    "description": "ปักหมุดโมเดลหรือกลยุทธ์เฉพาะให้กับระดับและประเภทงาน เพื่อแทนที่การเลือกโมเดลอัตโนมัติ",
                    "responses": {
                        "201": "สร้างการปักหมุดเส้นทางสำเร็จ",
                    },
                },
                "delete": {
                    "summary": "ลบการปักหมุดเส้นทาง",
                    "description": "ลบการปักหมุดเพื่อคืนค่าการเลือกโมเดลแบบไดนามิกอัตโนมัติ",
                    "responses": {
                        "204": "ลบการปักหมุดสำเร็จ",
                        "404": "ไม่พบการปักหมุดที่ตรงกับเงื่อนไข",
                    },
                    "parameters": {
                        "tier": "หมายเลขระดับของการปักหมุดที่ต้องการลบ",
                        "task": "ประเภทงาน (ระบุหรือไม่ระบุก็ได้)",
                    },
                },
            },
            "/quota": {
                "get": {
                    "summary": "สรุปการใช้งานโควตา",
                    "description": "ส่งคืนสรุปการใช้โทเค็นแบบหน้าต่างเลื่อน จำนวนคำขอ ขีดจำกัดอัตรา และงบประมาณคงเหลือของผู้ให้บริการทั้งหมด",
                    "responses": {
                        "200": "ข้อมูลสรุปโควตาตามผู้ให้บริการ",
                    },
                }
            },
            "/quota/{provider}": {
                "get": {
                    "summary": "ตรวจสอบโควตาของผู้ให้บริการ",
                    "description": "ส่งคืนการใช้งานโทเค็น ขีดจำกัด และสถานะคูลดาวน์ของผู้ให้บริการที่ระบุ",
                    "responses": {
                        "200": "สถานะโควตาของผู้ให้บริการ",
                    },
                    "parameters": {
                        "provider": "ชื่อผู้ให้บริการ (เช่น groq, mistral, google)",
                    },
                }
            },
            "/telemetry/stats": {
                "get": {
                    "summary": "ดูสถิติการวัดผลการทำงาน",
                    "description": "ส่งคืนสถิติรวม เช่น อัตราความสำเร็จ ความหน่วงเฉลี่ย และปริมาณโทเค็นที่ประมวลผล",
                    "responses": {
                        "200": "สถิติการวัดผลรวม",
                    },
                    "parameters": {
                        "tier": "กรองสถิติตามระดับ",
                        "task": "กรองสถิติตามประเภทงาน",
                        "strategy_id": "กรองสถิติตามรหัสกลยุทธ์",
                    },
                }
            },
            "/telemetry/events": {
                "get": {
                    "summary": "แสดงรายการบันทึกเหตุการณ์การทำงาน",
                    "description": "ส่งคืนประวัติการประมวลผลล่าสุดพร้อมเหตุผลการเลือกเส้นทาง การเรียกใช้โมเดล และรายละเอียดความหน่วง",
                    "responses": {
                        "200": "รายการเหตุการณ์การทำงาน",
                    },
                    "parameters": {
                        "limit": "จำนวนเหตุการณ์สูงสุดที่ต้องการแสดง (ค่าเริ่มต้น: 50)",
                        "offset": "ตำแหน่งเริ่มต้นข้อมูล",
                        "strategy_id": "กรองเหตุการณ์ตามรหัสกลยุทธ์",
                        "task": "กรองเหตุการณ์ตามประเภทงาน",
                        "tier": "กรองเหตุการณ์ตามระดับ",
                    },
                }
            },
            "/attachments/video": {
                "post": {
                    "summary": "ส่งวิดีโอเพื่อประมวลผล",
                    "description": "ส่งข้อมูลวิดีโอในรูปแบบ base64 เพื่อดึงภาพเฟรมและถอดเสียงแบบอะซิงโครนัส",
                    "responses": {
                        "200": "สร้างงานประมวลผลวิดีโอสำเร็จพร้อมรหัสติดตาม",
                        "422": "ข้อมูล base64 ไม่ถูกต้องหรือไม่รองรับรูปแบบวิดีโอ",
                    },
                }
            },
            "/attachments/video/{attachment_id}": {
                "get": {
                    "summary": "ตรวจสอบสถานะงานประมวลผลวิดีโอ",
                    "description": "ตรวจสอบสถานะ สรุปภาพรวม และข้อความถอดเสียงของงานวิดีโอที่ส่งเข้ามา",
                    "responses": {
                        "200": "สถานะและผลลัพธ์ของงานวิดีโอ",
                        "404": "ไม่พบรหัสงานวิดีโอในระบบ",
                    },
                    "parameters": {
                        "attachment_id": "รหัสเฉพาะของงานวิดีโอ",
                    },
                }
            },
            "/system/capabilities": {
                "get": {
                    "summary": "ดูความสามารถของระบบในขณะทำงาน",
                    "description": "ส่งคืนโปรไฟล์ Cortex ที่ติดตั้ง (Light/Medium/Complete) ความสามารถของฮาร์ดแวร์ และสถานะส่วนขยายเสริม",
                    "responses": {
                        "200": "โปรไฟล์และตารางความสามารถของระบบ",
                    },
                }
            },
            "/logs/stream": {
                "get": {
                    "summary": "สตรีมบันทึกระบบแบบเรียลไทม์ (WebSocket)",
                    "description": "จุดเชื่อมต่อ WebSocket สำหรับรับข้อมูลบันทึกระบบ เหตุการณ์การกำหนดเส้นทาง และร่องรอยการประมวลผลแบบสด",
                    "responses": {
                        "101": "อัปเกรดโปรโตคอล WebSocket สำเร็จ",
                    },
                }
            },
        },
    },
}


def get_localized_openapi(app: FastAPI, lang: str = "en") -> Dict[str, Any]:
    lang = lang.lower() if lang else "en"
    if lang not in I18N_METADATA:
        lang = "en"

    # Base openapi schema from FastAPI
    base_schema = get_openapi(
        title=app.title,
        version=app.version,
        openapi_version=app.openapi_version,
        description=app.description,
        routes=app.routes,
        tags=app.openapi_tags,
    )
    schema = copy.deepcopy(base_schema)
    meta = I18N_METADATA[lang]

    # Translate info
    if "info" in schema:
        schema["info"]["title"] = meta["title"]
        schema["info"]["description"] = meta["description"]

    # Translate tags
    if "tags" in schema:
        for tag in schema["tags"]:
            tag_name = tag.get("name")
            if tag_name in meta["tags"]:
                tag["description"] = meta["tags"][tag_name]

    # Translate paths and operations
    paths = schema.get("paths", {})
    meta_paths = meta.get("paths", {})

    for path_key, path_item in paths.items():
        if path_key in meta_paths:
            m_path = meta_paths[path_key]
            for method, op_item in path_item.items():
                if method in m_path:
                    m_op = m_path[method]
                    if "summary" in m_op:
                        op_item["summary"] = m_op["summary"]
                    if "description" in m_op:
                        op_item["description"] = m_op["description"]

                    # Responses
                    if "responses" in m_op and "responses" in op_item:
                        for status_code, desc in m_op["responses"].items():
                            if status_code in op_item["responses"]:
                                op_item["responses"][status_code]["description"] = desc

                    # Parameters
                    if "parameters" in m_op and "parameters" in op_item:
                        for param in op_item["parameters"]:
                            p_name = param.get("name")
                            if p_name in m_op["parameters"]:
                                param["description"] = m_op["parameters"][p_name]

    SCHEMA_PROP_TRANSLATIONS: Dict[str, Dict[str, str]] = {
        "pt": {
            "prompt": "Texto de entrada ou instrução para o modelo",
            "tier": "Nível de esforço desejado (0 a 5)",
            "task_type": "Tipo de tarefa (ex: code, general, reasoning)",
            "needs_web": "Se a inferência requer pesquisa na web em tempo real",
            "use_memory": "Se deve injetar contexto de memória RAG",
            "memory_topic": "Tópico específico para busca de memória RAG",
            "force_model": "Forçar uso de um ID de modelo específico",
            "force_provider": "Forçar uso de um provedor específico",
            "override_strategy": "Sobrescrever estratégia de execução",
            "force_context_format": "Forçar formato de contexto (ex: standard, compressed)",
            "attachments": "Lista de arquivos ou anexos multimodais",
            "attachment_job_id": "ID do job de vídeo processado anteriormente",
            "model": "Identificador do modelo ou modelo virtual (ex: cortex-auto, cortex-t1)",
            "messages": "Lista de mensagens da conversa no formato OpenAI",
            "stream": "Se a resposta deve ser transmitida em streaming SSE",
            "temperature": "Temperatura de amostragem da inferência",
            "max_tokens": "Limite máximo de tokens gerados",
            "filename": "Nome do arquivo do anexo",
            "mime_type": "Tipo MIME do anexo (ex: image/png, audio/wav, video/mp4)",
            "data_base64": "Conteúdo codificado em base64",
        },
        "th": {
            "prompt": "ข้อความคำสั่งหรือเนื้อหาที่ต้องการประมวลผล",
            "tier": "ระดับความยากและความสามารถที่ต้องการ (0 ถึง 5)",
            "task_type": "ประเภทของงาน (เช่น code, general, reasoning)",
            "needs_web": "ต้องการให้ค้นหาข้อมูลบนเว็บแบบเรียลไทม์หรือไม่",
            "use_memory": "เปิดใช้งานความจำ RAG หรือไม่",
            "memory_topic": "หัวข้อสำหรับค้นหาในหน่วยความจำ RAG",
            "force_model": "บังคับใช้รหัสโมเดลที่ระบุ",
            "force_provider": "บังคับใช้ผู้ให้บริการที่ระบุ",
            "override_strategy": "กำหนดกลยุทธ์การทำงานด้วยตนเอง",
            "force_context_format": "กำหนดรูปแบบบริบท (เช่น standard, compressed)",
            "attachments": "รายการไฟล์แนบมัลติโมดอล",
            "attachment_job_id": "รหัสงานวิดีโอที่ประมวลผลก่อนหน้านี้",
            "model": "รหัสโมเดลหรือโมเดลเสมือน (เช่น cortex-auto, cortex-t1)",
            "messages": "รายการข้อความการสนทนาตามรูปแบบ OpenAI",
            "stream": "เปิดใช้งานการสตรีมคำตอบแบบ SSE หรือไม่",
            "temperature": "ระดับความสร้างสรรค์ของคำตอบ",
            "max_tokens": "จำนวนโทเค็นสูงสุดที่สร้างขึ้น",
            "filename": "ชื่อไฟล์ของไฟล์แนบ",
            "mime_type": "ประเภท MIME ของไฟล์แนบ (เช่น image/png, audio/wav, video/mp4)",
            "data_base64": "ข้อมูลไฟล์ที่เข้ารหัสในรูปแบบ base64",
        },
    }

    if lang in SCHEMA_PROP_TRANSLATIONS:
        prop_map = SCHEMA_PROP_TRANSLATIONS[lang]
        schemas = schema.get("components", {}).get("schemas", {})
        for s_name, s_def in schemas.items():
            props = s_def.get("properties", {})
            for p_name, p_def in props.items():
                if p_name in prop_map:
                    p_def["description"] = prop_map[p_name]

    return schema
