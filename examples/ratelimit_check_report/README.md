## Wasm-shim configuration example: Rate limit using check and report services

### Description

The Wasm module configuration that performs gRPC call with descriptors populated
from the upstream response body, json formatted, data.

It supports two response body formats: 
* json 
* [SSE events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events#event_stream_format).

### JSON format

It requires Wasm module being built at `target/wasm32-wasip1/debug/wasm_shim.wasm`.
Check *Makefile* at the root of the project to build the module. Usually running `make build`
at the root of the project.

```sh
make run
```

* Send the request with expected body

```sh
curl --resolve trlp.example.com:18000:127.0.0.1 "http://trlp.example.com:18000"/v1/chat/completions \
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
docker compose logs -f limitador
```

which contains the desired hits_addend value `hits_addend: 24`.

> Note: the tokens value may be different

* Inspect traffic Gateway - llm model

Traffic between the gateway and llm model can be inspected looking at logs from `upstream` service

```
docker compose logs -f upstream
```

* Gateway logs

```sh
docker compose logs -f envoy
```

### Server Sent Events streaming format

The Wasm module supports [SSE events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events#event_stream_format).
This example will setup upstream API sending responses in event stream format. 

Event stream format example:

```
data: {"id":"chatcmpl-a66529f6-cc01-4585-b7f0-b25f4eef7209","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"role":"assistant"}}]}
data: {"id":"chatcmpl-699f630b-f39e-4db8-8f6d-d866ceee8495","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":"To"}}]}
data: {"id":"chatcmpl-559ad4c8-18e7-45e3-9e37-09ac03da891b","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" be"}}]}
data: {"id":"chatcmpl-1df5a4b7-8584-44d1-8cb6-7392df6d48ff","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" or"}}]}
data: {"id":"chatcmpl-517aa649-c120-46be-8d4e-8e8f5ad103b6","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" not"}}]}
data: {"id":"chatcmpl-913cefb6-d877-46bb-9351-58b3bc298f1c","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" to"}}]}


data: {"id":"chatcmpl-fced2400-9d5b-4157-a9f7-2516e3b7aa06","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" be"}}]}
data: {"id":"chatcmpl-8535dfa3-bd8a-4683-9318-b8c8a7e397b9","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" that"}}]}
data: {"id":"chatcmpl-9810b087-7875-4bc8-a595-c68633527f65","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" is"}}]}
data: {"id":"chatcmpl-c7a55b1e-4f33-4304-87f1-b1e3b0872b10","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" the"}}]}
data: {"id":"chatcmpl-2a501eee-9ee2-4e1d-b275-3ab9686b21ed","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":null,"delta":{"content":" question."}}]}
data: {"id":"chatcmpl-68209c62-4bbb-4d67-a063-0c4896f1ba17","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":null,"choices":[{"index":0,"finish_reason":"stop","delta":{}}]}


data: {"id":"chatcmpl-fda5e419-5449-4cb1-9537-07523fe3b1a7","created":1758102619,"model":"meta-llama/Llama-3.1-8B-Instruct","usage":{"prompt_tokens":0,"completion_tokens":4,"total_tokens":4},"choices":[]}
data: [DONE]
```

It requires Wasm module being built at `target/wasm32-wasip1/debug/wasm_shim.wasm`.
Check *Makefile* at the root of the project to build the module. Usually running `make build`
at the root of the project.

Then run the system

```sh
make run
```

* Send the request. Note that `stream: true` is explicitly set to ensure a streaming response: 

```sh
curl --resolve sse-streaming.example.com:18000:127.0.0.1 "http://sse-streaming.example.com:18000"/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "meta-llama/Llama-3.1-8B-Instruct",
    "max_tokens": 100,
    "stream": true,
    "stream_options": {
        "include_usage": true
    },
    "messages": [
      {
        "role": "user",
        "content": "Tell me a three sentence bedtime story about a unicorn."
      }
    ]
  }'
```

It should return `200 OK`.

* Rate limiting service logs:

```console
docker compose logs -f limitador
```

which contains the desired hits_addend value `hits_addend: 24`.

> Note: the tokens value may be different

* Inspect traffic Gateway - llm model

Traffic between the gateway and llm model can be inspected looking at logs from `upstream` service

```
docker compose logs -f upstream
```

* Gateway logs

```sh
docker compose logs -f envoy
```

### Clean up

```
make clean
```
