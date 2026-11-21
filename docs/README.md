# TRend documentation

## Using TRend

- [Getting started](getting-started.md): install, first program, first picture.
- [Usage](usage.md): drawing your own pixels, showing media, embedding from Rust, C and Python.
- [Rust API reference](api.md): every public type and feature flag.
- [Architecture](architecture.md): what TRend owns and what your program owns.
- [Read-back recipes](readback-recipes.md): handing frames from wgpu, OpenGL and CEF to TRend.
- [C ABI](ffi.md) and the [bridge protocol](bridge-protocol.md) used by the Python examples.
- Platform notes: [Windows](platforms/windows.md), [Linux](platforms/linux.md),
  [Android](platforms/android.md).

Each optional crate has its own README: [`trend-term`](../adapters/term/README.md),
[`trend-3d`](../adapters/3d/README.md), [`trend-text`](../adapters/text/README.md),
[`trend-video`](../adapters/video/README.md), [`trend-media`](../adapters/media/README.md).

## Working on TRend

- [Testing](testing.md), [benchmarks](benchmarks.md), [packaging](packaging.md) and
  [releasing](releasing.md).
- [Development status](development-status.md): what is verified, recent changes, next work.
- [Roadmap](architecture-roadmap.md): milestones done and proposed.

## Design notes

Proposals for work that is planned or partly built:

- [`trend-term` design](design/trend-term-design.md)
- [C ABI version 2 design](design/ffi-v2-design.md)

## History

Records of how the project got here. They describe the repository as it was when they were
written and are not kept up to date:

- [Architecture audit](history/architecture-audit.md) and
  [architecture proposal](history/architecture-proposal.md)
- [Phase 0 plan](history/phase-0-plan.md)
- [Repository audit and cleanup plan](history/repository-audit.md)
