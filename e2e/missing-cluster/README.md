## Missing cluster integration test

This is a integration test to validate when envoy cluster does not exist.

The test configures not existing envoy cluster on the fist action `fail-on-first-action.example.com`, 
as well as on the second action `fail-on-second-action.example.com`. Reason being to validate
error handling on the `on_grpc_call_response` event.

This test is being added to the CI test suite

### Description

```json
"services": {
  "existing-service": {
    "type": "ratelimit",
    "endpoint": "existing-cluster",
    "failureMode": "deny"
  }
  "mistyped-service": {
    "type": "ratelimit",
    "endpoint": "does-not-exist",
    "failureMode": "deny"
  }
},
"actionSets": [
{
    "name": "envoy-cluster-not-found-on-first-action",
    "routeRuleConditions": {
        "hostnames": [
            "fail-on-first-action.example.com"
        ]
    },
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
},
{
    "name": "envoy-cluster-not-found-on-second-action",
    "routeRuleConditions": {
        "hostnames": [
            "fail-on-second-action.example.com"
        ]
    },
    "actions": [
        {
            "service": "existing-service",
            "scope": "b",
            "data": [
                {
                    "expression": {
                        "key": "limit_to_be_activated",
                        "value": "1"
                    }
                }
            ]
        },
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
