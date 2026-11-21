# Repository audit and cleanup plan

This document records the repository state before the developer-experience
cleanup. It distinguishes authoritative source from generated output and keeps
the migration reviewable.

## Authoritative source

- `src/`: dependency-free `trend` renderer, terminal presenter, timing, cache,
  queue, resize, and C ABI implementation.
- `include/trend.h`: public C ABI declaration matching `src/ffi.rs`.
- `adapters/image`, `adapters/animation`, `adapters/video`, and
  `adapters/media`: separate decoding and media-abstraction crates.
- `tools/test-viewer` and `tools/python-tui-bridge`: user-facing executables.
- `tests/`: Rust integration and regression tests.
- `Test runs Envs/Materials`: checked-in media fixtures and fixture provenance.
- `Test runs Envs/Python TUI` and `Test runs Envs/Python Video Player`:
  standard-library Python integration examples with tests.
- `Test runs Envs/UI`: Windows-only native integration laboratory and Cargo
  workspace member.
- Root Markdown files, build/test batch files, and `.github/workflows/ci.yml`:
  current documentation, local automation, and CI policy.

## Generated or packaged output

- `target/`: Cargo output; generated and ignored.
- `builds/`: platform package output; generated and ignored.
- `Builds Organized/`: locally assembled Windows package; generated but not
  currently ignored.
- Python `__pycache__/` directories: generated and ignored.
- `android-ndk-r27d/`: local toolchain payload matched by `/android-ndk-*` in
  `.gitignore`; it is not project source.

Packaged copies of binaries, headers, licenses, and sample media are generated
from the authoritative files above. They must not become alternate sources.

## Verified cleanup map

| Current path | Classification | Destination |
| --- | --- | --- |
| `Test runs Envs/Materials` | test fixtures | `tests/fixtures/media` |
| `Test runs Envs/Python TUI` | example + integration tests | `examples/python-tui` |
| `Test runs Envs/Python Video Player` | example + integration tests | `examples/python-video-player` |
| `Test runs Envs/UI` | development tool | `tools/ui-lab` |
| `Test runs Envs/generate_jpeg_fixtures.ps1` | fixture utility | `scripts/fixtures/` |
| `Test runs Envs/generate_webp_spike_fixtures.py` | fixture utility | `scripts/fixtures/` |
| root build/test batch files | repository automation | `scripts/` |
| `Builds Organized/` and `builds/` | generated distribution output | `dist/` |

Moves are applied only after all references in Cargo manifests, CI, scripts,
documentation, and Python code have been identified. The untracked hash-named
GIF is not deleted during this cleanup because its provenance is not yet known.

## Deliberately retained architecture

- The root package remains the `trend` crate. Moving it into `crates/trend`
  would create widespread path churn without improving its API boundary.
- Decoder crates remain under `adapters/`; this accurately communicates their
  optional relationship to the dependency-free core.
- Rendering, decoding, media abstraction, terminal presentation, FFI, and
  application tooling remain separate.
- No renderer algorithm or public ABI is changed as part of repository layout
  work.

## Staged implementation

1. Add discoverable `--help`/`--version` behavior and tests to user-facing
   executables.
2. Move the UI lab, Python examples, fixtures, and fixture utilities; update all
   references immediately and run focused tests.
3. Add the first-render and positioned-overlay examples.
4. Establish the `docs/` hierarchy and document the bridge protocol and each
   supported platform without overstating validation.
5. Replace ad hoc package folders with deterministic `dist/windows` and
   `dist/linux` layouts plus package validation.
6. Add repository validation and CI coverage for stale paths and documented
   artifacts.
7. Run formatting, workspace checks/tests, strict Clippy, platform scripts,
   bridge checks, media probes, and the overlay example.

## Safety rules

- Preserve dirty user changes and inspect moved files before editing them.
- Do not delete an experiment merely because it is not in the default path.
- Do not commit generated binaries as source.
- Do not claim runtime support where only cross-compilation has been verified.
- Validate each stage before beginning the next one.