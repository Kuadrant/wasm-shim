## Remote address integration test

This is a integration test to validate integration between Envoy and Kuadrant's Wasm module.

The Wasm module defines `source.remote_address` that should generate `trusted client address`
based on envoy configuration. If Envoy changes the contract, this test should fail.

This test is being added to the CI test suite

### Description

The Wasm configuration defines a set of rules for `*.example.com`.

```yaml
{
    "name": "ratelimit-source",
    "hostnames": [
        "*.example.com"
    ],
    "rules": [
        {
            "conditions": [
                {
                    "allOf": [
                    {
                        "selector": "source.remote_address",
                        "operator": "neq",
                        "value": "50.0.0.1"
                    }
                    ]
                }
            ],
            "actions": [
                {
                    "extension": "limitador",
                    "scope": "ratelimit-source",
                    "data": [
                        {
                            "selector": {
                                "selector": "source.remote_address"
                            }
                        }
                    ]
                }
            ]
        }
    ]
}
```

And a new limit configuration

```yaml
- namespace: ratelimit-source
  max_value: 2
  seconds: 30
  conditions: []
  variables:
    - source.remote_address
```

That configuration enables source based rate limiting on `*.example.com` subdomains,
with only one "privileged" exception: the IP "50.0.0.1" will not be rate limited.

The test will run two requests:
* IP "40.0.0.1" -> the test will verify it is being rate limited inspecting limitador for counters
* IP "50.0.0.1" -> the test will verify it is not being rate limited inspecting limitador for counters

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
