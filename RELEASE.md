# How to release wasm-shim

The wasm-shim uses a two-phase release process as defined by [RFC 0020](https://github.com/Kuadrant/architecture/blob/main/rfcs/0020-two-phase-release-workflow.md).

## Overview

Every release is split into two workflows with a PR-based review gate between them:

1. **Pre-release** — makes code changes and opens a PR to the release branch
2. **Release** — tests, tags, builds artifacts, and creates the GitHub Release

The `release.yaml` file at the repository root is the machine-readable source of truth for version information.

## Quick Start

1. **Run the pre-release workflow**: Actions → "Pre-release" → "Run workflow"
   - **version**: Target version (e.g., `0.13.0`)
   - **source-branch**: `main` for new minor releases (default)
2. **Review and merge the PR** that gets created against the release branch
   - The version gate check validates `release.yaml` before merge
3. **Run the release workflow**: Actions → "Release" → "Run workflow"
   - **release-branch**: The release branch (e.g., `release-0.13`)
4. **Done** — smoke tests run, tag is created, artifacts are built, and the GitHub Release is published

## Standard Minor Release

1. Actions → "Pre-release" → "Run workflow"
   - **version**: `0.13.0`
   - **source-branch**: `main`
2. The workflow creates branch `release-0.13` (if it doesn't exist), updates `release.yaml` and `Cargo.toml`, and opens a PR
3. Review and merge the PR (version gate and CI checks must pass)
4. Actions → "Release" → "Run workflow"
   - **release-branch**: `release-0.13`
5. The workflow reads the version from `release.yaml`, runs smoke tests, creates tag `v0.13.0`, builds the WASM binary and container image, and creates the GitHub Release

## Patch Release

1. Prepare a branch with the backported fixes:
   ```bash
   git checkout -b backport-my-fix origin/release-0.13
   git cherry-pick <commit-sha>
   git push -u origin backport-my-fix
   ```

2. Actions → "Pre-release" → "Run workflow"
   - **version**: `0.13.1`
   - **source-branch**: `backport-my-fix`
3. Review and merge the PR (contains both the backported fixes and the version bump)
4. Actions → "Release" → "Run workflow"
   - **release-branch**: `release-0.13`

## Repository Configuration

### Required Secrets

Configure these in Settings → Secrets and variables → Actions → Repository secrets:

| Secret | Description |
|--------|-------------|
| `IMG_REGISTRY_USERNAME` | Container registry username or robot account |
| `IMG_REGISTRY_TOKEN` | Container registry password or token |

### Optional Variables

Configure these in Settings → Secrets and variables → Actions → Repository variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `IMG_REGISTRY_ORG` | `kuadrant` | Container registry organization/namespace (e.g., your Quay.io org for forks) |

### Required Repository Settings

- **Actions → General → Workflow permissions**: "Allow GitHub Actions to create and approve pull requests" must be enabled (required by the pre-release workflow to open PRs)

## Details

- **Version format**: semver without `v` prefix in `release.yaml` (e.g., `0.13.0`, not `v0.13.0`)
- **Release branches**: `release-0.13`, `release-0.14`, etc. — one branch per minor version, shared by all patches
- **Version gate**: A CI check on release branch PRs validates that `release.yaml` has a concrete version (not `0.0.0`)
- **On `main`**: `release.yaml` always has version `0.0.0` (sentinel for active development)
- **Artifacts built during release**: WASM binary (attached to GitHub Release) and container image (pushed to `quay.io/<IMG_REGISTRY_ORG>/wasm-shim`)
- **GitHub Release is always the last step** — if any preceding step fails, no release is created
