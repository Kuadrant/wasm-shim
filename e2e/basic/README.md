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
    "type": "dynamic",
    "endpoint": "limitador",
    "failureMode": "deny",
    "grpcService": "envoy.service.ratelimit.v3.RateLimitService",
    "grpcMethod": "ShouldRateLimit"
  },
  "limitadorB": {
    "type": "dynamic",
    "endpoint": "limitador",
    "failureMode": "deny",
    "grpcService": "envoy.service.ratelimit.v3.RateLimitService",
    "grpcMethod": "ShouldRateLimit"
  }
},
"actionSets": [
{
    "name": "basic",
    "routeRuleConditions": {
        "hostnames": ["*.example.com"]
    },
    "actions": [
        {
            "type": "grpc",
            "var": "ratelimit_response",
            "service": "limitadorA",
            "predicate": "true",
            "terminal": false,
            "label": "ratelimit",
            "messageBuilder": "envoy.service.ratelimit.v3.RateLimitRequest { domain: \"basic\", hits_addend: 1u, descriptors: [ envoy.extensions.common.ratelimit.v3.RateLimitDescriptor { entries: [ envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry { key: \"a\", value: string(1) } ] } ] }",
            "onReply": [
                { "type": "deny", "predicate": "ratelimit_response.overall_code == 2", "terminal": true, "denyWith": "DenyResponse{status: 429u, headers: ratelimit_response.response_headers_to_add, body: \"Too Many Requests\\n\"}" },
                { "type": "headers", "predicate": "ratelimit_response.overall_code == 1", "terminal": false, "target": "response", "headers": "ratelimit_response.response_headers_to_add" },
                { "type": "fail", "predicate": "ratelimit_response.overall_code != 1 && ratelimit_response.overall_code != 2", "terminal": true, "logMessage": "Unknown rate limit response code from ratelimit_response" }
            ]
        },
        {
            "type": "grpc",
            "execution": "sequential",
            "var": "ratelimit_response",
            "service": "limitadorB",
            "predicate": "true",
            "terminal": false,
            "label": "ratelimit",
            "messageBuilder": "envoy.service.ratelimit.v3.RateLimitRequest { domain: \"basic\", hits_addend: 1u, descriptors: [ envoy.extensions.common.ratelimit.v3.RateLimitDescriptor { entries: [ envoy.extensions.common.ratelimit.v3.RateLimitDescriptor.Entry { key: \"a\", value: string(1) } ] } ] }",
            "onReply": [
                { "type": "deny", "predicate": "ratelimit_response.overall_code == 2", "terminal": true, "denyWith": "DenyResponse{status: 429u, headers: ratelimit_response.response_headers_to_add, body: \"Too Many Requests\\n\"}" },
                { "type": "headers", "predicate": "ratelimit_response.overall_code == 1", "terminal": false, "target": "response", "headers": "ratelimit_response.response_headers_to_add" },
                { "type": "fail", "predicate": "ratelimit_response.overall_code != 1 && ratelimit_response.overall_code != 2", "terminal": true, "logMessage": "Unknown rate limit response code from ratelimit_response" }
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
  - "descriptors[0]['a'] == '1'"
  variables: []
```

The test will run one request and expect the counter to be decremented by two.
The counter starts with `30`, so after the request, the counter should be `28`.

### Run Manually

It requires Wasm module being built at `target/wasm32-wasip1/debug/wasm_shim.wasm`.
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
