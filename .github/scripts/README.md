# Release Scripts

Helper scripts for the two-phase release process. All scripts use `release.yaml` as the default path but accept an override as the first positional argument.

## Source of Truth

**`Cargo.toml` is the authoritative source for the wasm-shim version.** `release.yaml` is a derived mirror maintained by `sync-release-yaml.sh` for cross-repo tooling compatibility.

## Scripts

### `sync-release-yaml.sh`

Reads the wasm-shim version from `cargo metadata` and writes it to `release.yaml`. If the Cargo.toml version contains `-dev`, the sentinel value `0.0.0` is written instead.

```bash
.github/scripts/sync-release-yaml.sh [release.yaml]
```

**Requires:** `cargo`, `jq`, `yq`

### `check-versions.sh`

Validates that `release.yaml` and `Cargo.toml` are consistent:

- If `release.yaml` has `0.0.0` (sentinel), `Cargo.toml` must end in `-dev`
- Otherwise, both must match exactly

```bash
.github/scripts/check-versions.sh [release.yaml]
```

**Requires:** `cargo`, `jq`, `yq`

### `parse-version.sh`

Reads the version from `release.yaml`, validates it as semver, and outputs decomposed components to `$GITHUB_OUTPUT` (or stdout when run locally).

```bash
.github/scripts/parse-version.sh [release.yaml]
```

**Outputs:** `version`, `major`, `minor`, `patch`, `release-branch`

**Requires:** `yq`

### `validate-release-yaml.sh`

Validates `release.yaml` for release readiness:

- On `release-*` branches: rejects `0.0.0` sentinel and `-dev` versions
- Checks that declared dependency versions have corresponding GitHub Releases

```bash
.github/scripts/validate-release-yaml.sh <branch-name> [org] [release.yaml]
```

**Requires:** `yq`, `gh` (GitHub CLI)
