use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use trend::{
    CellFrame, FitMode, RenderOptions, RenderRequest, RenderStatus, Renderer, SamplingFilter,
    Viewport,
};
use trend_video::{DecodeLimits, VideoDecoder};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Quality {
    Fast,
    Balanced,
    High,
}

impl Quality {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fast => "FAST",
            Self::Balanced => "BALANCED",
            Self::High => "QUALITY",
        }
    }

    pub const fn sampling(self) -> SamplingFilter {
        match self {
            Self::Fast => SamplingFilter::Nearest,
            Self::Balanced => SamplingFilter::Bilinear,
            Self::High => SamplingFilter::Area,
        }
    }
}

#[derive(Clone, Debug)]
struct Request {
    generation: u64,
    path: Option<PathBuf>,
    viewport: Viewport,
    quality: Quality,
    fit: FitMode,
    shutdown: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PreviewMetrics {
    pub state: String,
    pub source: String,
    pub generation: u64,
    pub decoded_frames: u64,
    pub decoder_dropped_frames: u64,
    pub stale_renders: u64,
    pub replaced_publications: u64,
    pub decode_us: f64,
    pub render_us: f64,
    pub retained_decoder_bytes: usize,
    pub cell_bytes: usize,
    pub active_decoders: usize,
}

#[derive(Debug, Default)]
pub struct PublishedPreview {
    pub frame: CellFrame,
    pub metrics: PreviewMetrics,
    pub sequence: u64,
    pub painted_sequence: u64,
}

pub struct PreviewController {
    request: Arc<(Mutex<Request>, Condvar)>,
    generation: Arc<AtomicU64>,
    published: Arc<Mutex<PublishedPreview>>,
    worker: Option<JoinHandle<()>>,
}

impl PreviewController {
    pub fn new(notify_window: isize, notify_message: u32) -> Self {
        let generation = Arc::new(AtomicU64::new(0));
        let request = Arc::new((
            Mutex::new(Request {
                generation: 0,
                path: None,
                viewport: Viewport {
                    width: 40,
                    height: 12,
                },
                quality: Quality::Balanced,
                fit: FitMode::Contain,
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let published = Arc::new(Mutex::new(PublishedPreview::default()));
        let worker = {
            let worker_request = Arc::clone(&request);
            let worker_generation = Arc::clone(&generation);
            let worker_published = Arc::clone(&published);
            thread::Builder::new()
                .name("trend-ui-preview".into())
                .spawn(move || {
                    preview_worker(
                        worker_request,
                        worker_generation,
                        worker_published,
                        notify_window,
                        notify_message,
                    );
                })
                .expect("preview worker should start")
        };
        Self {
            request,
            generation,
            published,
            worker: Some(worker),
        }
    }

    pub fn update(
        &self,
        path: Option<PathBuf>,
        viewport: Viewport,
        quality: Quality,
        fit: FitMode,
    ) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let (lock, wake) = &*self.request;
        let mut request = lock.lock().unwrap_or_else(|error| error.into_inner());
        request.generation = generation;
        request.path = path;
        request.viewport = viewport;
        request.quality = quality;
        request.fit = fit;
        wake.notify_one();
        generation
    }

    pub fn with_published<T>(&self, read: impl FnOnce(&mut PublishedPreview) -> T) -> T {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        read(&mut published)
    }
}

impl Drop for PreviewController {
    fn drop(&mut self) {
        let (lock, wake) = &*self.request;
        let mut request = lock.lock().unwrap_or_else(|error| error.into_inner());
        request.shutdown = true;
        self.generation.fetch_add(1, Ordering::Release);
        wake.notify_one();
        drop(request);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct ActiveVideo {
    path: PathBuf,
    decoder: VideoDecoder,
    started: Instant,
    last_target: Duration,
}

fn preview_worker(
    request: Arc<(Mutex<Request>, Condvar)>,
    generation: Arc<AtomicU64>,
    published: Arc<Mutex<PublishedPreview>>,
    notify_window: isize,
    notify_message: u32,
) {
    let renderer = Renderer::new();
    let mut staging = CellFrame::default();
    let mut active: Option<ActiveVideo> = None;
    let mut stale_renders = 0_u64;
    let mut replaced_publications = 0_u64;

    loop {
        let current = {
            let (lock, wake) = &*request;
            let guard = lock.lock().unwrap_or_else(|error| error.into_inner());
            let (guard, _) = wake
                .wait_timeout(guard, Duration::from_millis(16))
                .unwrap_or_else(|error| error.into_inner());
            guard.clone()
        };
        if current.shutdown {
            break;
        }
        let Some(path) = current.path.clone() else {
            if active.take().is_some() {
                publish_state(
                    &published,
                    &mut staging,
                    PreviewMetrics {
                        state: "idle (decoder released)".into(),
                        generation: current.generation,
                        stale_renders,
                        replaced_publications,
                        ..PreviewMetrics::default()
                    },
                    notify_window,
                    notify_message,
                );
            }
            continue;
        };

        if active.as_ref().is_none_or(|video| video.path != path) {
            active = None;
            publish_status(
                &published,
                "loading decoder",
                &path,
                current.generation,
                notify_window,
                notify_message,
            );
            let open_start = Instant::now();
            match VideoDecoder::open(&path, DecodeLimits::default()) {
                Ok(decoder) => {
                    if generation.load(Ordering::Acquire) != current.generation {
                        continue;
                    }
                    active = Some(ActiveVideo {
                        path: path.clone(),
                        decoder,
                        started: Instant::now(),
                        last_target: Duration::ZERO,
                    });
                    let open_us = open_start.elapsed().as_secs_f64() * 1_000_000.0;
                    publish_status(
                        &published,
                        &format!("playing (open {:.1} ms)", open_us / 1_000.0),
                        &path,
                        current.generation,
                        notify_window,
                        notify_message,
                    );
                }
                Err(error) => {
                    publish_status(
                        &published,
                        &format!("error: {error}"),
                        &path,
                        current.generation,
                        notify_window,
                        notify_message,
                    );
                    continue;
                }
            }
        }

        let Some(video) = active.as_mut() else {
            continue;
        };
        let duration = video.decoder.info().duration;
        let mut target = video.started.elapsed();
        if !duration.is_zero() && target >= duration {
            if let Err(error) = video.decoder.restart() {
                publish_status(
                    &published,
                    &format!("restart error: {error}"),
                    &path,
                    current.generation,
                    notify_window,
                    notify_message,
                );
                active = None;
                continue;
            }
            video.started = Instant::now();
            video.last_target = Duration::ZERO;
            target = Duration::ZERO;
        } else if target < video.last_target {
            target = video.last_target;
        }

        let decode_start = Instant::now();
        let frame = match video.decoder.frame_for_time(target) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(error) => {
                publish_status(
                    &published,
                    &format!("decode error: {error}"),
                    &path,
                    current.generation,
                    notify_window,
                    notify_message,
                );
                active = None;
                continue;
            }
        };
        let decode_us = decode_start.elapsed().as_secs_f64() * 1_000_000.0;
        video.last_target = target;

        if staging
            .resize(current.viewport.width, current.viewport.height)
            .is_err()
        {
            continue;
        }
        let render_start = Instant::now();
        let status = renderer.render_to_slice_cancellable(
            match frame.view() {
                Ok(view) => view,
                Err(_) => continue,
            },
            RenderRequest {
                viewport: current.viewport,
                fit: current.fit,
                generation: current.generation,
            },
            RenderOptions {
                sampling: current.quality.sampling(),
                ..RenderOptions::default()
            },
            staging.cells_mut(),
            || generation.load(Ordering::Acquire) != current.generation,
        );
        let render_us = render_start.elapsed().as_secs_f64() * 1_000_000.0;
        if !matches!(status, Ok(RenderStatus::Complete)) {
            stale_renders += 1;
            continue;
        }
        staging.set_generation(current.generation);

        let metrics = PreviewMetrics {
            state: "playing".into(),
            source: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("video")
                .into(),
            generation: current.generation,
            decoded_frames: video.decoder.decoded_frames(),
            decoder_dropped_frames: video.decoder.dropped_frames(),
            stale_renders,
            replaced_publications,
            decode_us,
            render_us,
            retained_decoder_bytes: video.decoder.retained_bytes(),
            cell_bytes: staging.allocated_bytes(),
            active_decoders: 1,
        };
        let mut output = published.lock().unwrap_or_else(|error| error.into_inner());
        if output.sequence != output.painted_sequence {
            replaced_publications += 1;
        }
        std::mem::swap(&mut output.frame, &mut staging);
        output.metrics = metrics;
        output.sequence = output.sequence.wrapping_add(1);
        drop(output);
        post_window_message(notify_window, notify_message);
    }
}

fn publish_state(
    published: &Mutex<PublishedPreview>,
    staging: &mut CellFrame,
    metrics: PreviewMetrics,
    notify_window: isize,
    notify_message: u32,
) {
    staging.clear();
    let mut output = published.lock().unwrap_or_else(|error| error.into_inner());
    std::mem::swap(&mut output.frame, staging);
    output.metrics = metrics;
    output.sequence = output.sequence.wrapping_add(1);
    drop(output);
    post_window_message(notify_window, notify_message);
}

fn publish_status(
    published: &Mutex<PublishedPreview>,
    state: &str,
    path: &std::path::Path,
    generation: u64,
    notify_window: isize,
    notify_message: u32,
) {
    let mut output = published.lock().unwrap_or_else(|error| error.into_inner());
    output.metrics.state = state.into();
    output.metrics.source = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("video")
        .into();
    output.metrics.generation = generation;
    output.sequence = output.sequence.wrapping_add(1);
    drop(output);
    post_window_message(notify_window, notify_message);
}

#[cfg(windows)]
fn post_window_message(window: isize, message: u32) {
    unsafe extern "system" {
        fn PostMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> i32;
    }
    unsafe {
        PostMessageW(window, message, 0, 0);
    }
}

#[cfg(not(windows))]
fn post_window_message(_window: isize, _message: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_maps_to_expected_sampling() {
        assert_eq!(Quality::Fast.sampling(), SamplingFilter::Nearest);
        assert_eq!(Quality::Balanced.sampling(), SamplingFilter::Bilinear);
        assert_eq!(Quality::High.sampling(), SamplingFilter::Area);
    }

    #[test]
    fn update_replaces_pending_request_and_invalidates_generation() {
        let controller = PreviewController::new(0, 0);
        let first_generation = controller.update(
            Some(PathBuf::from("first.mp4")),
            Viewport {
                width: 30,
                height: 10,
            },
            Quality::Fast,
            FitMode::Contain,
        );
        let second_generation = controller.update(
            Some(PathBuf::from("second.mp4")),
            Viewport {
                width: 80,
                height: 24,
            },
            Quality::High,
            FitMode::Cover,
        );

        assert!(second_generation > first_generation);
        assert_eq!(
            controller.generation.load(Ordering::Acquire),
            second_generation
        );
        let request = controller
            .request
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(request.generation, second_generation);
        assert_eq!(
            request.path.as_deref(),
            Some(std::path::Path::new("second.mp4"))
        );
        assert_eq!(request.viewport.width, 80);
        assert_eq!(request.viewport.height, 24);
        assert_eq!(request.quality, Quality::High);
        assert_eq!(request.fit, FitMode::Cover);
    }

    #[cfg(windows)]
    #[test]
    fn inactive_preview_releases_real_decoder() {
        let controller = PreviewController::new(0, 0);
        let video = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Materials/ColorTest.mp4");
        controller.update(
            Some(video),
            Viewport {
                width: 40,
                height: 12,
            },
            Quality::Balanced,
            FitMode::Contain,
        );
        wait_for(&controller, |metrics| metrics.active_decoders == 1);

        controller.update(
            None,
            Viewport {
                width: 40,
                height: 12,
            },
            Quality::Balanced,
            FitMode::Contain,
        );
        wait_for(&controller, |metrics| {
            metrics.active_decoders == 0 && metrics.state == "idle (decoder released)"
        });
    }

    #[cfg(windows)]
    fn wait_for(controller: &PreviewController, predicate: impl Fn(&PreviewMetrics) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if controller.with_published(|output| predicate(&output.metrics)) {
                return;
            }
            assert!(Instant::now() < deadline, "preview worker timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
