## Wasm-shim configuration example: rate limiting service

### Description

The Wasm module configuration that performs gRPC call to the rate limiting service call.

### Run Manually

It requires Wasm module being built at `target/wasm32-wasip1/debug/wasm_shim.wasm`.
Check *Makefile* at the root of the project to build the module. Usually running `make build`
at the root of the project.

```sh
make run
```

* Send the request

```sh
curl --resolve ratelimit.example.com:18000:127.0.0.1 "http://ratelimit.example.com:18000"/path 
```

It should return `200 OK`.

Expected rate limiting service logs:

```sh
docker compose logs -f limitador
```

which contains the desired descriptor entry `entries: [Entry { key: "tokens", value: "24" }]`.

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
