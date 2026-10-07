#!/usr/bin/env bash
set -euo pipefail

RELEASE_YAML="${1:-release.yaml}"

if [[ ! -f "$RELEASE_YAML" ]]; then
  echo "::error::File not found: $RELEASE_YAML"
  exit 1
fi

YAML_VERSION=$(yq '.wasm-shim.version' "$RELEASE_YAML")
CARGO_VERSION=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name=="wasm-shim") | .version')

ERRORS=0

if [[ "$YAML_VERSION" == "0.0.0" ]]; then
  if [[ "$CARGO_VERSION" != *-dev* ]]; then
    echo "::error::release.yaml version is 0.0.0 but Cargo.toml version '${CARGO_VERSION}' does not end in -dev"
    ERRORS=$((ERRORS + 1))
  fi
else
  if [[ "$YAML_VERSION" != "$CARGO_VERSION" ]]; then
    echo "::error::Version mismatch: release.yaml has '${YAML_VERSION}' but Cargo.toml has '${CARGO_VERSION}'"
    ERRORS=$((ERRORS + 1))
  fi
fi

if [[ "$ERRORS" -gt 0 ]]; then
  echo "::error::Version consistency check failed with ${ERRORS} error(s)"
  exit 1
fi

echo "Version consistency check passed: release.yaml and Cargo.toml agree"
echo "  release.yaml=${YAML_VERSION} Cargo.toml=${CARGO_VERSION}"
