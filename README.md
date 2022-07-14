A Proxy-Wasm module written in Rust, acting as a shim between Envoy and Limitador.

## Sample configuration
Following is a sample configuration used by the shim.

```yaml
failure_mode_deny: true
rate_limit_policies:
  - name: toystore
    rate_limit_domain: toystore-app
    upstream_cluster: rate-limit-cluster
    hostnames: ["*.toystore.com"]
    gateway_actions:
      - rules:
          - paths: ["/admin/toy"]
            methods: ["GET"]
            hosts: ["pets.toystore.com"]
        configurations:
          - actions:
            - generic_key:
                descriptor_key: admin
                descriptor_value: "1"
```

## Building

Prerequisites:

* Install `wasm32-unknown-unknown` build target

```
rustup target add wasm32-unknown-unknown
```

Build the WASM module

```
make build
```

Build the WASM module in release mode

```
make build BUILD=release
```

## Running/Testing locally

`docker` and `docker-compose` required.

Run local development environment

```
make development
```

Three rate limit policies defined for e2e testing:

* `rlp-a`: Actions should not generate descriptors. Hence, rate limiting service should **not** be called.

```
curl -H "Host: test.a.com" http://127.0.0.1:18000/get
```

* `rlp-b`: Rules do not match. Hence, rate limiting service should **not** be called.

```
curl -H "Host: test.b.com" http://127.0.0.1:18000/get
```

* `rlp-c`: Four descriptors from multiple action types should be generated. Hence, rate limiting service should be called.

```
curl -H "Host: test.c.com" -H "x-forwarded-for: 127.0.0.1" -H "My-Custom-Header-01: my-custom-header-value-01" -H "My-Custom-Header-02: my-custom-header-value-02" http://127.0.0.1:18000/get
```

Clean up all resources

```
make stop-development
```
