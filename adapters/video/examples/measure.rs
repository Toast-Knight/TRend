use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

use trend::{CellFrame, FitMode, RenderRequest, Renderer, Viewport};
use trend_video::{DecodeLimits, VideoDecoder};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: measure <video-file>")?;
    let path = Path::new(&path);
    let limits = DecodeLimits::default();
    let mut decoder = VideoDecoder::open(path, limits)?;
    let info = decoder.info().clone();
    let frame_step =
        info.nominal_frame_rate
            .map_or(Duration::from_millis(33), |(numerator, denominator)| {
                Duration::from_secs_f64(f64::from(denominator) / f64::from(numerator))
            });

    let decode_started = Instant::now();
    let mut target = Duration::ZERO;
    while target <= info.duration {
        decoder.frame_for_time(target)?;
        target = target.saturating_add(frame_step);
    }
    let decode_elapsed = decode_started.elapsed();
    let decoded_frames = decoder.decoded_frames();

    let seek_started = Instant::now();
    decoder.seek(info.duration / 2)?;
    decoder.frame_for_time(info.duration / 2)?;
    let seek_elapsed = seek_started.elapsed();

    let frame = decoder
        .frame_for_time(info.duration / 2)?
        .ok_or("decoder returned no frame after seek")?;
    let renderer = Renderer::new();
    let mut cells = CellFrame::default();
    let render_started = Instant::now();
    let mut rendered_frames = 0_u64;
    for generation in 1..=100 {
        renderer.render_into(
            frame.view()?,
            RenderRequest {
                viewport: Viewport {
                    width: 80,
                    height: 24,
                },
                fit: FitMode::Contain,
                generation,
            },
            &mut cells,
        )?;
        rendered_frames += 1;
    }
    let render_elapsed = render_started.elapsed();

    let resize_started = Instant::now();
    for (generation, (width, height)) in [(101, (40, 12)), (102, (120, 36)), (103, (80, 24))] {
        renderer.render_into(
            frame.view()?,
            RenderRequest {
                viewport: Viewport { width, height },
                fit: FitMode::Contain,
                generation,
            },
            &mut cells,
        )?;
    }
    let resize_elapsed = resize_started.elapsed();

    println!("file={}", path.display());
    println!("resolution={}x{}", info.width, info.height);
    println!("duration_ms={}", info.duration.as_millis());
    println!("decoded_frames={decoded_frames}");
    println!("decode_fps={:.2}", rate(decoded_frames, decode_elapsed));
    println!(
        "render_fps_80x24={:.2}",
        rate(rendered_frames, render_elapsed)
    );
    println!(
        "seek_latency_ms={:.3}",
        seek_elapsed.as_secs_f64() * 1_000.0
    );
    println!(
        "resize_3_latency_ms={:.3}",
        resize_elapsed.as_secs_f64() * 1_000.0
    );
    println!("buffered_frames={}", decoder.buffered_frames());
    println!("retained_bytes={}", decoder.retained_bytes());
    println!("dropped_frames={}", decoder.dropped_frames());
    println!("seek_discarded_frames={}", decoder.seek_discarded_frames());
    println!("audio_present={}", info.audio_present);
    Ok(())
}

fn rate(count: u64, elapsed: Duration) -> f64 {
    count as f64 / elapsed.as_secs_f64()
}
