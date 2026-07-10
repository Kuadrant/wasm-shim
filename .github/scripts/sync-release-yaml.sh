#!/usr/bin/env bash
set -euo pipefail

RELEASE_YAML="${1:-release.yaml}"

WASM_SHIM_VERSION=$(cargo metadata --no-deps --format-version 1 \
  | jq --raw-output '.packages[] | select(.name=="wasm-shim") | .version')
KUADRANT_FILTER_VERSION=$(cargo metadata --no-deps --format-version 1 \
  | jq --raw-output '.packages[] | select(.name=="kuadrant-filter") | .version')

if [[ -z "$WASM_SHIM_VERSION" || "$WASM_SHIM_VERSION" == "null" ]]; then
  echo "::error::Could not read wasm-shim version from cargo metadata"
  exit 1
fi

if [[ -z "$KUADRANT_FILTER_VERSION" || "$KUADRANT_FILTER_VERSION" == "null" ]]; then
  echo "::error::Could not read kuadrant-filter version from cargo metadata"
  exit 1
fi

# On main, Cargo.toml has -dev versions but release.yaml uses 0.0.0 sentinel.
# Strip -dev suffix: if present, write 0.0.0 instead.
if [[ "$WASM_SHIM_VERSION" == *-dev* ]]; then
  WASM_SHIM_VERSION="0.0.0"
fi
if [[ "$KUADRANT_FILTER_VERSION" == *-dev* ]]; then
  KUADRANT_FILTER_VERSION="0.0.0"
fi

yq --inplace ".\"wasm-shim\".version = \"${WASM_SHIM_VERSION}\"" "$RELEASE_YAML"
yq --inplace ".\"kuadrant-filter\".version = \"${KUADRANT_FILTER_VERSION}\"" "$RELEASE_YAML"

echo "release.yaml synced: wasm-shim.version=${WASM_SHIM_VERSION}, kuadrant-filter.version=${KUADRANT_FILTER_VERSION}"
