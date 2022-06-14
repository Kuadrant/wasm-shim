A Proxy-Wasm module written in Rust, acting as a shim between Envoy and Limitador.

## Sample configuration
Following is a sample configuration used by the shim.

```yaml
failure_mode_deny: true
ratelimitpolicies:
  default/toystore:
    hosts:
    - "*.toystore.com"
    rules:
    - operations:
      - paths:
        - "/admin/toy"
        methods:
        - POST
        - DELETE
      actions:
      - generic_key:
          descriptor_value: 'yes'
          descriptor_key: admin
    global_actions:
    - generic_key:
        descriptor_value: 'yes'
        descriptor_key: vhaction
    upstream_cluster: rate-limit-cluster
    domain: toystore-app
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
