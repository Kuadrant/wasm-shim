## Basic integration test

This is a integration test to validate when envoy cluster exists but it is not reachable.

The test configures unreachable envoy cluster on the fist action `fail-on-first-action.example.com`,
as well as on the second action `fail-on-second-action.example.com`. Reason being to validate
error handling on the `on_grpc_call_response` event.

This test is being added to the CI test suite

### Description

```yaml
"services": {
  "limitadorA": {
    "type": "ratelimit",
    "endpoint": "limitador",
    "failureMode": "deny"
  },
  "limitador-unreachable": {
    "type": "ratelimit",
    "endpoint": "unreachable-cluster",
    "failureMode": "deny"
  }
},
"actionSets": [
{
    "name": "envoy-cluster-unreachable-on-first-action",
    "routeRuleConditions": {
        "hostnames": [
            "fail-on-first-action.example.com"
        ]
    },
    "actions": [
        {
            "service": "limitador-unreachable",
            "scope": "a",
            "data": [
                {
                    "expression": {
                        "key": "a",
                        "value": "1"
                    }
                }
            ]
        }
    ]
},
{
    "name": "envoy-cluster-unreachable-on-second-action",
    "routeRuleConditions": {
        "hostnames": [
            "fail-on-second-action.example.com"
        ]
    },
    "actions": [
        {
            "service": "limitadorA",
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
            "service": "limitador-unreachable",
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

And a new limit configuration

```yaml
- namespace: basic
  max_value: 30
  seconds: 60
  conditions:
  - "a == '1'"
  variables: []
```

The test will run two requests and expect them to fail because `failureMode` is set to `deny`.

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
