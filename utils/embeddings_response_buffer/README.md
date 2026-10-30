# Dev/test env: large embeddings response vs. 1 MiB body buffer

Reproduces the embeddings token-counting failure where a large
`POST /v1/embeddings` response makes the gateway buffer the whole body (to read
the trailing `/usage/total_tokens`) and trip Envoy's
`response_payload_too_large`.

What it sets up:

- **envoy** with `per_connection_buffer_limit_bytes: 1048576` (1 MiB) and the
  Kuadrant wasm filter running a `TokenRateLimitPolicy`-style config: a `store`
  action reading `responseBodyJSON("/usage/total_tokens")` plus a `Report` to
  Limitador using that value as `hits_addend`, both scoped to `/v1/embeddings`.
- **upstream**: a mock that returns an `application/json` embeddings body larger
  than 1 MiB, with `usage` serialized **last** (after the big `data` array) --
  the OpenAI embeddings shape. Size is controlled by `N_INPUTS` (default 400,
  ~2.7 MiB).
- **limitador**: in-memory, logs each `Report` with its `hits_addend`.

The wasm module is mounted from `target/wasm32-wasip1/debug/wasm_shim.wasm`, so
build it first (from the repo root):

```sh
make build
```

Then run the env:

```sh
make run
```

Send a large embeddings request:

```sh
curl -s -o /dev/null -w "%{http_code}\n" \
  --resolve emb.example.com:18000:127.0.0.1 \
  http://emb.example.com:18000/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"model":"granite-embedding","input":["hello"]}'
```

Check the rate-limit report (the extracted `total_tokens`):

```sh
docker compose logs limitador | grep -i hits_addend
```

Expected with a correct (streaming) wasm build: HTTP `200`, a `Report` with
`hits_addend: 42` in the Limitador logs, and **no** `response_payload_too_large`
in the Envoy logs:

```sh
docker compose logs envoy | grep -i response_payload_too_large   # (empty)
```

To contrast against a buggy build, rebuild `make build` from a revision that
buffers the whole body and repeat -- the request fails (Envoy replaces it with a
`500` / `response_payload_too_large`, or the token is never extracted and no
matching `Report` appears).

### Clean up

```sh
make clean
```
