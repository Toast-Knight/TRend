use std::error::Error;
use std::ffi::{OsStr, c_void};
use std::fs;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use trend::{Cell, CellFrame, Color, FitMode, RenderOptions, RenderRequest, Renderer, Viewport};
use trend_animation::{DecodeLimits as AnimationLimits, DecodedAnimation, decode_gif};
use trend_image::{DecodeLimits as ImageLimits, DecodedImage, decode_jpeg, decode_png};

use crate::integration::{PreviewController, Quality};

type Hwnd = isize;
type Hdc = isize;
type Hinstance = isize;
type Hbrush = isize;
type Hfont = isize;
type Hcursor = isize;
type Lresult = isize;

const CW_USEDEFAULT: i32 = i32::MIN;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const CS_HREDRAW: u32 = 0x0002;
const CS_VREDRAW: u32 = 0x0001;
const COLOR_WINDOW: isize = 5;
const IDC_ARROW: *const u16 = 32512_usize as *const u16;
const SW_SHOW: i32 = 5;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_KEYDOWN: u32 = 0x0100;
const WM_TIMER: u32 = 0x0113;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_APP_PREVIEW: u32 = 0x8001;
const TME_LEAVE: u32 = 0x0000_0002;
const TRANSPARENT: i32 = 1;
const OPAQUE: i32 = 2;
const FW_NORMAL: i32 = 400;
const DEFAULT_CHARSET: u32 = 1;
const OUT_DEFAULT_PRECIS: u32 = 0;
const CLIP_DEFAULT_PRECIS: u32 = 0;
const CLEARTYPE_QUALITY: u32 = 5;
const FIXED_PITCH: u32 = 1;
const FF_MODERN: u32 = 48;
const VK_ESCAPE: usize = 0x1B;
const VK_SPACE: usize = 0x20;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Rect {
    fn width(self) -> i32 {
        (self.right - self.left).max(0)
    }

    fn height(self) -> i32 {
        (self.bottom - self.top).max(0)
    }

    fn contains(self, point: Point) -> bool {
        point.x >= self.left && point.x < self.right && point.y >= self.top && point.y < self.bottom
    }
}

#[repr(C)]
struct PaintStruct {
    hdc: Hdc,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct Message {
    window: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}

#[repr(C)]
struct WindowClass {
    style: u32,
    window_proc: unsafe extern "system" fn(Hwnd, u32, usize, isize) -> Lresult,
    class_extra: i32,
    window_extra: i32,
    instance: Hinstance,
    icon: isize,
    cursor: Hcursor,
    background: Hbrush,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct TrackMouseEvent {
    size: u32,
    flags: u32,
    window: Hwnd,
    hover_time: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WindowClass) -> u16;
    fn CreateWindowExW(
        extended_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: isize,
        instance: Hinstance,
        parameter: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> Lresult;
    fn DestroyWindow(window: Hwnd) -> i32;
    fn ShowWindow(window: Hwnd, command: i32) -> i32;
    fn UpdateWindow(window: Hwnd) -> i32;
    fn GetMessageW(message: *mut Message, window: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(message: *const Message) -> i32;
    fn DispatchMessageW(message: *const Message) -> Lresult;
    fn PostQuitMessage(exit_code: i32);
    fn BeginPaint(window: Hwnd, paint: *mut PaintStruct) -> Hdc;
    fn EndPaint(window: Hwnd, paint: *const PaintStruct) -> i32;
    fn GetClientRect(window: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(window: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn TrackMouseEvent(event: *mut TrackMouseEvent) -> i32;
    fn SetTimer(window: Hwnd, id: usize, milliseconds: u32, callback: *const c_void) -> usize;
    fn KillTimer(window: Hwnd, id: usize) -> i32;
    fn LoadCursorW(instance: Hinstance, cursor_name: *const u16) -> Hcursor;
    fn FillRect(hdc: Hdc, rect: *const Rect, brush: Hbrush) -> i32;
    fn DrawTextW(hdc: Hdc, text: *const u16, length: i32, rect: *mut Rect, format: u32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn DeleteObject(object: isize) -> i32;
    fn GetStockObject(object: i32) -> isize;
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        output_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> Hfont;
    fn SelectObject(hdc: Hdc, object: isize) -> isize;
    fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
    fn SetBkColor(hdc: Hdc, color: u32) -> u32;
    fn SetDCBrushColor(hdc: Hdc, color: u32) -> u32;
    fn SetTextColor(hdc: Hdc, color: u32) -> u32;
    fn TextOutW(hdc: Hdc, x: i32, y: i32, text: *const u16, length: i32) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hinstance;
}

const DT_CENTER: u32 = 0x0001;
const DT_VCENTER: u32 = 0x0004;
const DT_SINGLELINE: u32 = 0x0020;
const DC_BRUSH: i32 = 18;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaKind {
    Png,
    Jpeg,
    Gif,
    Video,
}

struct Card {
    title: &'static str,
    kind: MediaKind,
    path: PathBuf,
    rect: Rect,
    frame: CellFrame,
    state: String,
}

enum StaticMedia {
    Image(DecodedImage),
    Animation(DecodedAnimation),
}

struct Lab {
    window: Hwnd,
    cards: Vec<Card>,
    static_media: Vec<Option<StaticMedia>>,
    renderer: Renderer,
    preview: PreviewController,
    quality: Quality,
    fit: FitMode,
    hovered: Option<usize>,
    preview_rect: Rect,
    client: Rect,
    started: Instant,
    font: Hfont,
    small_font: Hfont,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let class_name = wide("TRendUiLabWindow");
    let title = wide("TRend UI Laboratory - Media Browser");
    let instance = unsafe { GetModuleHandleW(null()) };
    let class = WindowClass {
        style: CS_HREDRAW | CS_VREDRAW,
        window_proc,
        class_extra: 0,
        window_extra: 0,
        instance,
        icon: 0,
        cursor: unsafe { LoadCursorW(0, IDC_ARROW) },
        background: (COLOR_WINDOW + 1) as Hbrush,
        menu_name: null(),
        class_name: class_name.as_ptr(),
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1180,
            820,
            0,
            0,
            instance,
            null_mut(),
        )
    };
    if window == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let mut lab = Box::new(Lab::new(window)?);
    unsafe {
        WINDOW_STATE = (&mut *lab) as *mut Lab;
        SetTimer(window, 1, 33, null());
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);
    }
    let mut message = Message {
        window: 0,
        message: 0,
        wparam: 0,
        lparam: 0,
        time: 0,
        point: Point::default(),
        private: 0,
    };
    while unsafe { GetMessageW(&mut message, 0, 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    unsafe {
        WINDOW_STATE = null_mut();
    }
    Ok(())
}

static mut WINDOW_STATE: *mut Lab = null_mut();

unsafe extern "system" fn window_proc(
    window: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> Lresult {
    let state = unsafe { WINDOW_STATE.as_mut() };
    match message {
        WM_PAINT => {
            if let Some(lab) = state {
                lab.paint();
            } else {
                let mut paint = empty_paint();
                unsafe {
                    BeginPaint(window, &mut paint);
                    EndPaint(window, &paint);
                }
            }
            0
        }
        WM_SIZE => {
            if let Some(lab) = state {
                lab.resize();
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(lab) = state {
                let point = Point {
                    x: low_word(lparam) as i16 as i32,
                    y: high_word(lparam) as i16 as i32,
                };
                lab.mouse_move(point);
                let mut event = TrackMouseEvent {
                    size: size_of::<TrackMouseEvent>() as u32,
                    flags: TME_LEAVE,
                    window,
                    hover_time: 0,
                };
                unsafe { TrackMouseEvent(&mut event) };
            }
            0
        }
        WM_MOUSELEAVE => {
            if let Some(lab) = state {
                lab.set_hover(None);
            }
            0
        }
        WM_TIMER => {
            if let Some(lab) = state {
                lab.tick();
            }
            0
        }
        WM_APP_PREVIEW => {
            unsafe { InvalidateRect(window, null(), 0) };
            0
        }
        WM_KEYDOWN => {
            if let Some(lab) = state {
                lab.key(wparam);
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(window) };
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(window, 1);
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

impl Lab {
    fn new(window: Hwnd) -> Result<Self, Box<dyn Error>> {
        let materials = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Materials");
        let specs = [
            ("PNG image", MediaKind::Png, "ColorTest.png"),
            ("JPEG image", MediaKind::Jpeg, "JpegPhotoScene.jpg"),
            ("Animated GIF", MediaKind::Gif, "ColorTest.gif"),
            ("Video preview A", MediaKind::Video, "ColorTest.mp4"),
            ("Video preview B", MediaKind::Video, "ColorTest.mp4"),
        ];
        let mut cards = Vec::new();
        let mut static_media = Vec::new();
        for (title, kind, file) in specs {
            let path = materials.join(file);
            let decoded: Result<Option<StaticMedia>, String> = match kind {
                MediaKind::Png => {
                    fs::read(&path)
                        .map_err(|error| error.to_string())
                        .and_then(|bytes| {
                            decode_png(&bytes, ImageLimits::default())
                                .map(StaticMedia::Image)
                                .map(Some)
                                .map_err(|error| error.to_string())
                        })
                }
                MediaKind::Jpeg => {
                    fs::read(&path)
                        .map_err(|error| error.to_string())
                        .and_then(|bytes| {
                            decode_jpeg(&bytes, ImageLimits::default())
                                .map(StaticMedia::Image)
                                .map(Some)
                                .map_err(|error| error.to_string())
                        })
                }
                MediaKind::Gif => {
                    fs::read(&path)
                        .map_err(|error| error.to_string())
                        .and_then(|bytes| {
                            decode_gif(&bytes, AnimationLimits::default(), Duration::ZERO)
                                .map(StaticMedia::Animation)
                                .map(Some)
                                .map_err(|error| error.to_string())
                        })
                }
                MediaKind::Video => Ok(None),
            };
            let (media, state) = match decoded {
                Ok(media) => {
                    let state = if kind == MediaKind::Video {
                        "hover to start"
                    } else {
                        "ready"
                    };
                    (media, state.into())
                }
                Err(error) => (None, format!("load error: {error}")),
            };
            cards.push(Card {
                title,
                kind,
                path,
                rect: Rect::default(),
                frame: CellFrame::default(),
                state,
            });
            static_media.push(media);
        }
        let font = create_font(14, "Cascadia Mono");
        let small_font = create_font(12, "Segoe UI");
        let mut lab = Self {
            window,
            cards,
            static_media,
            renderer: Renderer::new(),
            preview: PreviewController::new(window, WM_APP_PREVIEW),
            quality: Quality::Balanced,
            fit: FitMode::Contain,
            hovered: None,
            preview_rect: Rect::default(),
            client: Rect::default(),
            started: Instant::now(),
            font,
            small_font,
        };
        lab.resize();
        Ok(lab)
    }

    fn resize(&mut self) {
        unsafe { GetClientRect(self.window, &mut self.client) };
        let margin = 18;
        let gap = 14;
        let top = 78;
        let available = (self.client.width() - margin * 2 - gap * 2).max(300);
        let card_width = available / 3;
        let card_height = ((self.client.height() - 250) / 2).clamp(135, 245);
        for (index, card) in self.cards.iter_mut().enumerate() {
            let row = index / 3;
            let column = index % 3;
            let left = margin + column as i32 * (card_width + gap);
            let card_top = top + row as i32 * (card_height + gap);
            card.rect = Rect {
                left,
                top: card_top,
                right: left + card_width,
                bottom: card_top + card_height,
            };
        }
        let preview_top = top + 2 * (card_height + gap) + 4;
        self.preview_rect = Rect {
            left: margin,
            top: preview_top,
            right: self.client.right - margin,
            bottom: self.client.bottom - 18,
        };
        self.render_static_cards(false);
        self.submit_preview();
        unsafe { InvalidateRect(self.window, null(), 1) };
    }

    fn render_static_cards(&mut self, animations_only: bool) {
        let now = self.started.elapsed();
        for index in 0..self.cards.len() {
            let Some(media) = self.static_media[index].as_ref() else {
                continue;
            };
            if animations_only && !matches!(media, StaticMedia::Animation(_)) {
                continue;
            }
            let viewport = viewport_for(self.cards[index].rect, 9, 18);
            let source = match media {
                StaticMedia::Image(image) => image
                    .view(Duration::ZERO)
                    .map_err(|error| error.to_string()),
                StaticMedia::Animation(animation) => animation
                    .current_frame(now)
                    .map_err(|error| error.to_string()),
            };
            match source {
                Ok(source) => {
                    let start = Instant::now();
                    let result = self.renderer.render_with_options_into(
                        source,
                        RenderRequest {
                            viewport,
                            fit: self.fit,
                            generation: now.as_millis() as u64,
                        },
                        RenderOptions {
                            sampling: self.quality.sampling(),
                            ..RenderOptions::default()
                        },
                        &mut self.cards[index].frame,
                    );
                    self.cards[index].state = match result {
                        Ok(()) => format!("render {:.2} ms", start.elapsed().as_secs_f64() * 1e3),
                        Err(error) => format!("render error: {error}"),
                    };
                }
                Err(error) => self.cards[index].state = format!("frame error: {error}"),
            }
        }
    }

    fn mouse_move(&mut self, point: Point) {
        let hovered = self
            .cards
            .iter()
            .enumerate()
            .find_map(|(index, card)| card.rect.contains(point).then_some(index));
        self.set_hover(hovered);
    }

    fn set_hover(&mut self, hovered: Option<usize>) {
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        self.submit_preview();
        unsafe { InvalidateRect(self.window, null(), 0) };
    }

    fn submit_preview(&self) {
        let path = self.hovered.and_then(|index| {
            (self.cards[index].kind == MediaKind::Video).then(|| self.cards[index].path.clone())
        });
        self.preview.update(
            path,
            viewport_for(self.preview_rect, 9, 18),
            self.quality,
            self.fit,
        );
    }

    fn tick(&mut self) {
        self.render_static_cards(true);
        unsafe { InvalidateRect(self.window, null(), 0) };
    }

    fn key(&mut self, key: usize) {
        match key {
            VK_ESCAPE => unsafe {
                DestroyWindow(self.window);
            },
            VK_SPACE => self.set_hover(None),
            0x31 | 0x61 => self.quality = Quality::Fast,
            0x32 | 0x62 => self.quality = Quality::Balanced,
            0x33 | 0x63 => self.quality = Quality::High,
            0x46 => self.fit = FitMode::Contain,
            0x53 => self.fit = FitMode::Stretch,
            _ => return,
        }
        self.render_static_cards(false);
        self.submit_preview();
        unsafe { InvalidateRect(self.window, null(), 1) };
    }

    fn paint(&mut self) {
        let mut paint = empty_paint();
        let hdc = unsafe { BeginPaint(self.window, &mut paint) };
        fill(hdc, self.client, rgb(18, 21, 24));
        select(hdc, self.small_font);
        text(
            hdc,
            20,
            16,
            "TRend UI Laboratory",
            rgb(238, 240, 232),
            TRANSPARENT,
            0,
        );
        text(
            hdc,
            20,
            43,
            &format!(
                "Hover a video | 1/2/3 {} | F contain / S stretch | Space stop | Esc close",
                self.quality.name()
            ),
            rgb(140, 200, 188),
            TRANSPARENT,
            0,
        );
        for (index, card) in self.cards.iter().enumerate() {
            self.paint_card(hdc, index, card);
        }
        self.paint_preview(hdc);
        unsafe { EndPaint(self.window, &paint) };
    }

    fn paint_card(&self, hdc: Hdc, index: usize, card: &Card) {
        let hovered = self.hovered == Some(index);
        fill(
            hdc,
            card.rect,
            if hovered {
                rgb(42, 58, 55)
            } else {
                rgb(31, 35, 39)
            },
        );
        select(hdc, self.small_font);
        text(
            hdc,
            card.rect.left + 10,
            card.rect.top + 8,
            card.title,
            rgb(238, 240, 232),
            TRANSPARENT,
            0,
        );
        text(
            hdc,
            card.rect.left + 10,
            card.rect.bottom - 23,
            if card.kind == MediaKind::Video && hovered {
                "preview active"
            } else {
                &card.state
            },
            rgb(140, 200, 188),
            TRANSPARENT,
            0,
        );
        if card.kind == MediaKind::Video {
            let mut center = Rect {
                left: card.rect.left + 8,
                top: card.rect.top + 32,
                right: card.rect.right - 8,
                bottom: card.rect.bottom - 30,
            };
            let label = wide(if hovered { "PLAYING >" } else { "VIDEO >" });
            unsafe {
                SetTextColor(hdc, rgb(220, 177, 77));
                SetBkMode(hdc, TRANSPARENT);
                DrawTextW(
                    hdc,
                    label.as_ptr(),
                    (label.len() - 1) as i32,
                    &mut center,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
            }
        } else {
            draw_cell_frame(hdc, card.rect, &card.frame, self.font);
        }
    }

    fn paint_preview(&self, hdc: Hdc) {
        fill(hdc, self.preview_rect, rgb(8, 10, 12));
        self.preview.with_published(|published| {
            draw_cell_frame(hdc, self.preview_rect, &published.frame, self.font);
            published.painted_sequence = published.sequence;
            select(hdc, self.small_font);
            let metrics = &published.metrics;
            let status = format!(
                "{} | {} | decode {:.2} ms | render {:.2} ms | decoded {} dropped {} | stale {} replaced {} | decoder {:.2} MiB cells {:.1} KiB | active decoders {}",
                metrics.state,
                metrics.source,
                metrics.decode_us / 1_000.0,
                metrics.render_us / 1_000.0,
                metrics.decoded_frames,
                metrics.decoder_dropped_frames,
                metrics.stale_renders,
                metrics.replaced_publications,
                metrics.retained_decoder_bytes as f64 / 1_048_576.0,
                metrics.cell_bytes as f64 / 1_024.0,
                metrics.active_decoders,
            );
            text(
                hdc,
                self.preview_rect.left + 8,
                self.preview_rect.bottom - 20,
                &status,
                rgb(184, 190, 181),
                OPAQUE,
                rgb(8, 10, 12),
            );
        });
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.small_font);
        }
    }
}

fn viewport_for(rect: Rect, cell_width: i32, cell_height: i32) -> Viewport {
    Viewport {
        width: u16::try_from((rect.width() - 16).max(cell_width) / cell_width).unwrap_or(u16::MAX),
        height: u16::try_from((rect.height() - 48).max(cell_height) / cell_height)
            .unwrap_or(u16::MAX),
    }
}

fn draw_cell_frame(hdc: Hdc, rect: Rect, frame: &CellFrame, font: Hfont) {
    if frame.width() == 0 || frame.height() == 0 {
        return;
    }
    select(hdc, font);
    let cell_width = ((rect.width() - 16) / i32::from(frame.width())).max(1);
    let cell_height = ((rect.height() - 48) / i32::from(frame.height())).max(1);
    let left = rect.left + 8;
    let top = rect.top + 31;
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            let index = usize::from(y) * usize::from(frame.width()) + usize::from(x);
            draw_cell(
                hdc,
                left + i32::from(x) * cell_width,
                top + i32::from(y) * cell_height,
                cell_width,
                cell_height,
                frame.cells()[index],
            );
        }
    }
}

fn draw_cell(hdc: Hdc, x: i32, y: i32, width: i32, height: i32, cell: Cell) {
    let foreground = color_or(cell.foreground, rgb(218, 222, 215));
    let background = color_or(cell.background, rgb(8, 10, 12));
    let area = Rect {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    };
    fill(hdc, area, background);
    if cell.glyph == '▀' as u32 {
        fill(
            hdc,
            Rect {
                bottom: y + (height + 1) / 2,
                ..area
            },
            foreground,
        );
    } else if cell.glyph == '▄' as u32 {
        fill(
            hdc,
            Rect {
                top: y + height / 2,
                ..area
            },
            foreground,
        );
    } else if cell.glyph != ' ' as u32 {
        let glyph = [u16::try_from(cell.glyph).unwrap_or('?' as u16)];
        unsafe {
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, foreground);
            TextOutW(hdc, x, y, glyph.as_ptr(), 1);
        }
    }
}

fn fill(hdc: Hdc, rect: Rect, color: u32) {
    unsafe {
        SetDCBrushColor(hdc, color);
        FillRect(hdc, &rect, GetStockObject(DC_BRUSH));
    }
}

fn text(hdc: Hdc, x: i32, y: i32, value: &str, color: u32, mode: i32, background: u32) {
    let value = wide(value);
    unsafe {
        SetTextColor(hdc, color);
        SetBkMode(hdc, mode);
        if mode == OPAQUE {
            SetBkColor(hdc, background);
        }
        TextOutW(hdc, x, y, value.as_ptr(), (value.len() - 1) as i32);
    }
}

fn select(hdc: Hdc, object: isize) {
    unsafe {
        SelectObject(hdc, object);
    }
}

fn create_font(height: i32, face: &str) -> Hfont {
    let face = wide(face);
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            FW_NORMAL,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FIXED_PITCH | FF_MODERN,
            face.as_ptr(),
        )
    }
}

fn color_or(color: Color, fallback: u32) -> u32 {
    if color.kind == 0 {
        fallback
    } else {
        rgb(color.red, color.green, color.blue)
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn low_word(value: isize) -> u16 {
    value as u16
}

fn high_word(value: isize) -> u16 {
    (value as usize >> 16) as u16
}

fn empty_paint() -> PaintStruct {
    PaintStruct {
        hdc: 0,
        erase: 0,
        paint: Rect::default(),
        restore: 0,
        inc_update: 0,
        reserved: [0; 32],
    }
}
