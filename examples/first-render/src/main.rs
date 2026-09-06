use std::env;
use std::fs;
use std::io::{self, Write};
use std::time::Duration;

use trend::{
    AnsiPresenter, CellFrame, FitMode, RenderRequest, Renderer, TerminalColorMode, Viewport,
};
use trend_image::{DecodeLimits, decode_png};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os()
        .nth(1)
        .ok_or("usage: cargo run -p trend-first-render -- <image.png>")?;
    let image = decode_png(&fs::read(path)?, DecodeLimits::default())?;
    let pixels = image.view(Duration::ZERO)?;

    let mut cells = CellFrame::default();
    Renderer::new().render_into(
        pixels,
        RenderRequest {
            viewport: Viewport {
                width: 40,
                height: 12,
            },
            fit: FitMode::Contain,
            generation: 1,
        },
        &mut cells,
    )?;

    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    io::stdout().write_all(presenter.present(&cells))?;
    Ok(())
}
