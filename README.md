A Proxy-Wasm module written in Rust, acting as a shim between Envoy and Limitador.

## Sample configuration
Following is a sample configuration used by the shim.

```yaml
failure_mode_deny: true,
ratelimitpolicies:
  - ratelimitpolicy-1:
      hosts: ["*.toystore.com"]
      rules:
        - operations:
            - paths: ["/toy*"]
              methods: ["GET"]
          actions:
            - generic_key:
                descriptor_key: "get-toy"
                descriptor_value: "yes"
      global_actions:
        - generic_key:
            descriptor_key: "vhost-hit"
            descriptor_value: "yes"
      upstream_cluster: "limitador" # Should match cluster name in envoy
      domain: "toystore"
```
