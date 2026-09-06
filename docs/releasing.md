# Releasing TRend

TRend release tags use `vMAJOR.MINOR.PATCH`. Every workspace package must use
the same version, and [../CHANGELOG.md](../CHANGELOG.md) must contain a matching
section.

## Prepare

1. Update every workspace `Cargo.toml` version and refresh `Cargo.lock`.
2. Move completed changelog entries from `Unreleased` into a dated version.
3. Run the complete local validation:

```powershell
.\scripts\validate-repository.ps1 -IncludeLinux -IncludeAndroid
.\scripts\validate-release.ps1 -Tag v0.1.0
```

4. Commit the release state and push it normally so CI passes.

## Publish

Create and push an annotated tag only after the commit passes CI:

```powershell
git tag -a v0.1.0 -m "TRend 0.1.0"
git push origin v0.1.0
```

The release workflow validates the tag, versions, changelog, formatting, tests,
and Clippy. It then builds validated Windows x64, Linux x64, and Android ARM64
packages, creates platform archives, generates `SHA256SUMS`, and publishes a
GitHub Release. Crates.io publishing remains intentionally disabled.

If a release job fails, fix the source and create a new version. Do not move or
replace a published release tag.