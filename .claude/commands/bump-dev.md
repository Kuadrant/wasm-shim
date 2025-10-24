description: Bump version to next -dev after a release

Bump the version to the next development version after a release:

1. Read Cargo.toml to get current version
2. Calculate next minor version with -dev suffix (e.g., 0.11.0-dev -> 0.12.0-dev)
3. Update version in Cargo.toml
4. Run `cargo check` to update Cargo.lock with new version
5. Git add Cargo.toml and Cargo.lock
6. Git commit with message "On to the next release: {VERSION}" (e.g., "On to the next release: 0.12.0")
7. Show summary and remind me to:
   - Review the commit
   - Push with: git push

Important notes:
- DO NOT push automatically
- Use exact format from git history: "On to the next release: 0.12.0"
- Bump the MINOR version (0.11.0-dev -> 0.12.0-dev, not 0.11.1-dev)
- Update BOTH Cargo.toml and Cargo.lock (via cargo check)
- Include "Signed-off-by" in commit message following project conventions
- Include a comment in the commit message explicitly stating it was driven by an AI tool
