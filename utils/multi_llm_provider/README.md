## Dev/test environment: multi-provider token extraction

Not a configuration example — see [`../../examples`](../../examples) for those
(e.g. [`ratelimit_check_report`](../../examples/ratelimit_check_report), which
this reuses the `store` + `grpc` check/report pattern from). This is disposable
test scaffolding, alongside the kind-based [`local-setup`](../../README.md#running-local-development-environment-kind)
under `../`, for exercising wasm-shim's `responseBodyJSON` behavior against
response shapes no real local backend can produce.

### Description

This environment runs inference-shaped traffic for multiple LLM providers
past wasm-shim. Provider is selected per request via the `Host` header, which
routes to one of two mock backends:

| Provider | Host | Backend | Streaming |
|---|---|---|---|
| OpenAI | `openai.127.0.0.1.nip.io` | [`llm-d-inference-sim`](https://github.com/llm-d/llm-d-inference-sim) | yes |
| Anthropic | `anthropic.127.0.0.1.nip.io` | [MockServer](https://www.mock-server.com/) | yes |
| Gemini | `gemini.127.0.0.1.nip.io` | MockServer | yes |
| `ambiguous` (synthetic) | `ambiguous.127.0.0.1.nip.io` | MockServer | no (JSON only) |
| `unknown` (synthetic) | `unknown.127.0.0.1.nip.io` | MockServer | no (JSON only) |

The `store` action here is:

```
responseBodyJSON(["/usage/total_tokens", "/usageMetadata/totalTokenCount", "/usage/output_tokens"], "number")
```

i.e. OpenAI shape first, Gemini shape second, and Anthropic's
`output_tokens`-only interim workaround last (Anthropic has no native total,
so this is an intentional under-count — see RFC 0024's rationale).

### Running

Requires the Wasm module built at `target/wasm32-wasip1/debug/wasm_shim.wasm`
(`make build` at the repo root).

```sh
make run
```

### Non-streaming

```sh
# OpenAI shape — real tokenization via llm-d-inference-sim, resolves via the 1st candidate
curl "http://openai.127.0.0.1.nip.io:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -d '{
    "model": "mock",
    "messages": [
      { "role": "user", "content": "Tell me a three sentence bedtime story about a unicorn." }
    ]
  }'

# Gemini shape — 1st candidate absent, resolves via the 2nd
curl "http://gemini.127.0.0.1.nip.io:18000/v1beta/models/mock:generateContent" \
  -H "Content-Type: application/json" -d '{}'

# Anthropic shape — 1st and 2nd absent, resolves via the 3rd (output_tokens only)
curl "http://anthropic.127.0.0.1.nip.io:18000/v1/messages" \
  -H "Content-Type: application/json" -d '{}'

# First candidate present but not a number — the `number` type hint skips it
curl "http://ambiguous.127.0.0.1.nip.io:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -d '{}'

# None of the candidates present — fail-open, request still succeeds, nothing counted
curl "http://unknown.127.0.0.1.nip.io:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -d '{}'
```

Each should return `200 OK`. Check the resolved `hits_addend`:

```sh
docker compose logs limitador | grep hits_addend
```

Expected values: `gemini` → 18, `anthropic` → 8 (the documented under-count),
`ambiguous` → 33 (falls through the non-numeric first candidate to the
numeric second one). `openai` varies — `llm-d-inference-sim` genuinely
tokenizes the request/response text rather than returning a fixed count.
`unknown` produces **no** `Report` call at all — the store action fails to
resolve anything, so the downstream `grpc` report action, which depends on
that value, never runs.

For the `unknown` case, also check:

```sh
docker compose logs envoy | grep -i "no candidate resolved"
curl -s http://127.0.0.1:18001/stats/prometheus | grep body_extraction_misses
```

You should see a `warn`-level log line and the counter incrementing by 1 per
miss.

### Streaming

Each provider signals streaming its own way, matching the real APIs:

- **anthropic**: same path (`/v1/messages`), add `-d '{"stream": true}'`.
- **gemini**: a different path/method entirely — `:streamGenerateContent`
  instead of `:generateContent` — no body flag involved.
- **openai**: same path, `-d '{"stream": true}'` plus
  `stream_options.include_usage` (see below).

The `ambiguous`/`unknown` fixtures are JSON-only. For example:

```sh
curl -N "http://anthropic.127.0.0.1.nip.io:18000/v1/messages" \
  -H "Content-Type: application/json" -d '{"stream": true}'

curl -N "http://gemini.127.0.0.1.nip.io:18000/v1beta/models/mock:streamGenerateContent" \
  -H "Content-Type: application/json" -d '{}'
```

For `openai`, `stream_options.include_usage` must also be set, matching the
real Chat Completions API's opt-in for a usage chunk on streaming responses:

```sh
curl -N "http://openai.127.0.0.1.nip.io:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -d '{
    "model": "mock",
    "stream": true,
    "stream_options": { "include_usage": true },
    "messages": [
      { "role": "user", "content": "Tell me a three sentence bedtime story about a unicorn." }
    ]
  }'
```

`SseBodyParser` scans every SSE event as it arrives for the candidates'
leaf keys, so a candidate resolves as soon as a matching event is seen —
regardless of where in the stream (or how many events) that turns out to be.
Expected outcomes:

- **openai**: resolves correctly, `hits_addend` varies with input —
  `llm-d-inference-sim` emits a genuine final usage chunk.
- **anthropic**: resolves correctly (`hits_addend: 8`, the documented
  under-count via `output_tokens`) — `message_delta` is matched wherever it
  appears in the stream, not by relying on event position.
- **gemini**: resolves correctly (`hits_addend: 18`) — `usageMetadata` is
  picked up from the true final chunk as soon as it's fed to the parser, no
  longer requiring it to coincide with a fixed offset from the end.

### Inspecting traffic

```sh
docker compose logs -f mock-llm
docker compose logs -f llm-d-inference-sim
docker compose logs -f envoy
docker compose logs -f limitador
```

### Clean up

```sh
make clean
```
