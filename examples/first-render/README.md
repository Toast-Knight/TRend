# First render

This is the smallest complete PNG-to-terminal pipeline in the repository:

```text
PNG bytes -> decode_png -> PixelFrameView -> Renderer -> CellFrame -> AnsiPresenter -> stdout
```

From the repository root:

```powershell
cargo run -p trend-first-render -- "tests\fixtures\media\ColorTest.png"
```

The example uses a fixed 40x12 terminal viewport and TrueColor output so the
data flow remains visible without application-level terminal management.