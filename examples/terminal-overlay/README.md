# Terminal overlay

This example uses `trend-python-tui-bridge` to render an animated GIF into a
15x15 region at the top-right of the current Windows terminal.

Build the release bridge, then run the batch file from the repository root:

```powershell
cargo build -p trend-python-tui-bridge --release
.\examples\terminal-overlay\play-top-right.bat
```

Pass another GIF as the first argument:

```powershell
.\examples\terminal-overlay\play-top-right.bat "C:\media\secure.gif"
```

Press Ctrl+C to stop. The batch file delegates to the PowerShell reference
client; protocol parsing is implemented only there. See
[`../../docs/bridge-protocol.md`](../../docs/bridge-protocol.md) for the wire
format.