## Parallel requests integration test

This is a integration test to validate when envoy receives multiple parallel requests.

This test is being added to the CI test suite

### Description

The Wasm configuration defines a set of rules for `*.example.com`

```json
"services": {
  "limitador": {
    "type": "ratelimit",
    "endpoint": "limitador",
    "failureMode": "deny"
  }
},
"actionSets": [
{
    "actions": [
        {
            "service": "limitador",
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

The test will run multiple requests in parallel and expect all of them to succeed.

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
