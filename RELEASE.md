# How to release wasm-shim

To release a version “vX.Y.Z” of the wasm-shim in GitHub and Quay.io, follow these steps:

1. Pick a `<git-ref>` (SHA-1) as source.

```shell
git checkout <git-ref>
```

2. Create a new "floating" commit with the release version removing the `-dev`
   suffix ([example](https://github.com/Kuadrant/wasm-shim/commit/55d785e6f6f56b57184a95b5bf285f43226e8974)).

3. Create a new tag and named release `vX.Y.Z`. Push the tag to GitHub. This will trigger the image to be built in
   Quay.io.

```shell
git tag -a vX.Y.Z -m "vX.Y.Z" -s
git push origin vX.Y.Z
```

4. Then at the GitHub repository, create a new release from the tag you just pushed, auto-generating the release notes.
   This will trigger the workflow to build the wasm-shim binary to append to the
   release ([example](https://github.com/Kuadrant/wasm-shim/releases/tag/v0.8.0)).

5. Now that the release has been created, create a PR to update to the next development (`-dev`)
   version ([example](https://github.com/Kuadrant/wasm-shim/pull/150))
