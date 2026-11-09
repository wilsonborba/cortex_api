# Local Ollama routing and capacity policy

## Roles

The host maintains two local models but only one may remain loaded at a time:

| Work | Cortex registry model | Required capability |
| --- | --- | --- |
| Tier-0 text and transcribed audio | `ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0` | `vision=false` |
| Image attachments and video-frame descriptions | `ollama/qwen2.5vl:7b` | `vision=true` |

The Router enforces this before caller overrides, pins and dynamic scoring.
An image can never be sent to the Heretic model, whose Ollama build is run as
text-only. A normal Tier-0 text request cannot load the vision model merely
because it is another local candidate. Tiers 1–5 retain their existing policy.

## Ollama capacity

`deploy/ollama/cortex-queue-limits.conf` configures Ollama to keep one model
loaded, execute one request at a time, queue at most four waiting requests,
and unload an idle model after two minutes. Install it with:

```bash
sudo ./scripts/configure_ollama_queue_limits.sh
```

The fifth concurrent request is rejected rather than causing GPU-memory
pressure. Cortex returns that service-unavailable result; it must not silently
substitute another model.

## Register and verify the roles

After the Cortex API is running, sync the local catalog and explicitly set
capabilities. The settings persist through later discovery syncs.

```bash
curl -X POST http://127.0.0.1:8003/models/sync

curl -X PATCH \
  'http://127.0.0.1:8003/models/ollama%2Fhf.co%2FThalisAI%2FQwen3-VL-8B-Instruct-heretic%3AQ8_0' \
  -H 'content-type: application/json' \
  -d '{"tier_eligibility":[0],"is_vision_capable":false}'

curl -X PATCH \
  'http://127.0.0.1:8003/models/ollama%2Fqwen2.5vl%3A7b' \
  -H 'content-type: application/json' \
  -d '{"tier_eligibility":[0],"is_vision_capable":true}'
```

Use `GET /models` to verify the entries before accepting user traffic.
