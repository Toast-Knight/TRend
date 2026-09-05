use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::fs;
use std::io::{BufReader, Cursor};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use image_webp::WebPDecoder;
use trend::{CellFrame, FitMode, PixelFormat, PixelFrameView, RenderRequest, Renderer, Viewport};

struct CountingAllocator;

static CURRENT_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                CURRENT_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn record_allocation(bytes: usize) {
    ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
    let current = CURRENT_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_BYTES.fetch_max(current, Ordering::Relaxed);
}

#[derive(Debug)]
struct Measurement {
    width: u32,
    height: u32,
    decoded_bytes: usize,
    has_alpha: bool,
    is_lossy: bool,
    metadata_us: f64,
    decode_us: f64,
    allocations: usize,
    peak_extra_bytes: usize,
    pixels: Vec<u8>,
}

fn decode(bytes: &[u8], memory_limit: usize) -> Result<Measurement, Box<dyn Error>> {
    let baseline_bytes = CURRENT_BYTES.load(Ordering::Relaxed);
    let baseline_allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_bytes, Ordering::Relaxed);

    let metadata_start = Instant::now();
    let mut decoder = WebPDecoder::new(BufReader::new(Cursor::new(bytes)))?;
    let metadata_elapsed = metadata_start.elapsed();
    decoder.set_memory_limit(memory_limit);

    let (width, height) = decoder.dimensions();
    if decoder.is_animated() {
        return Err("animated WebP is outside the static spike".into());
    }
    let has_alpha = decoder.has_alpha();
    let is_lossy = decoder.is_lossy();
    let pixel_count = usize::try_from(u64::from(width) * u64::from(height))?;
    let rgba_len = pixel_count.checked_mul(4).ok_or("RGBA size overflow")?;
    let decoded_len = decoder
        .output_buffer_size()
        .ok_or("decoded size overflow")?;
    let expected_len = pixel_count * if has_alpha { 4 } else { 3 };
    if decoded_len != expected_len {
        return Err("unexpected decoded buffer size".into());
    }

    let mut pixels = vec![0; rgba_len];
    let decode_start = Instant::now();
    decoder.read_image(&mut pixels[..decoded_len])?;
    let decode_elapsed = decode_start.elapsed();

    if !has_alpha {
        for index in (0..pixel_count).rev() {
            let source = index * 3;
            let destination = index * 4;
            let red = pixels[source];
            let green = pixels[source + 1];
            let blue = pixels[source + 2];
            pixels[destination..destination + 4].copy_from_slice(&[red, green, blue, 255]);
        }
    }

    let peak_bytes = PEAK_BYTES.load(Ordering::Relaxed);
    Ok(Measurement {
        width,
        height,
        decoded_bytes: decoded_len,
        has_alpha,
        is_lossy,
        metadata_us: metadata_elapsed.as_secs_f64() * 1_000_000.0,
        decode_us: decode_elapsed.as_secs_f64() * 1_000_000.0,
        allocations: ALLOCATION_COUNT.load(Ordering::Relaxed) - baseline_allocations,
        peak_extra_bytes: peak_bytes.saturating_sub(baseline_bytes),
        pixels,
    })
}

fn verify_borrowed_render(measurement: &Measurement) -> Result<(), Box<dyn Error>> {
    let view = PixelFrameView::new(
        &measurement.pixels,
        measurement.width,
        measurement.height,
        usize::try_from(measurement.width)? * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )?;
    let address = view.pixels().as_ptr();
    let mut cells = CellFrame::default();
    Renderer::new().render_into(
        view,
        RenderRequest {
            viewport: Viewport {
                width: 80,
                height: 24,
            },
            fit: FitMode::Contain,
            generation: 1,
        },
        &mut cells,
    )?;
    if address != measurement.pixels.as_ptr() || cells.cells().is_empty() {
        return Err("borrowed core render validation failed".into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let paths = std::env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        return Err("usage: webp_spike <file.webp>...".into());
    }

    println!(
        "file,width,height,kind,alpha,decoded_bytes,metadata_us,decode_us,allocations,peak_extra_bytes,one_byte_limit"
    );
    for path in paths {
        let bytes = fs::read(&path)?;
        let measurement = decode(&bytes, 8 * 1024 * 1024)?;
        verify_borrowed_render(&measurement)?;
        if measurement.has_alpha
            && !measurement
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] < 255)
        {
            return Err(
                format!("{path}: alpha was declared but no transparent pixel decoded").into(),
            );
        }
        if !measurement.has_alpha
            && measurement
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 255)
        {
            return Err(format!("{path}: RGB conversion produced non-opaque alpha").into());
        }

        let one_byte_limit = if decode(&bytes, 1).is_err() {
            "rejected"
        } else {
            "ignored"
        };
        println!(
            "{path},{},{},{},{},{},{:.1},{:.1},{},{},{}",
            measurement.width,
            measurement.height,
            if measurement.is_lossy {
                "lossy"
            } else {
                "lossless"
            },
            measurement.has_alpha,
            measurement.decoded_bytes,
            measurement.metadata_us,
            measurement.decode_us,
            measurement.allocations,
            measurement.peak_extra_bytes,
            one_byte_limit
        );
    }
    Ok(())
}
