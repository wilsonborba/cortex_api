# เอกสารอ้างอิง Cortex API (ข้อกำหนดทางเทคนิค Scalar)

Cortex ให้บริการ REST, WebSocket และ API ที่เข้ากันได้กับ OpenAI เพื่อจัดการและประสานการทำงานของโมเดล AI หลายตัว (Multi-Model Pipeline) จากผู้ให้บริการมากกว่า 14 ราย (เช่น Ollama ในเครื่อง, Groq, Google AI Studio, OpenRouter, Mistral, Cohere, Cloudflare, NVIDIA, SambaNova, HuggingFace เป็นต้น) พร้อมการจัดการโควตาอัตโนมัติ การควบคุมความเร็วในการตอบสนองตามระดับชั้น (Tiers T0 ถึง T5) และการเลือกโมเดลตามประสิทธิภาพจริง

* **URL หลัก:** `http://localhost:8003` (หรือ `http://0.0.0.0:8003`)
* **หน้าจออินเทอร์แอคทีฟ Scalar:** `http://localhost:8003/scalar`
* **โครงสร้าง OpenAPI 3.1 JSON:** `http://localhost:8003/openapi.json`

---

## สารบัญ

1. [ระบบประมวลผลคำสั่ง (`POST /execute`)](#1-ระบบประมวลผลคำสั่ง)
2. [ระบบเชื่อมต่อมาตรฐาน OpenAI (`/v1/chat/completions`, `/v1/models`)](#2-ระบบเชื่อมต่อมาตรฐาน-openai)
3. [แคตตาล็อกโมเดล (`/models`, `/models/sync`, `/models/{model_id}`)](#3-แคตตาล็อกโมเดล)
4. [การติดตามโควตาและโทเค็น (`/quota`, `/quota/{provider}`)](#4-การติดตามโควตาและโทเค็น)
5. [การตั้งค่าระดับชั้นและขอบเขตการทำงาน (`/tiers`, `/tiers/{tier}`)](#5-การตั้งค่าระดับชั้นและขอบเขตการทำงาน)
6. [การปักหมุดเส้นทาง (`/routing/pins`)](#6-การปักหมุดเส้นทาง)
7. [ระบบตรวจวัดและบันทึกสถิติ (`/telemetry/stats`, `/telemetry/events`)](#7-ระบบตรวจวัดและบันทึกสถิติ)
8. [การสตรีมบันทึกการทำงานแบบเรียลไทม์ (`WS /logs/stream`)](#8-การสตรีมบันทึกการทำงานแบบเรียลไทม์)
9. [การประมวลผลวิดีโอหลายมิติ (`/attachments/video`)](#9-การประมวลผลวิดีโอหลายมิติ)

---

## 1. ระบบประมวลผลคำสั่ง

### `POST /execute`
ส่งคำสั่ง (Prompt) เข้าสู่ระบบประมวลผลแบบหลายระดับของ Cortex ระบบจะประเมินความซับซ้อน คัดเลือกโมเดลที่เหมาะสมที่สุดตามคะแนนและโควตา ดำเนินการตามขั้นตอน (สร้างคำตอบ $\rightarrow$ ขัดเกลา $\rightarrow$ วิจารณ์ตรวจสอบ) และดึงข้อมูลเสริมจากเว็บหรือหน่วยความจำตามที่ต้องการ

#### โครงสร้างคำขอ (`application/json`)
| ฟิลด์ | ชนิดข้อมูล | จำเป็น | ค่าเริ่มต้น | คำอธิบาย |
| :--- | :--- | :--- | :--- | :--- |
| `prompt` | string | **ใช่** | — | คำสั่งหรือข้อความหลักที่ต้องการให้ระบบประมวลผล |
| `tier` | integer \| string | ไม่ | `null` | ระดับความพยายาม (`0` ถึง `5`) หรือ `"auto"` เพื่อให้ระบบประเมินอัตโนมัติ |
| `task_type` | string | ไม่ | `"general"` | ประเภทของงาน: `"general"`, `"coding"`, `"reasoning"`, `"creative"`, `"retrieval"` |
| `needs_web` | boolean | ไม่ | `false` | หากตั้งเป็น `true` ระบบจะค้นหาข้อมูลจากเว็บผ่าน SearXNG/DuckDuckGo |
| `use_memory` | boolean | ไม่ | `false` | หากตั้งเป็น `true` ระบบจะค้นหาข้อมูลจากหน่วยความจำเวกเตอร์ Hippocampus |
| `memory_topic` | string | ไม่ | `null` | หัวข้อเฉพาะสำหรับการค้นหาหรือบันทึกในหน่วยความจำ |
| `force_model` | string | ไม่ | `null` | บังคับใช้งานโมเดลที่ระบุโดยตรง (เช่น `groq/llama-3.3-70b-versatile`) |
| `force_provider`| string | ไม่ | `null` | บังคับใช้งานโมเดลใดก็ได้จากผู้ให้บริการที่ระบุ (เช่น `groq`, `openrouter`) |
| `override_strategy` | string | ไม่ | `null` | บังคับใช้งานกลยุทธ์การประมวลผลที่กำหนดไว้ |
| `force_context_format` | string | ไม่ | `null` | บังคับรูปแบบการจัดรูปแบบข้อมูลบริบท: `"toon"` (TOON) หรือ `"json"` |
| `attachments` | array[object] | ไม่ | `[]` | ไฟล์แนบหลายมิติ แต่ละออบเจกต์ประกอบด้วย `{ "filename": str, "mime_type": str, "data_base64": str }` |
| `attachment_job_id` | string (UUID) | ไม่ | `null` | รหัสงานวิดีโอที่ประมวลผลเสร็จแล้ว (`/attachments/video/{id}`) เพื่อดึงข้อความถอดเสียงและภาพสรุป |

#### ตัวอย่างคำขอ
```json
{
  "prompt": "อธิบายความแตกต่างเชิงสถาปัตยกรรมระหว่างโหมด WAL ของ SQLite และแบบเดิม",
  "tier": 3,
  "task_type": "coding",
  "needs_web": false,
  "use_memory": false,
  "force_context_format": "toon"
}
```

#### ตัวอย่างการตอบกลับ (`200 OK`)
```json
{
  "request_id": "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d",
  "tier_requested": 3,
  "tier_executed": 3,
  "strategy_id": "coding_t3_groq_dynamic",
  "task_type": "coding",
  "success": true,
  "response_text": "ใน SQLite โหมด Write-Ahead Logging (WAL) ได้ปรับเปลี่ยนโครงสร้างการบันทึกทรานแซกชันอย่างมีนัยสำคัญ...",
  "steps": [
    {
      "role": "primary",
      "provider": "groq",
      "model_id": "groq/llama-3.3-70b-versatile",
      "success": true,
      "response_text": "ใน SQLite โหมด WAL...",
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

---

## 2. ระบบเชื่อมต่อมาตรฐาน OpenAI

Cortex มีอินเทอร์เฟซในเส้นทาง `/v1` ที่เข้ากันได้กับโปรโตคอลของ OpenAI สำหรับเชื่อมต่อกับเครื่องมือพัฒนา เช่น OpenCode, Claude Code, Cursor, Continue, Roo Code และ SDK ทั่วไป

### `GET /v1/models`
ส่งคืนรายชื่อโมเดลเสมือนที่เชื่อมโยงกับระดับชั้น (Tiers) ของ Cortex

#### การตอบกลับ (`200 OK`)
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
อินเทอร์เฟซมาตรฐานสำหรับ Chat Completions รองรับทั้งแบบซิงโครนัสและแบบสตรีมมิงผ่าน Server-Sent Events (SSE)

---

## 3. แคตตาล็อกโมเดล

### `GET /models`
เรียกดูรายการโมเดลทั้งหมดที่บันทึกไว้ในแคชฐานข้อมูล SQLite ภายในเครื่อง ด้วยความเร็วต่ำกว่า 1 มิลลิวินาที
* พารามิเตอร์การค้นหา: `provider`, `tier`, `status` (`AVAILABLE`, `OFFLINE`, `COOLING_DOWN`, `DISABLED_MANUALLY`, `REQUIRES_SUBSCRIPTION`)

### `POST /models/sync`
เริ่มการตรวจสอบสถานะและค้นหาโมเดลสดจากผู้ให้บริการทั้งหมดที่ตั้งค่าไว้ และอัปเดตฐานข้อมูล

### `GET /models/{model_id}`
ดูข้อมูลและขีดความสามารถของโมเดลที่ระบุ

### `PATCH /models/{model_id}`
ปรับแต่งการตั้งค่าของโมเดล (ระดับชั้นที่อนุญาต, การเปิด/ปิดใช้งาน, รูปแบบ Context Pin)

---

## 4. การติดตามโควตาและโทเค็น

### `GET /quota`
สรุปการใช้งานโทเค็นในหน้าต่างเวลาเลื่อน (Sliding Window), โควตาคงเหลือ และสถานะความพร้อมของผู้ให้บริการทั้งหมด

### `GET /quota/{provider}`
ดูข้อมูลโควตาอย่างละเอียดของผู้ให้บริการรายที่ระบุ

---

## 5. การตั้งค่าระดับชั้นและขอบเขตการทำงาน

### `GET /tiers`
แสดงนโยบายและขอบเขตการทำงานของแต่ละระดับชั้น (**T0** ถึง **T5**)

| ระดับชั้น (Tier) | ชื่อ | ความเร็วเป้าหมาย | กระบวนการหลายโมเดล | การตรวจสอบ/วิจารณ์ | การค้นหาข้อมูล (RAG) |
| :---: | :--- | :---: | :---: | :---: | :---: |
| **T0** | Basic | $\le 5$ วินาที | ไม่ใช้ (โมเดลเดี่ยว) | ไม่ใช้ | ไม่มี |
| **T1** | Light | $\le 10$ วินาที | ไม่ใช้ (โมเดลเดี่ยว) | ไม่ใช้ | แบบง่าย |
| **T2** | Standard | $\le 20$ วินาที | ตามความเหมาะสม (1-2 โมเดล) | ไม่ใช้ | เวกเตอร์ + Rerank |
| **T3** | Advanced | $\le 45$ วินาที | ใช้ (สร้างคำตอบ $\rightarrow$ ขัดเกลา) | ไม่ใช้ | RAG เต็มรูปแบบ |
| **T4** | High | $\le 90$ วินาที | ใช้ (หลายโมเดล) | ตามความเหมาะสม | Deep RAG |
| **T5** | Ultra | $\le 180$ วินาที | ใช้ (สร้าง $\rightarrow$ ขัดเกลา $\rightarrow$ แก้ไข) | บังคับใช้ (ลูปตรวจสอบ) | Deep + Web RAG |

---

## 6. การปักหมุดเส้นทาง

### `GET /routing/pins`
แสดงรายการปักหมุดการทำงานทั้งหมดที่กำลังใช้งาน

### `POST /routing/pins`
ปักหมุดโมเดลหรือกลยุทธ์เฉพาะสำหรับระดับชั้นหรืองานที่กำหนด

### `DELETE /routing/pins?tier=3&task=coding`
ลบการปักหมุดออก (`204 No Content`)

---

## 7. ระบบตรวจวัดและบันทึกสถิติ

### `GET /telemetry/stats`
ดึงข้อมูลสถิติภาพรวม: อัตราความสำเร็จ, ความเร็วเฉลี่ย, โทเค็นที่ใช้, และค่าใช้จ่ายโดยประมาณ

### `GET /telemetry/events`
ดึงบันทึกประวัติการทำงานแต่ละครั้งเพื่อการตรวจสอบ

---

## 8. การสตรีมบันทึกการทำงานแบบเรียลไทม์

### `WS /logs/stream`
เชื่อมต่อผ่าน WebSocket เพื่อรับข้อความบันทึกการทำงาน (Log) แบบสดตามเหตุการณ์ที่เกิดขึ้น

---

## 9. การประมวลผลวิดีโอหลายมิติ

### `POST /attachments/video`
ส่งไฟล์วิดีโอ base64 เพื่อประมวลผลแยกเสียงเป็นข้อความ (ผ่าน Whisper) และวิเคราะห์ภาพเฟรมหลัก (ผ่าน Vision LLM) ในเบื้องหลัง

### `GET /attachments/video/{attachment_id}`
ตรวจสอบสถานะและดึงผลลัพธ์ข้อความถอดเสียงและภาพสรุปจากวิดีโอ
