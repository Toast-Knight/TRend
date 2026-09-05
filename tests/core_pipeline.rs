use core::time::Duration;

use trend::{
    AnsiPresenter, CacheInsert, Cell, CellFrame, CellFrameCache, Color, FitMode, PixelFormat,
    PixelFrameView, RenderOptions, RenderRequest, RenderStatus, Renderer, ResizeCoalescer,
    TerminalColorMode, Viewport,
};

#[test]
fn rapid_resize_renders_only_latest_viewport_without_reloading_source() {
    let pixels = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let source = PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap();
    let source_address = source.pixels().as_ptr();
    let mut resizes = ResizeCoalescer::new();

    for step in 0..32 {
        resizes.submit(Viewport {
            width: 40 + step,
            height: 12 + step / 2,
        });
    }

    let pending = resizes.take_pending().unwrap();
    let mut cells = CellFrame::default();
    Renderer::new()
        .render_into(
            source,
            RenderRequest {
                viewport: pending.viewport,
                fit: FitMode::Contain,
                generation: pending.generation,
            },
            &mut cells,
        )
        .unwrap();

    assert_eq!(source.pixels().as_ptr(), source_address);
    assert_eq!((cells.width(), cells.height()), (71, 27));
    assert_eq!(cells.generation(), 32);

    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    assert!(!presenter.present(&cells).is_empty());
    assert!(presenter.present(&cells).is_empty());
}

#[test]
fn stale_resize_cancels_and_latest_frame_stays_within_cache_budget() {
    let pixels = [127_u8; 16 * 16 * 4];
    let source =
        PixelFrameView::new(&pixels, 16, 16, 64, PixelFormat::Rgba8, Duration::ZERO).unwrap();
    let mut resizes = ResizeCoalescer::new();
    let stale_generation = resizes.submit(Viewport {
        width: 40,
        height: 20,
    });
    for step in 0..24 {
        resizes.submit(Viewport {
            width: 60 + step,
            height: 20 + step / 2,
        });
    }

    let sentinel = Cell::half_block(Color::rgb(1, 2, 3), Color::rgb(4, 5, 6));
    let mut stale_staging = vec![sentinel; 40 * 20];
    let stale_status = Renderer::new()
        .render_to_slice_cancellable(
            source,
            RenderRequest {
                viewport: Viewport {
                    width: 40,
                    height: 20,
                },
                fit: FitMode::Contain,
                generation: stale_generation,
            },
            RenderOptions::default(),
            &mut stale_staging,
            || !resizes.is_current(stale_generation),
        )
        .unwrap();
    assert_eq!(stale_status, RenderStatus::Cancelled);
    assert!(stale_staging.iter().all(|cell| *cell == sentinel));

    let latest = resizes.take_pending().unwrap();
    let mut latest_frame = CellFrame::default();
    Renderer::new()
        .render_with_options_into(
            source,
            RenderRequest {
                viewport: latest.viewport,
                fit: FitMode::Contain,
                generation: latest.generation,
            },
            RenderOptions::default(),
            &mut latest_frame,
        )
        .unwrap();

    let frame_bytes = latest_frame.allocated_bytes();
    let cell_address = latest_frame.cells().as_ptr();
    let key = (1_u64, latest.viewport.width, latest.viewport.height);
    let mut cache = CellFrameCache::new(2, frame_bytes);
    assert_eq!(cache.insert(key, latest_frame), CacheInsert::Inserted);
    let cached = cache.get(&key).unwrap();

    assert_eq!(cached.cells().as_ptr(), cell_address);
    assert_eq!(cached.generation(), latest.generation);
    assert!(cache.used_bytes() <= cache.max_bytes());
    assert_eq!(cache.len(), 1);
}
