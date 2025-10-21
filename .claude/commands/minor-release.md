description: Create a minor release by removing -dev suffix and creating a tag

Create a minor release following the process in RELEASE.md:

1. Read Cargo.toml to get current version (e.g., "0.12.0-dev")
2. Calculate release version by removing -dev suffix (e.g., "0.12.0-dev" -> "0.12.0")
3. Update version in Cargo.toml to the release version
4. Run `cargo check` to update Cargo.lock with new version
5. Git add Cargo.toml and Cargo.lock
6. Git commit with message "[release] v{VERSION}" (e.g., "[release] v0.12.0")
7. Create a signed, annotated git tag with: `git tag -a v{VERSION} -m "v{VERSION}" -s`
8. Show summary and ask me to review the commit and tag before proceeding
9. After user confirms, push the tag to origin with: `git push origin v{VERSION}`
10. Check if `gh` command is available:
   - If available, create GitHub release with: `gh release create v{VERSION} --generate-notes`
   - If not available, remind me to create the GitHub release manually from the tag
11. Show final summary and remind me to:
   - Verify the release was created successfully
   - Check that the Quay.io image build was triggered
   - Run `/bump-dev` to update to next dev version

Important notes:
- DO NOT push automatically - let me review first
- Use exact format from git history: "[release] v0.12.0"
- Remove the -dev suffix, don't bump the version number
- Update BOTH Cargo.toml and Cargo.lock (via cargo check)
- Include "Signed-off-by" in commit message following project conventions
- Include a comment in the commit message explicitly stating it was driven by an AI tool
- Create a SIGNED annotated tag (-a -s flags)
- This creates a "floating" commit as described in RELEASE.md step 2
