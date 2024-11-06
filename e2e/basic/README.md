## Basic integration test

This is a integration test to validate basic happy path.

This test is being added to the CI test suite

### Description

The Wasm configuration defines a set of rules for `*.example.com`.

Two (rate limiting) services are being defined, namely `limitadorA` and `limitadorB`.

One `actionSet` is defined that has two actions.
Each action should hit the same limitador instance, decrementing the counter twice.

```yaml
"services": {
  "limitadorA": {
    "type": "ratelimit",
    "endpoint": "limitador",
    "failureMode": "deny"
  },
  "limitadorB": {
    "type": "ratelimit",
    "endpoint": "limitador",
    "failureMode": "deny"
  }
},
"actionSets": [
{
    "actions": [
        {
            "service": "limitadorA",
            "scope": "a",
            "data": [
                {
                    "expression": {
                        "key": "a",
                        "value": "1"
                    }
                }
            ]
        },
        {
            "service": "limitadorB",
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

The test will run one request and expect the counter to be decremented by two.
The counter starts with `30`, so after the request, the counter should be `28`.

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
