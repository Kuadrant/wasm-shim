## Missing cluster integration test

This is a integration test to validate when envoy cluster does not exist.

Specifically, when happens on the second action.

This test is being added to the CI test suite

### Description

The Wasm configuration defines a set of rules for `*.example.com` and the rate limiting endpoint does
not exist from the defined set of envoy clusters.

```json
"services": {
  "mistyped-service": {
    "type": "ratelimit",
    "endpoint": "does-not-exist",
    "failureMode": "deny"
  }
},
"actionSets": [
{
    "actions": [
        {
            "service": "mistyped-service",
            "scope": "b",
            "data": [
                {
                    "expression": {
                        "key": "limit_to_be_activated",
                        "value": "1"
                    }
                }
            ]
        }
    ]
}
]
```

Check Envoy logs:

```
docker compose logs -f envoy
```

The test will run one request and expect it to fail because `failureMode` is set to `deny`.

### Run Manually

It requires Wasm module being built at `target/wasm32-unknown-unknown/debug/wasm_shim.wasm`.
Check *Makefile* at the root of the project to build the module.

```
make run
```

Run the test

```
make test
```

### Clean up

```
make clean
```
