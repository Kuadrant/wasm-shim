## Wasm-shim configuration example: gRPC call at the response body phase

### Description

The Wasm module configuration that performs gRPC call with descriptors populated
from the upstream response body, json formatted, data.

### Run Manually

It requires Wasm module being built at `target/wasm32-unknown-unknown/debug/wasm_shim.wasm`.
Check *Makefile* at the root of the project to build the module.

```sh
make run
```

* Send the request with expected body

```sh
curl --resolve body-request.example.com:18000:127.0.0.1 "http://body-request.example.com:18000"/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "meta-llama/Llama-3.1-8B-Instruct",
    "message": [
      {
        "role": "user",
        "content": "Tell me a three sentence bedtime story about a unicorn."
      }
    ]
  }'
```

It should return `200 OK`.

Expected rate limiting service logs:

```sh
docker compose logs -f rlsbin
```

```console
rlsbin-1  | 2025-07-17T16:43:00.062Z DEBUG [rlsbin::envoy_rls::server] Request received: Request { metadata: MetadataMap { headers: {"te": "trailers", "grpc-timeout": "20m", "content-type": "application/grpc", "x-envoy-internal": "true", "x-envoy-expected-rq-timeout-ms": "20"} }, message: RateLimitRequest { domain: "domain-a", descriptors: [RateLimitDescriptor { entries: [Entry { key: "tokens", value: "24" }], limit: None }], hits_addend: 1 }, extensions: Extensions }
```

which contains the desired descriptor entry `entries: [Entry { key: "tokens", value: "24" }]`.

> Note: the tokens value may be different

### Clean up

```
make clean
```
