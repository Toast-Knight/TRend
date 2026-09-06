# Packaging

Generated artifacts have one authoritative root: `dist/`. Delete and recreate
packages from source; do not hand-edit or commit them.

## Windows x64

```powershell
.\scripts\build-windows.bat
```

This builds the all-feature release workspace, copies the declared runtime,
headers, import library, examples, samples, notices, and docs into
`dist/windows`, writes a sorted `manifest.sha256`, and runs package probes.
Run `scripts/validate-package.ps1` to validate an existing package.

## Linux x64

```bash
./scripts/build-linux.sh
```

This produces and smoke-tests `dist/linux`. Optional video features still use
the compatible system libvpx/OpenH264 runtime on the destination host.

## Android ARM64

```powershell
.\scripts\build-android.bat
```

The Android package is generated under `dist/android/aarch64-linux-android`.
See [platforms/android.md](platforms/android.md) for NDK setup and the validated
scope. Android video playback is not implemented.

Each package includes its own README. SHA-256 manifests make file contents
repeatable; compiler and platform metadata can still affect produced binaries.
Tagged release archives and their top-level checksums are produced by the
process in [releasing.md](releasing.md).