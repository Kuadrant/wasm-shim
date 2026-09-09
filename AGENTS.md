# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Proxy-Wasm module written in Rust that acts as a shim between Envoy proxy and Kuadrant services (Authorino for authentication and Limitador for rate limiting). The module is compiled to WebAssembly and loaded into Envoy as an HTTP filter.

## Building & Testing

### Prerequisites

Install the WebAssembly target:
```bash
rustup target add wasm32-wasip1
```

### Build Commands

```bash
# Debug build
make build

# Release build
make build BUILD=release

# Build with specific features
make build FEATURES=debug-host-behaviour
```

The built WASM module will be at: `target/wasm32-wasip1/{debug|release}/wasm_shim.wasm`

### Testing

```bash
# Run all tests
cargo test

# Run a specific test
cargo test test_name

# Run tests in a specific module
cargo test module_name::
```

### Code Quality

```bash
# Format code
cargo fmt

# Run linter
cargo clippy --all-targets --all-features -- -D warnings

# Check code without building
cargo check --release --target wasm32-wasip1
```

## Local Development Environment

The project includes a complete local Kubernetes setup using kind:

```bash
# Set up local environment (creates kind cluster with Envoy, Authorino, Limitador)
make local-setup

# Expose Envoy for testing
kubectl port-forward --namespace kuadrant-system deployment/envoy 8000:8000

# Rebuild and deploy changes
make build local-rollout

# Clean up
make local-cleanup
```

## Architecture

### Workspace layout

This is a Cargo workspace with two crates:
- **crates/wasm-shim**: the actual Proxy-Wasm entrypoint compiled to `wasm32-wasip1`. Thin glue code: `FilterRoot` (root context / VM lifecycle) and `KuadrantFilter` (per-request HTTP filter) that drive a `Pipeline` built by `kuadrant-filter`.
- **crates/kuadrant-filter**: all filter logic as a plain (non-wasm) library — configuration parsing, CEL, the pipeline engine, generic gRPC service dispatch, tracing/metrics. Kept separate from `wasm-shim` so it can be unit-tested without the wasm target.

### Request Processing Flow

1. **FilterRoot** (crates/wasm-shim/src/filter/root_context.rs): the root context manages VM lifecycle and configuration
   - Parses plugin configuration on startup (`on_configure`) into a `PluginConfiguration`
   - Builds a `PipelineFactory` (`kuadrant_filter::kuadrant::PipelineFactory::try_from`) from the configured action sets
   - Owns a `DescriptorManager` that fetches gRPC service descriptors (via reflection, over a configurable "descriptor service") needed for generic dynamic gRPC dispatch, refetching missing ones on `on_tick`
   - Creates `KuadrantFilter` instances for each HTTP request

2. **PipelineFactory** (crates/kuadrant-filter/src/kuadrant/pipeline/factory.rs): routes requests to applicable action sets and builds their runtime pipelines
   - Uses a radix trie (`radix_trie::Trie`) to match request hostnames to compiled action sets
   - Reverses hostname for efficient longest-match lookup (e.g., "test.example.com" → ".moc.elpmaxe.tset$")
   - Supports wildcard matching (e.g., "*.example.com")
   - Compiles each `ActionSet` into a `Blueprint` at configuration time, then instantiates a `Pipeline` of runnable `Task`s per request

3. **KuadrantFilter** (crates/wasm-shim/src/filter/kuadrant_filter.rs): main HTTP filter context
   - Drives the `Pipeline` through request/response header and body phases (`on_http_request_headers`, `on_http_request_body`, `on_http_response_headers`, `on_http_response_body`)
   - Feeds asynchronous gRPC call responses back into the pipeline via `on_grpc_call_response`
   - Pauses/resumes the Envoy filter chain (`Action::Pause`/`Action::Continue`) based on pipeline state; handles direct responses (e.g., 401, 429) via the `SendReplyTask`

4. **Blueprint / Pipeline** (crates/kuadrant-filter/src/kuadrant/pipeline/): compiled and runtime representation of an `ActionSet`
   - `blueprint.rs`: compiles configured actions (predicate + `Operation`) into a `Blueprint`; CEL predicates and data expressions are pre-compiled at configuration time; computes dependency ordering between actions (`execution: sequential` fences off prior `parallel` actions)
   - `executor.rs`: `Pipeline` walks a queue of `Task`s, tracking deferred (in-flight gRPC) tasks, completed tasks, and teardown tasks, and reports whether the filter should pause or resume
   - `tasks/`: the concrete task kinds — `DynamicTask` (gRPC call to a `DynamicService`, with `onReply` sub-actions evaluated against the response), `ModifyHeadersTask`, `SendReplyTask` (deny responses), `StoreTask` (write a CEL value into the request-scoped store, optionally exported to the host), `FailTask`, `FailureModeTask` (wraps a task to honor `failureMode: allow|deny` on service errors), `ExportTracesTask`, `TracingDecoratorTask`

### Service Integration

There is no hardcoded auth-specific or rate-limit-specific service type. All external gRPC calls go through one generic, reflection-based service:

- **DynamicService** (crates/kuadrant-filter/src/services/dynamic.rs): dispatches to any gRPC service/method named in configuration (`grpcService` / `grpcMethod`), using `prost-reflect` descriptors resolved at runtime by `DescriptorManager` (crates/kuadrant-filter/src/filter/descriptor_manager.rs). Authorino's `envoy.service.auth.v3.Authorization/Check`, Limitador's `envoy.service.ratelimit.v3.RateLimitService/ShouldRateLimit` (and Kuadrant's own `kuadrant.service.ratelimit.v1.RateLimitService/CheckRateLimit` and `Report`), or any other reflectable gRPC service are all just configuration — not distinct Rust types.
- **TracingService** (crates/kuadrant-filter/src/services/tracing.rs): a non-gRPC pseudo-service (`type: tracing`) used to mark actions whose call should be wrapped in a span and exported via `ExportTracesTask`.

### CEL Expression System

The module uses Common Expression Language (CEL) for predicates and data expressions:

- **Predicates & Expressions** (crates/kuadrant-filter/src/data/cel.rs): boolean predicates that gate actions, and expressions that generate values (e.g. gRPC request messages, header sets, stored values) from request/response attributes
- **Custom Functions**: `requestBodyJSON()` and `responseBodyJSON()` for parsing JSON bodies
- **Attribute System**: `ReqRespCtx` (crates/kuadrant-filter/src/kuadrant/context.rs) is the per-request context — request/response body buffers, an attribute cache, a CEL value store, and tracing state — backed by an `AttributeResolver` trait (crates/kuadrant-filter/src/kuadrant/resolver/mod.rs) that abstracts the actual Envoy/wasm hostcalls (implemented by `ProxyWasmHost` in crates/wasm-shim, and by `MockWasmHost` in tests). `AttributeCache` (crates/kuadrant-filter/src/kuadrant/cache.rs, radix-trie-backed) memoizes resolved attributes for the lifetime of a request. Low-level attribute types live in crates/kuadrant-filter/src/data/attribute.rs.

### Configuration Structure

Plugin configuration (crates/kuadrant-filter/src/configuration.rs) defines:
- **services**: a map of named service instances. `type: dynamic` (default) is the generic gRPC service — `endpoint`, `grpcService`, `grpcMethod`, `failureMode` (`allow`|`deny`), `timeout`; `type: tracing` marks a tracing export target.
- **actionSets**: collections of actions with route matching rules
  - `routeRuleConditions`: `hostnames` and CEL `predicates` for matching requests
  - `actions`: each has a `predicate`, `terminal` flag, `isGuard`, `execution` (`parallel` (default) | `sequential`, controlling dependency/ordering between actions), optional `sources`, and one operation type: `grpc` (call a `dynamic` service; `onReply` sub-actions run against the decoded response), `deny`, `headers`, `store`, or `fail`
- **observability**: log level and optional tracing exporter configuration
- **descriptorService**: name of the service used to fetch gRPC descriptors for reflection-based dynamic dispatch

## Important Constraints

### Clippy Lints
The project enforces strict error handling (Cargo.toml, both crates):
- `panic = "deny"` - No panic! calls allowed
- `unwrap_used = "deny"` - No .unwrap() calls allowed
- `expect_used = "deny"` - No .expect() calls allowed

Always use proper error handling with Result types and the ? operator. A handful of narrowly-scoped `#[allow(clippy::panic)]` / `#[allow(clippy::expect_used)]` overrides exist for cases that are provably unreachable or genuinely unrecoverable (e.g. crates/wasm-shim/src/filter/kuadrant_filter.rs, crates/kuadrant-filter/src/kuadrant/pipeline/{factory,blueprint}.rs, .../tasks/send_reply.rs, crates/kuadrant-filter/src/data/cel.rs, build.rs). Treat these as rare, deliberate exceptions, not a pattern to extend — new code should not add more without good reason.

### Protocol Buffers
Protobuf definitions are in `vendor-protobufs/` and compiled via build.rs. To update protobufs:
```bash
make update-protobufs
```

Generated protobuf code is in `crates/kuadrant-filter/src/proto/` - do not edit these files directly.

### WASM Target Limitations
- No std::thread support
- Limited system calls
- All external service communication must use Envoy's hostcalls API (proxy-wasm crate)
- Cannot use file I/O directly

## Testing Patterns

- `crates/wasm-shim/tests/` holds integration-style tests using the proxy-wasm-test-framework for mocking Envoy hostcalls end-to-end (config parsing, header modifications, status codes, streaming/body handling).
- `crates/kuadrant-filter` unit-tests its `AttributeResolver` consumers directly against `MockWasmHost` (crates/kuadrant-filter/src/kuadrant/resolver/mock.rs, `#[cfg(test)]`) without needing the wasm target.
- Many tests use `#[serial_test]` annotation to prevent concurrent execution that could interfere with shared state.
