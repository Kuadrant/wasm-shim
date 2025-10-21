# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Proxy-Wasm module written in Rust that acts as a shim between Envoy proxy and Kuadrant services (Authorino for authentication and Limitador for rate limiting). The module is compiled to WebAssembly and loaded into Envoy as an HTTP filter.

## Building & Testing

### Prerequisites

Install the WebAssembly target:
```bash
rustup target add wasm32-unknown-unknown
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

The built WASM module will be at: `target/wasm32-unknown-unknown/{debug|release}/wasm_shim.wasm`

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
cargo check --release --target wasm32-unknown-unknown
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

### Request Processing Flow

1. **FilterRoot** (src/filter/root_context.rs): The root context manages VM lifecycle and configuration
   - Parses plugin configuration on startup
   - Builds the ActionSetIndex from configured action sets
   - Creates KuadrantFilter instances for each HTTP request

2. **ActionSetIndex** (src/action_set_index.rs): Routes requests to applicable policies
   - Uses a radix trie to match request hostnames to action sets
   - Reverses hostname for efficient longest-match lookup (e.g., "test.example.com" → ".moc.elpmaxe.tset$")
   - Supports wildcard matching (e.g., "*.example.com")

3. **KuadrantFilter** (src/filter/kuadrant_filter.rs): Main HTTP filter context
   - Processes each HTTP request through multiple phases (request headers, request body, response headers, response body)
   - Executes applicable RuntimeActionSets for the request
   - Manages asynchronous gRPC calls to auth/rate-limit services
   - Handles direct responses (e.g., 401, 429) and header modifications

4. **RuntimeActionSet** (src/runtime_action_set.rs): Compiled representation of an ActionSet
   - Evaluates route rule predicates to determine if actions should run
   - Contains RuntimeActions that make gRPC calls to external services
   - Request data expressions are pre-compiled at configuration time

5. **RuntimeAction** (src/runtime_action.rs): Individual auth or rate-limit action
   - Builds gRPC requests to external services
   - Evaluates CEL predicates and conditional data expressions
   - Processes responses and determines next steps (continue, deny, modify headers)

### Service Integration

- **AuthService** (src/service/auth.rs): Communicates with Authorino using Envoy's External Authorization API
  - Service: `envoy.service.auth.v3.Authorization`
  - Method: `Check`

- **RateLimitService** (src/service/rate_limit.rs): Communicates with Limitador using Envoy's Rate Limit Service API
  - Standard service: `envoy.service.ratelimit.v3.RateLimitService` / `ShouldRateLimit`
  - Kuadrant extensions: `kuadrant.service.ratelimit.v1.RateLimitService` / `CheckRateLimit` and `Report`

### CEL Expression System

The module uses Common Expression Language (CEL) for predicates and data expressions:

- **Predicates** (src/data/cel.rs): Boolean expressions that determine when actions should execute
- **Expressions**: Generate values from request/response attributes to pass to services
- **Custom Functions**: `requestBodyJSON()` and `responseBodyJSON()` for parsing JSON bodies
- **Attribute System** (src/data/attribute.rs, src/data/property.rs): Provides access to Envoy attributes and auth service data

### Configuration Structure

Plugin configuration (src/configuration.rs) defines:
- **Services**: External auth/rate-limit service endpoints and failure modes
- **ActionSets**: Collections of actions with route matching rules
  - `routeRuleConditions`: Hostnames and CEL predicates for matching requests
  - `actions`: Auth or rate-limit actions with scopes and conditional data

## Important Constraints

### Clippy Lints
The project enforces strict error handling (Cargo.toml):
- `panic = "deny"` - No panic! calls allowed
- `unwrap_used = "deny"` - No .unwrap() calls allowed
- `expect_used = "deny"` - No .expect() calls allowed

Always use proper error handling with Result types and the ? operator.

### Protocol Buffers
Protobuf definitions are in `vendor-protobufs/` and compiled via build.rs. To update protobufs:
```bash
make update-protobufs
```

Generated protobuf code is in src/envoy/ - do not edit these files directly.

### WASM Target Limitations
- No std::thread support
- Limited system calls
- All external service communication must use Envoy's hostcalls API (proxy-wasm crate)
- Cannot use file I/O directly

## Testing Patterns

Tests use the proxy-wasm-test-framework for mocking Envoy hostcalls. Key testing utilities:
- Mock service responses for auth and rate-limit calls
- Verify header modifications and status codes
- Test CEL expression evaluation with PathCache for attribute resolution

Many tests use `#[serial_test]` annotation to prevent concurrent execution that could interfere with shared state.
