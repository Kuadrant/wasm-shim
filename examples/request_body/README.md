## Wasm-shim configuration example: gRPC call at the request body phase

### Description

The Wasm module configuration that performs gRPC call with descriptors populated
from the downstream request body, json formatted, data.

### Run Manually

It requires Wasm module being built at `target/wasm32-unknown-unknown/debug/wasm_shim.wasm`.
Check *Makefile* at the root of the project to build the module.

```sh
make run
```

* Send the request with expected body

```sh
curl --resolve body-request.example.com:18000:127.0.0.1 "http://body-request.example.com:18000"/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4.1",
    "input": "Tell me a three sentence bedtime story about a unicorn."
  }'
```

It should return `200 OK`.

Expected rate limiting service logs:

```sh
docker compose logs -f rlsbin
```

```console
rlsbin-1  | 2025-06-17T08:19:30.014Z DEBUG [rlsbin::envoy_rls::server] Request received: Request { metadata: MetadataMap { headers: {"te": "trailers", "grpc-timeout": "20m", "content-type": "application/grpc", "x-envoy-internal": "true", "x-envoy-expected-rq-timeout-ms": "20"} }, message: RateLimitRequest { domain: "domain-a", descriptors: [RateLimitDescriptor { entries: [Entry { key: "model", value: "gpt-4.1" }], limit: None }], hits_addend: 1 }, extensions: Extensions }
```

which contains the desired descriptor entry `entries: [Entry { key: "model", value: "gpt-4.1" }]`.

### Clean up

```
make clean
```
