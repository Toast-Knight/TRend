# TRend Windows x64 package

This package contains the TRend C runtime and two standalone terminal tools.

```text
bin/       runtime DLL and executables
include/   C header
lib/       MSVC import library
examples/  positioned terminal overlay client
samples/   PNG, GIF, and MP4 test media
docs/      bridge protocol
```

From this directory:

```powershell
.\bin\trend-test-viewer.exe --help
.\bin\trend-test-viewer.exe .\samples\ColorTest.png
.\bin\trend-test-viewer.exe .\samples\ColorTest.gif
.\bin\trend-test-viewer.exe .\samples\ColorTest.mp4
.\bin\trend-test-viewer.exe --probe 20x10 2 .\samples\ColorTest.mp4
.\examples\terminal-overlay\play-top-right.bat .\samples\ColorTest.gif
```

The tools include TRend statically. `bin\trend.dll`, `include\trend.h`, and
`lib\trend.dll.lib` are for C/C++ consumers. Windows Media Foundation supplies
video decoding; no FFmpeg installation is required. Install the current x64
Microsoft Visual C++ Redistributable if `VCRUNTIME140.dll` is unavailable.
Use Windows Terminal or another ANSI/VT-capable terminal.