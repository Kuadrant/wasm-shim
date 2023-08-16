A Proxy-Wasm module written in Rust, acting as a shim between Envoy and Limitador.

## Sample configuration
Following is a sample configuration used by the shim.

```yaml
failureMode: deny
rateLimitPolicies:
  - name: rlp-ns-A/rlp-name-A
    domain: rlp-ns-A/rlp-name-A
    service: rate-limit-cluster
    hostnames: ["*.toystore.com"]
    rules:
    - conditions:
      - allOf:
        - selector: request.url_path
          operator: startswith
          value: /get
        - selector: request.host
          operator: eq
          value: test.toystore.com
        - selector: request.method
          operator: eq
          value: GET
      data:
      - selector:
          selector: request.headers.My-Custom-Header
      - static:
          key: admin
          value: "1"
```

## Features

#### Condition operators implemented

```Rust
#[derive(Deserialize, PartialEq, Debug, Clone)]
pub enum WhenConditionOperator {
    #[serde(rename = "eq")]
    EqualOperator,
    #[serde(rename = "neq")]
    NotEqualOperator,
    #[serde(rename = "startswith")]
    StartsWithOperator,
    #[serde(rename = "endswith")]
    EndsWithOperator,
    #[serde(rename = "matches")]
    MatchesOperator,
}
```

The `matches` operator is a a simple globbing pattern implementation based on regular expressions.
The only characters taken into account are:
* `?`: 0 or 1 characters
* `*`: 0 or more characters
* `+`: 1 or more characters

#### Selectors

Selector of an attribute from the contextual properties provided by kuadrant.
Currently, only some of the
[Envoy Attributes](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes)
can be used.

The struct is

```Rust
#[derive(Deserialize, Debug, Clone)]
pub struct SelectorItem {
    // Selector of an attribute from the contextual properties provided by kuadrant
    // during request and connection processing
    pub selector: String,

    // If not set it defaults to `selector` field value as the descriptor key.
    #[serde(default)]
    pub key: Option<String>,

    // An optional value to use if the selector is not found in the context.
    // If not set and the selector is not found in the context, then no data is generated.
    #[serde(default)]
    pub default: Option<String>,
}
```

Selectors are tokenized at each non-escaped occurrence of a separator character `.`.
Example:

```
Input: this.is.a.exam\.ple -> Retuns: ["this", "is", "a", "exam.ple"].
```

Some path segments include dot `.` char in them. For instance envoy filter names: `envoy.filters.http.header_to_metadata`.
In that particular cases, the dot chat (separator), needs to be escaped.


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

## Testing

```
cargo test
```

## Running local development environment

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
RateLimitDescriptor { entries: [Entry { key: "limit_to_be_activated", value: "1" }], limit: None }
```

```
RateLimitDescriptor { entries: [Entry { key: "source.address", value: "127.0.0.1:0" }], limit: None }
```

```
RateLimitDescriptor { entries: [Entry { key: "request.headers.My-Custom-Header-01", value: "my-custom-header-value-01" }], limit: None }
```

```
RateLimitDescriptor { entries: [Entry { key: "user_id", value: "bob" }], limit: None }
```

**Note:** Using [Header-To-Metadata filter](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/header_to_metadata_filter#config-http-filters-header-to-metadata), `x-dyn-user-id` header value is available in the metadata struct with the `user-id` key.

According to the defined limits:

```yaml
---
- namespace: rlp-ns-C/rlp-name-C
  max_value: 2
  seconds: 10
  conditions:
    - "limit_to_be_activated == '1'"
    - "user_id == 'bob'"
  variables: []
```

The third request in less than 10 seconds should return `429 Too Many Requests`.

### Clean up all resources

```
make stop-development
```
