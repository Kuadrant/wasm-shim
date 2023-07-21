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

* `rlp-a`: Only one data item. Data selector should not generate return any value. Thus, descriptor should be empty and rate limiting service should **not** be called.

```
curl -H "Host: test.a.com" http://127.0.0.1:18000/get
```

* `rlp-b`: Conditions do not match. Hence, rate limiting service should **not** be called.

```
curl -H "Host: test.b.com" http://127.0.0.1:18000/get
```

* `rlp-c`: Four descriptors from multiple rules should be generated. Hence, rate limiting service should be called.

```
curl -H "Host: test.c.com" -H "x-forwarded-for: 127.0.0.1" -H "My-Custom-Header-01: my-custom-header-value-01" -H "x-dyn-user-id: bob" http://127.0.0.1:18000/get
```

The expected descriptors:

```
RateLimitDescriptor { entries: [Entry { key: "limit-to-be-activated", value: "1" }], limit: None }
```

```
RateLimitDescriptor { entries: [Entry { key: "source.address", value: "127.0.0.1:0" }], limit: None }
```

```
RateLimitDescriptor { entries: [Entry { key: "request.headers.My-Custom-Header-01", value: "my-custom-header-value-01" }], limit: None }
```

```
RateLimitDescriptor { entries: [Entry { key: "metadata.filter_metadata.envoy\\.filters\\.http\\.header_to_metadata.user-id", value: "bob" }], limit: None }
```

**Note:** Using [Header-To-Metadata filter](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/header_to_metadata_filter#config-http-filters-header-to-metadata), `x-dyn-user-id` header value is available in the metadata struct with the `user-id` key.

Clean up all resources

```
make stop-development
```
