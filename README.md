# Wasm-shim

[![Rust](https://github.com/Kuadrant/wasm-shim/actions/workflows/rust.yaml/badge.svg)](https://github.com/Kuadrant/wasm-shim/actions/workflows/rust.yaml)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B162%2Fgit%2Bgithub.com%2FKuadrant%2Fwasm-shim.svg?type=shield&issueType=license)](https://app.fossa.com/projects/custom%2B162%2Fgit%2Bgithub.com%2FKuadrant%2Fwasm-shim?ref=badge_shield&issueType=license)

A Proxy-Wasm module written in Rust, acting as a shim between Envoy and both Rate-limiting and External Auth services.

## Sample configuration

Following is a sample configuration used by the shim.

```yaml
services:
  auth-service:
    type: auth
    endpoint: auth-cluster
    failureMode: deny
    timeout: 10ms
  ratelimit-service:
    type: ratelimit
    endpoint: ratelimit-cluster
    failureMode: allow
  tracing-service:
    type: tracing
    endpoint: tracing-cluster
    failureMode: allow
observability:
  httpHeaderIdentifier: x-request-id
  defaultLevel: INFO
  tracing:
    service: tracing-service
actionSets:
  - name: rlp-ns-A/rlp-name-A
    routeRuleConditions:
      hostnames: [ "*.toystore.com" ]
      predicates:
      - request.url_path.startsWith("/get")
      - request.host == "test.toystore.com"
      - request.method == "GET"
    actions:
    - service: auth-service
      scope: auth-scope-a
      predicates:
        - auth.identity.user_id == "alice"
    - service: ratelimit-service
      scope: ratelimit-scope-a
      conditionalData:
      - predicates:
        - auth.identity.anonymous == true
        data:
        - expression:
            key: my_header
            value: request.headers["my-custom-header"]
```

## Features

### CEL Predicates and Expression

`routeRuleConditions`'s `predicate`s are expressed in [Common Expression Language (CEL)](https://cel.dev). `Predicate`s
evaluating to a `bool` value, while `Expression`, used for passing data to a service, evaluate to some `Value`.

These expression can operate on the data made available to them through the Well Known Attributes, see below

### Custom CEL Functions

#### `requestBodyJSON(json_pointer)`

Parses request body as json and looks up a value by a JSON Pointer.
JSON Pointer defines a string syntax for identifying a specific value within a JavaScript Object Notation (JSON) document.
A Pointer is a Unicode string with the reference tokens separated by `/`.
For more information read [RFC6901](https://datatracker.ietf.org/doc/html/rfc6901).

If the request body is not a valid JSON, the function returns evaluation error.
If there is no such value, the function returns evaluation error.
If the value is found, it returns the value as a CEL `Value`.

Example:

when the request body is:

```json
{
  "my": {
    "value": "hello",
    "list": ["a", "b", "c"]
  }
}
```
and the expression is:

```yaml
data:
- expression:
    key: my_value
    value: requestBodyJSON('/my/value')
```

it evaluates to: `"hello"` CEL value. Similarly,

`requestBodyJSON('/my/list/1')` evaluates to `"b"` CEL value.

`requestBodyJSON('/a/b/c')` evaluates to `Null` CEL value.


It can also be used in predicates:

```yaml
predicates:
- requestBodyJSON('/my/value') == 'hello'
```

#### `responseBodyJSON(json_pointer)`

Parses response body as json and looks up a value by a JSON Pointer.
JSON Pointer defines a string syntax for identifying a specific value within a JavaScript Object Notation (JSON) document.
A Pointer is a Unicode string with the reference tokens separated by `/`.
For more information read [RFC6901](https://datatracker.ietf.org/doc/html/rfc6901).

If the response body is not a valid JSON, the function returns evaluation error.
If there is no such value, the function returns evaluation error.
If the value is found, it returns the value as a CEL `Value`.

Example:

when the response body is:

```json
{
  "my": {
    "value": "hello",
    "list": ["a", "b", "c"]
  }
}
```
and the expression is:

```yaml
data:
- expression:
    key: my_value
    value: responseBodyJSON('/my/value')
```

it evaluates to: `"hello"` CEL value. Similarly,

`responseBodyJSON('/my/list/1')` evaluates to `"b"` CEL value.

`responseBodyJSON('/a/b/c')` evaluates to `Null` CEL value.


It can also be used in predicates:

```yaml
predicates:
- responseBodyJSON('/my/value') == 'hello'
```

### Well Known Attributes

| Attribute                                                                                               | Description                                                                                                                                                                                                                    |
|---------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| [Envoy Attributes](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/attributes) | Contextual properties provided by Envoy during request and connection processing                                                                                                                                               |
| `source.remote_address`                                                                                 | This attribute evaluates to the `trusted client address` (IP address without port) as it is being defined by [Envoy Doc](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_conn_man/headers#x-forwarded-for) |
| `auth.*`                                                                                                | Data made available by the authentication service to the `ActionSet`'s pipeline                                                                                                                                                |

### Metrics

The WASM module exposes the following Prometheus-compatible metrics via Envoy:

| Metric Name           | Type    | Description                                                      |
|-----------------------|---------|------------------------------------------------------------------|
| `kuadrant.configs`    | Counter | Number of times the plugin configuration has been loaded         |
| `kuadrant.hits`       | Counter | Number of requests that matched an action set                    |
| `kuadrant.misses`     | Counter | Number of requests that did not match any action set             |
| `kuadrant.allowed`    | Counter | Number of requests allowed after evaluation                      |
| `kuadrant.denied`     | Counter | Number of requests denied as a result of actions                 |
| `kuadrant.errors`     | Counter | Number of errors encountered during request processing           |

These metrics are automatically exposed through Envoy's stats endpoint and can be scraped by Prometheus or other monitoring systems. To view metrics, access Envoy's admin interface (typically at `:8001/stats/prometheus`).

## Building

Prerequisites:

* Install `wasm32-wasip1` build target

```
rustup target add wasm32-wasip1
```

Build the WASM module

```
make build
```

Build the WASM module in release mode

```
make build BUILD=release
```

Build the WASM module with features

```
make build FEATURES=debug-host-behaviour
```

## Testing

```
cargo test
```

## Running local development environment (kind)

`docker` is required.

Run local development environment

```sh
make local-setup
```

This deploys a local kubernetes cluster using kind, with the local build of wasm-shim mapped to the envoy container. An
echo API as well as limitador, authorino, and some test policies are configured.

To expose the envoy endpoint run the following:

```sh
kubectl port-forward --namespace kuadrant-system deployment/envoy 8000:8000
```

There is then a single auth action set defined for e2e testing:

* `auth-a` which defines auth is required for requests to `/get` for the `AuthConfig` with `effective-route-1`

```sh
curl -H "Host: test.a.auth.com" http://127.0.0.1:8000/get -i
# HTTP/1.1 401 Unauthorized
```

```sh
curl -H "Host: test.a.auth.com" -H "Authorization: APIKEY IAMALICE" http://127.0.0.1:8000/get -i
# HTTP/1.1 200 OK
```

And some rate limit action sets defined for e2e testing:

* `rlp-a`: Only one data item. Data selector should not generate return any value. Thus, descriptor should be empty and
  rate limiting service should **not** be called.

```sh
curl -H "Host: test.a.rlp.com" http://127.0.0.1:8000/get -i
```

* `rlp-b`: Conditions do not match. Hence, rate limiting service should **not** be called.

```sh
curl -H "Host: test.b.rlp.com" http://127.0.0.1:8000/get -i
```

* `rlp-c`: Descriptor entries from multiple data items should be generated. Hence, rate limiting service should be called.

```sh
curl -H "Host: test.c.rlp.com" -H "x-forwarded-for: 50.0.0.1" -H "my-custom-header-01: my-custom-header-value-01" -H "x-dyn-user-id: bob" http://127.0.0.1:8000/get -i
```

Check limitador logs for received descriptor entries.

```sh
kubectl logs -f deployment/limitador-limitador -n kuadrant-system
```

The expected descriptor entries:

```
Entry { key: "limit_to_be_activated", value: "1" }
```

```
Entry { key: "source.address", value: "50.0.0.1:0" }
```

```
Entry { key: "request.headers.my-custom-header-01", value: "my-custom-header-value-01" }
```

```
Entry { key: "user_id", value: "bob" }
```

* `multi-a` which defines two actions for authenticated ratelimiting.

```sh
curl -H "Host: test.a.multi.com" http://127.0.0.1:8000/get -i
# HTTP/1.1 401 Unauthorized
```

Alice has 5 requests per 10 seconds:
```sh
while :; do curl --write-out '%{http_code}\n' --silent --output /dev/null -H "Authorization: APIKEY IAMALICE" -H "Host: test.a.multi.com" http://127.0.0.1:8000/get | grep -E --color "\b(429)\b|$"; sleep 1; done
```

Bob has 2 requests per 10 seconds:
```sh
while :; do curl --write-out '%{http_code}\n' --silent --output /dev/null -H "Authorization: APIKEY IAMBOB" -H "Host: test.a.multi.com" http://127.0.0.1:8000/get | grep -E --color "\b(429)\b|$"; sleep 1; done
```

To rebuild and deploy to the cluster:

```sh
make build local-rollout
```

Stop and clean up resources:

```sh
make local-cleanup
```

## License

[Apache 2.0 License](LICENSE)

[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B162%2Fgit%2Bgithub.com%2FKuadrant%2Fwasm-shim.svg?type=large&issueType=license)](https://app.fossa.com/projects/custom%2B162%2Fgit%2Bgithub.com%2FKuadrant%2Fwasm-shim?ref=badge_large&issueType=license)
