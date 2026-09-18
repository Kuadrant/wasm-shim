## Dev/test environment: multi-provider token extraction

Not a configuration example — see [`../../examples`](../../examples) for those
(e.g. [`ratelimit_check_report`](../../examples/ratelimit_check_report), which
this reuses the `store` + `grpc` check/report pattern from). This is disposable
test scaffolding, alongside the kind-based [`local-setup`](../../README.md#running-local-development-environment-kind)
under `../`, for exercising wasm-shim's `responseBodyJSON` behavior against
response shapes no real local backend can produce.

### Description

The upstream here is a small mock server (`mock/server.py`) instead of a real
LLM, capable of returning OpenAI-, Anthropic-, or Gemini-*shaped* responses
(chosen per request via an `X-Mock-Provider` header), in both plain JSON and
real chunked-transfer SSE streaming.

There's no way to run the actual Anthropic or Google Gemini models locally —
both are closed, hosted-only APIs with no downloadable weights — and
`llm-d-inference-sim` (used in
[`../../examples/ratelimit_check_report`](../../examples/ratelimit_check_report))
only ever produces OpenAI-shaped output. This exists specifically to exercise
the [ordered JSON Pointer candidate list](../../README.md#responsebodyjsonjson_pointer--json_pointer-type)
form of `responseBodyJSON` against the various shapes it was built for,
without needing real inference.

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
# OpenAI shape — resolves via the 1st candidate
curl --resolve trlp.example.com:18000:127.0.0.1 "http://trlp.example.com:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -H "X-Mock-Provider: openai" -d '{}'

# Gemini shape — 1st candidate absent, resolves via the 2nd
curl --resolve trlp.example.com:18000:127.0.0.1 "http://trlp.example.com:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -H "X-Mock-Provider: gemini" -d '{}'

# Anthropic shape — 1st and 2nd absent, resolves via the 3rd (output_tokens only)
curl --resolve trlp.example.com:18000:127.0.0.1 "http://trlp.example.com:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -H "X-Mock-Provider: anthropic" -d '{}'

# First candidate present but not a number — the `number` type hint skips it
curl --resolve trlp.example.com:18000:127.0.0.1 "http://trlp.example.com:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -H "X-Mock-Provider: ambiguous" -d '{}'

# None of the candidates present — fail-open, request still succeeds, nothing counted
curl --resolve trlp.example.com:18000:127.0.0.1 "http://trlp.example.com:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -H "X-Mock-Provider: unknown" -d '{}'
```

Each should return `200 OK`. Check the resolved `hits_addend`:

```sh
docker compose logs limitador | grep hits_addend
```

Expected values: `openai` → 18, `gemini` → 18, `anthropic` → 8 (the
documented under-count), `ambiguous` → 33 (falls through the non-numeric
first candidate to the numeric second one). `unknown` produces **no**
`Report` call at all — the store action fails to resolve anything, so the
downstream `grpc` report action, which depends on that value, never runs.

For the `unknown` case, also check:

```sh
docker compose logs envoy | grep -i "no candidate resolved"
curl -s http://127.0.0.1:18001/stats/prometheus | grep body_extraction_misses
```

You should see a `warn`-level log line and the counter incrementing by 1 per
miss.

### Streaming

Add `-d '{"stream": true}'` to any of the `openai`/`anthropic`/`gemini`
curls above (the `ambiguous`/`unknown` fixtures are JSON-only). For example:

```sh
curl --resolve trlp.example.com:18000:127.0.0.1 "http://trlp.example.com:18000/v1/chat/completions" \
  -H "Content-Type: application/json" -H "X-Mock-Provider: gemini" -d '{"stream": true}'
```

`SseBodyParser` currently still uses a "look at the second-to-last SSE event"
heuristic (replacing it with a provider-agnostic, per-event strategy is
tracked follow-up work — see RFC 0024). Expected outcomes on this branch:

- **openai**: resolves correctly (`hits_addend: 18`) — the usage chunk is
  exactly the event this heuristic was designed to find.
- **anthropic**: resolves (`hits_addend: 8`) for this mock's specific 4-event
  stream, but only because `message_delta` (which carries `output_tokens`)
  happens to be the penultimate event here — that's a coincidence of this
  exact event count, not a real fix. A stream with a different number of
  trailing events could easily miss it.
- **gemini**: **fails** — the penultimate event is a content chunk with no
  `usageMetadata` at all (Gemini only includes it in the true last chunk).
  Expect no `Report` call, the `warn` log, and the
  `kuadrant.body_extraction_misses` counter incrementing. This is the exact
  gap RFC 0024 describes and the SSE-rewrite follow-up is meant to close —
  seeing it fail here now is the expected, correct result.

### Inspecting traffic

```sh
docker compose logs -f mock-llm
docker compose logs -f envoy
docker compose logs -f limitador
```

### Clean up

```sh
make clean
```
