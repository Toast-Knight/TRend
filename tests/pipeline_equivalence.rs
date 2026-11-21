//! M1 differential proof: the new pipeline reproduces the existing renderer.
//!
//! Old path: `Renderer::render_with_options_into` (sample and encode fused).
//! New path: `ImageSource` → `PixelBuffer` → ordered dither → `HalfBlock`
//! → `CellViewMut`, then the unchanged `AnsiPresenter`.
//!
//! Every case of the Phase 0 matrix must produce identical cells, and both
//! cell streams must produce identical ANSI bytes in every color mode.

mod common;

use trend::{
    AnsiPresenter, CellAspect, CellEncoder, CellFrame, HalfBlock, ImageSource, PixelBuffer,
    RasterSpec, Renderer, TerminalCapabilities, TerminalColorMode, dither,
};

fn render_new_path(
    case: &common::Case,
    pattern: &common::Pattern,
    pixels: &mut PixelBuffer,
    cells: &mut CellFrame,
) {
    let options = case.options();
    let spec = RasterSpec::new(case.viewport, HalfBlock.grid(), CellAspect::default()).unwrap();

    ImageSource::new(pattern.view())
        .with_fit(case.fit)
        .with_sampling(options.sampling)
        .with_transform(options.transform)
        .with_alpha_background(options.alpha_background)
        .render_into(&spec, pixels)
        .unwrap();
    dither::ordered_4x4_in_place(pixels, options.dither_strength);

    cells
        .resize(case.viewport.width, case.viewport.height)
        .unwrap();
    cells.set_generation(case.generation);
    HalfBlock.encode(pixels.view(), cells.view_mut()).unwrap();
}

#[test]
fn half_block_pipeline_matches_renderer_for_every_case() {
    let patterns = common::patterns();
    let cases = common::cases();
    assert_eq!(cases.len(), 2_880);

    let renderer = Renderer::new();
    let mut old_cells = CellFrame::default();
    let mut new_cells = CellFrame::default();
    let mut pixels = PixelBuffer::new(1, 1).unwrap();
    let mut nonblank_cases = 0;

    for case in &cases {
        let pattern = &patterns[case.pattern];
        renderer
            .render_with_options_into(
                pattern.view(),
                case.request(),
                case.options(),
                &mut old_cells,
            )
            .unwrap();
        render_new_path(case, pattern, &mut pixels, &mut new_cells);

        assert_eq!(
            new_cells, old_cells,
            "pipeline diverged for pattern {:?}: {case:?}",
            pattern.name
        );
        // The same comparison through the read-only view the backend side uses.
        let view = new_cells.view();
        for y in 0..view.height() {
            let start = usize::from(y) * usize::from(view.width());
            assert_eq!(
                view.row(y),
                &old_cells.cells()[start..start + usize::from(view.width())]
            );
        }
        if old_cells
            .cells()
            .iter()
            .any(|cell| *cell != trend::Cell::default())
        {
            nonblank_cases += 1;
        }
    }

    // Guard against a vacuous pass where both paths render nothing.
    assert!(
        nonblank_cases > 2_700,
        "only {nonblank_cases} cases drew anything"
    );
}

#[test]
fn half_block_pipeline_presents_identical_ansi_bytes() {
    let patterns = common::patterns();
    let renderer = Renderer::new();
    let mut old_cells = CellFrame::default();
    let mut new_cells = CellFrame::default();
    let mut pixels = PixelBuffer::new(1, 1).unwrap();
    let modes = [
        TerminalColorMode::TrueColor,
        TerminalColorMode::Ansi256,
        TerminalColorMode::Ansi16,
        TerminalColorMode::Monochrome,
    ];
    let capabilities = |color| TerminalCapabilities {
        color,
        unicode: true,
    };
    let mut old_presenters: Vec<_> = modes
        .iter()
        .map(|&mode| AnsiPresenter::with_capabilities(capabilities(mode)))
        .collect();
    let mut new_presenters: Vec<_> = modes
        .iter()
        .map(|&mode| AnsiPresenter::with_capabilities(capabilities(mode)))
        .collect();
    let mut compared_bytes = 0;

    // Stream every case through long-lived presenters so incremental diffing,
    // geometry changes, and SGR state carry across frames exactly as in playback.
    for (index, case) in common::cases().iter().enumerate() {
        let pattern = &patterns[case.pattern];
        renderer
            .render_with_options_into(
                pattern.view(),
                case.request(),
                case.options(),
                &mut old_cells,
            )
            .unwrap();
        render_new_path(case, pattern, &mut pixels, &mut new_cells);

        for (old, new) in old_presenters.iter_mut().zip(&mut new_presenters) {
            let (old_bytes, new_bytes) = if index % 2 == 0 {
                (
                    old.present(&old_cells).to_vec(),
                    new.present(&new_cells).to_vec(),
                )
            } else {
                (
                    old.present_at(&old_cells, 4, 7).to_vec(),
                    new.present_at(&new_cells, 4, 7).to_vec(),
                )
            };
            assert_eq!(new_bytes, old_bytes, "ANSI diverged for {case:?}");
            compared_bytes += old_bytes.len();
        }
    }
    assert!(
        compared_bytes > 1_000_000,
        "only {compared_bytes} bytes compared"
    );
}
