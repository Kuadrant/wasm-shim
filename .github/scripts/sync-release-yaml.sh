#!/usr/bin/env bash
set -euo pipefail

RELEASE_YAML="${1:-release.yaml}"

VERSION=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name=="wasm-shim") | .version')

if [[ -z "$VERSION" || "$VERSION" == "null" ]]; then
  echo "::error::Could not read wasm-shim version from cargo metadata"
  exit 1
fi

if [[ "$VERSION" == *-dev* ]]; then
  VERSION="0.0.0"
fi

yq --inplace ".\"wasm-shim\".version = \"${VERSION}\"" "$RELEASE_YAML"

echo "release.yaml synced: version=${VERSION}"
