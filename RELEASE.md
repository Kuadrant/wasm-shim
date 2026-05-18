# How to release wasm-shim

The wasm-shim uses an automated release process with protected release branches.

## Quick Start

1. **Run the workflow**: Actions → “Automated Release WASM Shim” → “Run workflow”
   - **wasmShimVersion**: Version to release (e.g., `0.12.1`)
   - **gitRef**: `main` for new minor, `release-0.12` for patches
2. **Review and merge the PR** that gets created
3. **Done** - tag and release happen automatically on merge

## Standard Release

1. Actions → “Automated Release WASM Shim” → “Run workflow”
   - **gitRef**: `main` (for new minor like `0.13.0`) or `release-0.12` (for patch like `0.12.1`)
   - **wasmShimVersion**: `0.12.1` (or whatever version)
2. Review and merge the PR
3. Tag and release created automatically

## Release with Cherry-picked Fixes

1. **First, cherry-pick and merge your fixes:**

   ```bash
   git checkout -b backport-my-fix origin/release-0.12
   git cherry-pick <commit-sha>
   git push -u origin HEAD
   # Create PR from backport-my-fix to release-0.12, get it merged
   ```

2. **Then run the release workflow:**
   - Actions → “Automated Release WASM Shim” → “Run workflow”
   - **gitRef**: `release-0.12` (picks up the cherry-picks)
   - **wasmShimVersion**: `0.12.1`

3. Review and merge the version bump PR
4. Tag and release created automatically

## Details

- Version format: semver without `v` prefix (e.g., `0.12.1`, not `v0.12.1`)
- Release branches: `release-0.12`, `release-0.13`, etc.
- One branch per minor version, shared by all patches
- Workflow creates the release branch if it doesn't exist
  - For new minor versions, creates from specified `gitRef`
  - For existing branches, keeps existing branch (use `gitRef` to catch up via PR)
