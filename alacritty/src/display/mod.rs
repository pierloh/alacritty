//! The display subsystem including window management, font rasterization, and
//! GPU drawing.

use std::cmp;
use std::fmt::{self, Formatter};
use std::mem::{self, ManuallyDrop};
use std::num::NonZeroU32;
use std::ops::Deref;
use std::time::{Duration, Instant};

use glutin::config::GetGlConfig;
use glutin::context::{NotCurrentContext, PossiblyCurrentContext};
use glutin::display::GetGlDisplay;
use glutin::error::ErrorKind;
use glutin::prelude::*;
use glutin::surface::{Surface, SwapInterval, WindowSurface};

use log::{debug, info, warn};
use parking_lot::MutexGuard;
use serde::{Deserialize, Serialize};
use winit::dpi::PhysicalSize;
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::RawWindowHandle;
use winit::window::CursorIcon;

use crossfont::{Rasterize, Rasterizer, Size as FontSize};
use unicode_width::UnicodeWidthChar;

use alacritty_terminal::event::{EventListener, OnResize, WindowSize};
use alacritty_terminal::grid::Dimensions as TermDimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{
    self, LineDamageBounds, MAX_MULTI_CURSORS, MIN_COLUMNS, MIN_SCREEN_LINES, MultiCursorInfo,
    Term, TermDamage, TermMode,
};
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor};

use crate::config::UiConfig;
use crate::config::debug::RendererPreference;
use crate::config::font::Font;
use crate::config::window::Dimensions;
#[cfg(not(windows))]
use crate::config::window::StartupMode;
use crate::display::bell::VisualBell;
use crate::display::color::{List, Rgb};
use crate::display::content::{RenderableContent, RenderableCursor};
use crate::display::cursor::IntoRects;
use crate::display::damage::{DamageTracker, damage_y_to_viewport_y};
use crate::display::hint::{HintMatch, HintState};
use crate::display::meter::Meter;
use crate::display::window::Window;
use crate::event::{Event, EventType, Mouse, SearchState};
use crate::message_bar::{MessageBuffer, MessageType};
use crate::renderer::custom_shader::{CustomShaderPipeline, ShaderUniforms};
use crate::renderer::rects::{RenderLine, RenderLines, RenderRect};
use crate::renderer::{self, GlyphCache, Renderer, platform};
use crate::scheduler::{Scheduler, TimerId, Topic};
use crate::string::{ShortenDirection, StrShortener};

pub mod color;
pub mod content;
pub mod cursor;
pub mod hint;
pub mod window;

mod bell;
mod damage;
mod meter;

/// Label for the forward terminal search bar.
const FORWARD_SEARCH_LABEL: &str = "Search: ";

/// Label for the backward terminal search bar.
const BACKWARD_SEARCH_LABEL: &str = "Backward Search: ";

/// The character used to shorten the visible text like uri preview or search regex.
const SHORTENER: char = '…';

/// Color which is used to highlight damaged rects when debugging.
const DAMAGE_RECT_COLOR: Rgb = Rgb::new(255, 0, 255);

#[derive(Debug)]
pub enum Error {
    /// Error with window management.
    Window(window::Error),

    /// Error dealing with fonts.
    Font(crossfont::Error),

    /// Error in renderer.
    Render(renderer::Error),

    /// Error during context operations.
    Context(glutin::error::Error),
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Window(err) => err.source(),
            Error::Font(err) => err.source(),
            Error::Render(err) => err.source(),
            Error::Context(err) => err.source(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Error::Window(err) => err.fmt(f),
            Error::Font(err) => err.fmt(f),
            Error::Render(err) => err.fmt(f),
            Error::Context(err) => err.fmt(f),
        }
    }
}

impl From<window::Error> for Error {
    fn from(val: window::Error) -> Self {
        Error::Window(val)
    }
}

impl From<crossfont::Error> for Error {
    fn from(val: crossfont::Error) -> Self {
        Error::Font(val)
    }
}

impl From<renderer::Error> for Error {
    fn from(val: renderer::Error) -> Self {
        Error::Render(val)
    }
}

impl From<glutin::error::Error> for Error {
    fn from(val: glutin::error::Error) -> Self {
        Error::Context(val)
    }
}

/// Terminal size info.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
pub struct SizeInfo<T = f32> {
    /// Terminal window width.
    width: T,

    /// Terminal window height.
    height: T,

    /// Width of individual cell.
    cell_width: T,

    /// Height of individual cell.
    cell_height: T,

    /// Horizontal window padding.
    padding_x: T,

    /// Vertical window padding.
    padding_y: T,

    /// Number of lines in the viewport.
    screen_lines: usize,

    /// Number of columns in the viewport.
    columns: usize,
}

impl From<SizeInfo<f32>> for SizeInfo<u32> {
    fn from(size_info: SizeInfo<f32>) -> Self {
        Self {
            width: size_info.width as u32,
            height: size_info.height as u32,
            cell_width: size_info.cell_width as u32,
            cell_height: size_info.cell_height as u32,
            padding_x: size_info.padding_x as u32,
            padding_y: size_info.padding_y as u32,
            screen_lines: size_info.screen_lines,
            columns: size_info.screen_lines,
        }
    }
}

impl From<SizeInfo<f32>> for WindowSize {
    fn from(size_info: SizeInfo<f32>) -> Self {
        Self {
            num_cols: size_info.columns() as u16,
            num_lines: size_info.screen_lines() as u16,
            cell_width: size_info.cell_width() as u16,
            cell_height: size_info.cell_height() as u16,
        }
    }
}

impl<T: Clone + Copy> SizeInfo<T> {
    #[inline]
    pub fn width(&self) -> T {
        self.width
    }

    #[inline]
    pub fn height(&self) -> T {
        self.height
    }

    #[inline]
    pub fn cell_width(&self) -> T {
        self.cell_width
    }

    #[inline]
    pub fn cell_height(&self) -> T {
        self.cell_height
    }

    #[inline]
    pub fn padding_x(&self) -> T {
        self.padding_x
    }

    #[inline]
    pub fn padding_y(&self) -> T {
        self.padding_y
    }
}

impl SizeInfo<f32> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
        mut padding_x: f32,
        mut padding_y: f32,
        dynamic_padding: bool,
    ) -> SizeInfo {
        if dynamic_padding {
            padding_x = Self::dynamic_padding(padding_x.floor(), width, cell_width);
            padding_y = Self::dynamic_padding(padding_y.floor(), height, cell_height);
        }

        let lines = (height - 2. * padding_y) / cell_height;
        let screen_lines = cmp::max(lines as usize, MIN_SCREEN_LINES);

        let columns = (width - 2. * padding_x) / cell_width;
        let columns = cmp::max(columns as usize, MIN_COLUMNS);

        SizeInfo {
            width,
            height,
            cell_width,
            cell_height,
            padding_x: padding_x.floor(),
            padding_y: padding_y.floor(),
            screen_lines,
            columns,
        }
    }

    #[inline]
    pub fn reserve_lines(&mut self, count: usize) {
        self.screen_lines = cmp::max(self.screen_lines.saturating_sub(count), MIN_SCREEN_LINES);
    }

    /// Check if coordinates are inside the terminal grid.
    ///
    /// The padding, message bar or search are not counted as part of the grid.
    #[inline]
    pub fn contains_point(&self, x: usize, y: usize) -> bool {
        x <= (self.padding_x + self.columns as f32 * self.cell_width) as usize
            && x > self.padding_x as usize
            && y <= (self.padding_y + self.screen_lines as f32 * self.cell_height) as usize
            && y > self.padding_y as usize
    }

    /// Calculate padding to spread it evenly around the terminal content.
    #[inline]
    fn dynamic_padding(padding: f32, dimension: f32, cell_dimension: f32) -> f32 {
        padding + ((dimension - 2. * padding) % cell_dimension) / 2.
    }
}

impl TermDimensions for SizeInfo {
    #[inline]
    fn columns(&self) -> usize {
        self.columns
    }

    #[inline]
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    #[inline]
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct DisplayUpdate {
    pub dirty: bool,

    dimensions: Option<PhysicalSize<u32>>,
    cursor_dirty: bool,
    font: Option<Font>,
}

impl DisplayUpdate {
    pub fn dimensions(&self) -> Option<PhysicalSize<u32>> {
        self.dimensions
    }

    pub fn font(&self) -> Option<&Font> {
        self.font.as_ref()
    }

    pub fn cursor_dirty(&self) -> bool {
        self.cursor_dirty
    }

    pub fn set_dimensions(&mut self, dimensions: PhysicalSize<u32>) {
        self.dimensions = Some(dimensions);
        self.dirty = true;
    }

    pub fn set_font(&mut self, font: Font) {
        self.font = Some(font);
        self.dirty = true;
    }

    pub fn set_cursor_dirty(&mut self) {
        self.cursor_dirty = true;
        self.dirty = true;
    }
}

/// Tracks cursor position/color for the custom shader pipeline.
struct CursorState {
    current: [f32; 4],
    previous: [f32; 4],
    color: [f32; 4],
    start_instant: Instant,
    time_cursor_change: f32,
    visible: bool,
    multi_cursors: [[f32; 4]; MAX_MULTI_CURSORS],
    previous_multi_cursors: [[f32; 4]; MAX_MULTI_CURSORS],
    multi_cursor_types: [i32; MAX_MULTI_CURSORS],
    multi_cursor_count: i32,
    previous_multi_cursor_count: i32,
    /// App-level cursor visibility (DECTCEM), not affected by blink.
    app_visible: bool,
    /// Timestamp of the last app-level mode change (DECTCEM transition).
    time_mode_change: f32,
    /// Previous cursor color for transition detection.
    previous_color: [f32; 4],
    /// Current cursor style (ghostty-compatible int).
    cursor_style: i32,
    /// Previous cursor style for transition detection.
    previous_cursor_style: i32,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            current: [-1.0, -1.0, 0.0, 0.0],
            previous: [-1.0, -1.0, 0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
            start_instant: Instant::now(),
            time_cursor_change: 0.0,
            visible: true,
            multi_cursors: [[0.0; 4]; MAX_MULTI_CURSORS],
            previous_multi_cursors: [[0.0; 4]; MAX_MULTI_CURSORS],
            multi_cursor_types: [0; MAX_MULTI_CURSORS],
            multi_cursor_count: 0,
            previous_multi_cursor_count: 0,
            app_visible: true,
            time_mode_change: 0.0,
            previous_color: [1.0, 1.0, 1.0, 1.0],
            cursor_style: 0,
            previous_cursor_style: 0,
        }
    }
}

/// Accumulates per-second render timer statistics.
struct RenderTimerStats {
    start_instant: Instant,
    last_frame_time: f32,
    accum_time: f32,
    samples: Vec<f32>,
    display_avg: f32,
    display_p1: f32,
    display_p99: f32,
    display_mem: f32,
}

impl Default for RenderTimerStats {
    fn default() -> Self {
        Self {
            start_instant: Instant::now(),
            last_frame_time: 0.0,
            accum_time: 0.0,
            samples: Vec::with_capacity(240),
            display_avg: 0.0,
            display_p1: 0.0,
            display_p99: 0.0,
            display_mem: 0.0,
        }
    }
}

/// Get process RSS in megabytes.
fn process_memory_mb() -> f32 {
    #[cfg(target_os = "macos")]
    {
        use libc::{RUSAGE_INFO_V0, c_int, rusage_info_v0};
        unsafe {
            let mut ri = std::mem::MaybeUninit::<rusage_info_v0>::zeroed();
            let ret = libc::proc_pid_rusage(
                libc::getpid(),
                RUSAGE_INFO_V0 as c_int,
                ri.as_mut_ptr() as *mut _,
            );
            if ret == 0 {
                ri.assume_init().ri_resident_size as f32 / (1024.0 * 1024.0)
            } else {
                0.0
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::fs;
        // Read /proc/self/statm: fields are in pages. Second field is RSS.
        fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|pages| {
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as f32;
                pages as f32 * page_size / (1024.0 * 1024.0)
            })
            .unwrap_or(0.0)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        unsafe {
            let mut pmc = std::mem::MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
            let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            if GetProcessMemoryInfo(GetCurrentProcess(), pmc.as_mut_ptr(), size) != 0 {
                pmc.assume_init().WorkingSetSize as f32 / (1024.0 * 1024.0)
            } else {
                0.0
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        0.0
    }
}

/// The display wraps a window, font rasterizer, and GPU renderer.
pub struct Display {
    pub window: Window,

    pub size_info: SizeInfo,

    /// Hint highlighted by the mouse.
    pub highlighted_hint: Option<HintMatch>,
    /// Frames since hint highlight was created.
    highlighted_hint_age: usize,

    /// Hint highlighted by the vi mode cursor.
    pub vi_highlighted_hint: Option<HintMatch>,
    /// Frames since hint highlight was created.
    vi_highlighted_hint_age: usize,

    pub raw_window_handle: RawWindowHandle,

    /// UI cursor visibility for blinking.
    pub cursor_hidden: bool,

    pub visual_bell: VisualBell,

    /// Mapped RGB values for each terminal color.
    pub colors: List,

    /// State of the keyboard hints.
    pub hint_state: HintState,

    /// Unprocessed display updates.
    pub pending_update: DisplayUpdate,

    /// The renderer update that takes place only once before the actual rendering.
    pub pending_renderer_update: Option<RendererUpdate>,

    /// The ime on the given display.
    pub ime: Ime,

    /// The state of the timer for frame scheduling.
    pub frame_timer: FrameTimer,

    /// Damage tracker for the given display.
    pub damage_tracker: DamageTracker,

    /// Custom shader cursor tracking state.
    cursor_state: CursorState,

    /// Render timer statistics (debug only).
    render_timer_stats: RenderTimerStats,

    /// Font size used by the window.
    pub font_size: FontSize,

    // Mouse point position when highlighting hints.
    hint_mouse_point: Option<Point>,

    renderer: ManuallyDrop<Renderer>,
    renderer_preference: Option<RendererPreference>,

    surface: ManuallyDrop<Surface<WindowSurface>>,

    context: ManuallyDrop<PossiblyCurrentContext>,

    glyph_cache: GlyphCache,
    meter: Meter,
}

impl Display {
    pub fn new(
        window: Window,
        gl_context: NotCurrentContext,
        config: &UiConfig,
        _tabbed: bool,
    ) -> Result<Display, Error> {
        let raw_window_handle = window.raw_window_handle();

        let scale_factor = window.scale_factor as f32;
        let rasterizer = Rasterizer::new()?;

        let font_size = config.font.size().scale(scale_factor);
        debug!("Loading \"{}\" font", &config.font.normal().family);
        let font = config.font.clone().with_size(font_size);
        let mut glyph_cache = GlyphCache::new(rasterizer, &font)?;

        let metrics = glyph_cache.font_metrics();
        let (cell_width, cell_height) = compute_cell_size(config, &metrics);

        // Resize the window to account for the user configured size.
        if let Some(dimensions) = config.window.dimensions() {
            let size = window_size(config, dimensions, cell_width, cell_height, scale_factor);
            window.request_inner_size(size);
        }

        // Create the GL surface to draw into.
        let surface = platform::create_gl_surface(
            &gl_context,
            window.inner_size(),
            window.raw_window_handle(),
        )?;

        // Make the context current.
        let context = gl_context.make_current(&surface)?;

        // Create renderer.
        let mut renderer = Renderer::new(&context, config.debug.renderer)?;

        // Load font common glyphs to accelerate rendering.
        debug!("Filling glyph cache with common glyphs");
        renderer.with_loader(|mut api| {
            glyph_cache.reset_glyph_cache(&mut api);
        });

        let padding = config.window.padding(window.scale_factor as f32);
        let viewport_size = window.inner_size();

        // Create new size with at least one column and row.
        let size_info = SizeInfo::new(
            viewport_size.width as f32,
            viewport_size.height as f32,
            cell_width,
            cell_height,
            padding.0,
            padding.1,
            config.window.dynamic_padding && config.window.dimensions().is_none(),
        );

        info!("Cell size: {cell_width} x {cell_height}");
        info!("Padding: {} x {}", size_info.padding_x(), size_info.padding_y());
        info!("Width: {}, Height: {}", size_info.width(), size_info.height());

        // Update OpenGL projection.
        renderer.resize(&size_info);

        // Clear screen.
        let background_color = config.colors.primary.background;
        renderer.clear(background_color, config.window_opacity());

        // Disable shadows for transparent windows on macOS.
        #[cfg(target_os = "macos")]
        window.set_has_shadow(config.window_opacity() >= 1.0);

        let is_wayland = matches!(raw_window_handle, RawWindowHandle::Wayland(_));

        // On Wayland we can safely ignore this call, since the window isn't visible until you
        // actually draw something into it and commit those changes.
        if !is_wayland {
            surface.swap_buffers(&context).expect("failed to swap buffers.");
            renderer.finish();
        }

        // Set resize increments for the newly created window.
        if config.window.resize_increments {
            window.set_resize_increments(PhysicalSize::new(cell_width, cell_height));
        }

        window.set_visible(true);

        // Always focus new windows, even if no Alacritty window is currently focused.
        #[cfg(target_os = "macos")]
        window.focus_window();

        #[allow(clippy::single_match)]
        #[cfg(not(windows))]
        if !_tabbed {
            match config.window.startup_mode {
                #[cfg(target_os = "macos")]
                StartupMode::SimpleFullscreen => window.set_simple_fullscreen(true),
                StartupMode::Maximized if !is_wayland => window.set_maximized(true),
                _ => (),
            }
        }

        let hint_state = HintState::new(config.hints.alphabet());

        let mut damage_tracker = DamageTracker::new(size_info.screen_lines(), size_info.columns());
        damage_tracker.debug = config.debug.highlight_damage;

        // Load custom shader pipeline if configured.
        if !config.general.custom_shader.is_empty() {
            if let Some(pipeline) = CustomShaderPipeline::new(&config.general.custom_shader.0) {
                renderer.set_custom_shader_pipeline(Some(pipeline));
            }
        }

        // Enable vsync when custom shaders are active (continuous rendering),
        // disable when not (on-demand rendering doesn't benefit from vsync).
        let has_shaders = renderer.has_custom_shader_pipeline();
        let interval = if has_shaders {
            SwapInterval::Wait(NonZeroU32::new(1).unwrap())
        } else {
            SwapInterval::DontWait
        };
        if let Err(err) = surface.set_swap_interval(&context, interval) {
            info!("Failed to set swap interval: {err}");
        }

        Ok(Self {
            context: ManuallyDrop::new(context),
            visual_bell: VisualBell::from(&config.bell),
            renderer: ManuallyDrop::new(renderer),
            renderer_preference: config.debug.renderer,
            surface: ManuallyDrop::new(surface),
            colors: List::from(&config.colors),
            frame_timer: FrameTimer::new(),
            raw_window_handle,
            damage_tracker,
            cursor_state: CursorState::default(),
            render_timer_stats: RenderTimerStats::default(),
            glyph_cache,
            hint_state,
            size_info,
            font_size,
            window,
            pending_renderer_update: Default::default(),
            vi_highlighted_hint_age: Default::default(),
            highlighted_hint_age: Default::default(),
            vi_highlighted_hint: Default::default(),
            highlighted_hint: Default::default(),
            hint_mouse_point: Default::default(),
            pending_update: Default::default(),
            cursor_hidden: Default::default(),
            meter: Default::default(),
            ime: Default::default(),
        })
    }

    #[inline]
    pub fn gl_context(&self) -> &PossiblyCurrentContext {
        &self.context
    }

    pub fn make_not_current(&mut self) {
        if self.context.is_current() {
            self.context.make_not_current_in_place().expect("failed to disable context");
        }
    }

    pub fn make_current(&mut self) {
        let is_current = self.context.is_current();

        // Attempt to make the context current if it's not.
        let context_loss = if is_current {
            self.renderer.was_context_reset()
        } else {
            match self.context.make_current(&self.surface) {
                Err(err) if err.error_kind() == ErrorKind::ContextLost => {
                    info!("Context lost for window {:?}", self.window.id());
                    true
                },
                _ => false,
            }
        };

        if !context_loss {
            return;
        }

        let gl_display = self.context.display();
        let gl_config = self.context.config();
        let raw_window_handle = Some(self.window.raw_window_handle());
        let context = platform::create_gl_context(&gl_display, &gl_config, raw_window_handle)
            .expect("failed to recreate context.");

        // Drop the old context and renderer.
        unsafe {
            ManuallyDrop::drop(&mut self.renderer);
            ManuallyDrop::drop(&mut self.context);
        }

        // Activate new context.
        let context = context.treat_as_possibly_current();
        self.context = ManuallyDrop::new(context);
        self.context.make_current(&self.surface).expect("failed to reativate context after reset.");

        // Recreate renderer.
        let renderer = Renderer::new(&self.context, self.renderer_preference)
            .expect("failed to recreate renderer after reset");
        self.renderer = ManuallyDrop::new(renderer);

        // Resize the renderer.
        self.renderer.resize(&self.size_info);

        self.reset_glyph_cache();
        self.damage_tracker.frame().mark_fully_damaged();

        debug!("Recovered window {:?} from gpu reset", self.window.id());
    }

    fn swap_buffers(&self) {
        #[allow(clippy::single_match)]
        let res = match (self.surface.deref(), &self.context.deref()) {
            #[cfg(not(any(target_os = "macos", windows)))]
            (Surface::Egl(surface), PossiblyCurrentContext::Egl(context))
                if matches!(self.raw_window_handle, RawWindowHandle::Wayland(_))
                    && !self.damage_tracker.debug =>
            {
                let damage = self.damage_tracker.shape_frame_damage(self.size_info.into());
                surface.swap_buffers_with_damage(context, &damage)
            },
            (surface, context) => surface.swap_buffers(context),
        };
        if let Err(err) = res {
            debug!("error calling swap_buffers: {err}");
        }
    }

    /// Whether custom shaders need a redraw this frame.
    ///
    /// Returns true only when a shader animation is likely still playing
    /// (within MAX_ANIMATION_SECS of the last cursor or mode change).
    /// Combined with vsync, this keeps GPU idle when nothing is animating.
    ///
    /// Note for shader authors: ambient effects that depend on `iTime` without
    /// cursor/mode changes will freeze after this timeout. This is intentional
    /// for battery life. Trigger redraws via cursor movement or mode switches.
    pub fn needs_shader_redraw(&self) -> bool {
        if !self.renderer.has_custom_shader_pipeline() {
            return false;
        }

        const MAX_ANIMATION_SECS: f32 = 2.0;

        let elapsed = self.cursor_state.start_instant.elapsed().as_secs_f32();
        let since_cursor = elapsed - self.cursor_state.time_cursor_change;
        let since_mode = elapsed - self.cursor_state.time_mode_change;

        since_cursor < MAX_ANIMATION_SECS || since_mode < MAX_ANIMATION_SECS
    }

    /// Convert cursor shape to ghostty-compatible int for shader uniforms.
    /// Hidden is not a style -- `iCursorVisible` signals that instead.
    /// Returns `None` for Hidden so callers preserve the last-known style.
    fn cursor_style_int(shape: CursorShape) -> Option<i32> {
        match shape {
            CursorShape::Block => Some(0),
            CursorShape::HollowBlock => Some(1),
            CursorShape::Beam => Some(2),
            CursorShape::Underline => Some(3),
            CursorShape::Hidden => None,
        }
    }

    /// Compute cursor rect as (x_left, y_bottom, w, h) in top-left-origin
    /// pixel space, matching the actual rendered cursor shape and size.
    fn cursor_rect(cursor: &RenderableCursor, size_info: &SizeInfo, thickness: f32) -> [f32; 4] {
        let point = cursor.point();
        let x = point.column.0 as f32 * size_info.cell_width() + size_info.padding_x();
        let y_top = point.line as f32 * size_info.cell_height() + size_info.padding_y();
        let cell_w = size_info.cell_width() * cursor.width().get() as f32;
        let cell_h = size_info.cell_height();
        let thick = (thickness * size_info.cell_width()).round().max(1.);

        let (w, h, y_off) = match cursor.shape() {
            CursorShape::Beam => (thick, cell_h, 0.0),
            CursorShape::Underline => (cell_w, thick, cell_h - thick),
            _ => (cell_w, cell_h, 0.0), // Block, HollowBlock, Hidden.
        };

        [x, y_top + y_off + h, w, h]
    }

    /// Compute cursor color as [r, g, b, a] in 0.0-1.0 range.
    fn cursor_color(cursor: &RenderableCursor) -> [f32; 4] {
        let c = cursor.color();
        [f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0, 1.0]
    }

    /// Update cursor tracking state for the custom shader pipeline.
    ///
    /// Returns `true` if the custom shader pipeline is active and should be used
    /// for rendering this frame.
    fn update_custom_shader_state(
        &mut self,
        cursor: &RenderableCursor,
        size_info: &SizeInfo,
        config: &UiConfig,
        extra_cursors: &[MultiCursorInfo],
        app_cursor_visible: bool,
    ) -> bool {
        if !self.renderer.has_custom_shader_pipeline() {
            return false;
        }

        let new_rect = Self::cursor_rect(cursor, size_info, config.cursor.thickness());
        let new_color = Self::cursor_color(cursor);
        let cs = &mut self.cursor_state;
        let now = cs.start_instant.elapsed().as_secs_f64() as f32;

        // Always extract position, regardless of visibility.
        let cursor_visible = !matches!(cursor.shape(), CursorShape::Hidden);

        // Detect app-level mode change (DECTCEM), ignoring blink transitions.
        let mode_changed = app_cursor_visible != cs.app_visible;

        // When hidden (blink-off, DECTCEM off), cursor_rect returns a block-sized rect
        // at the cursor position (falls through to the _ arm in cursor_rect match).
        // Only update rect when visible OR when position has changed, to avoid
        // overwriting a good rect with a potentially zero-size hidden rect.
        let pos_changed = new_rect[0] != cs.current[0] || new_rect[1] != cs.current[1];
        if cursor_visible || pos_changed || mode_changed {
            let size_changed =
                cursor_visible && (new_rect[2] != cs.current[2] || new_rect[3] != cs.current[3]);
            if pos_changed || size_changed || mode_changed {
                cs.previous = cs.current;
                cs.previous_multi_cursor_count = cs.multi_cursor_count;
                // When multi-cursors are active and incoming extra_cursors is
                // non-empty, skip the timer reset for position-only moves.
                // Through a multiplexer, content rendering moves the primary
                // cursor before the multi-cursor sequence arrives in the same
                // frame, causing a spurious timer reset that replays animations.
                //
                // But when extra_cursors is empty (e.g. switching from a
                // multi-cursor app to a plain terminal tab), always reset so
                // the shader pipeline picks up the new cursor position.
                if mode_changed
                    || extra_cursors.is_empty()
                    || cs.multi_cursor_count == 0
                {
                    cs.time_cursor_change = now;
                }
            }
            // On mode change, suppress trail by syncing previous to new position.
            if mode_changed {
                cs.previous[0] = new_rect[0];
                cs.previous[1] = new_rect[1];
            }
            if cursor_visible {
                // Full update: position, size, and color.
                if new_color != cs.color {
                    cs.previous_color = cs.color;
                }
                cs.current = new_rect;
                cs.color = new_color;
            } else {
                // Hidden but moved: update position only, keep last-known size.
                cs.current[0] = new_rect[0];
                cs.current[1] = new_rect[1];
            }
        }
        cs.visible = cursor_visible;

        // Track cursor style and color changes.
        // Hidden returns None -- preserve last-known style (iCursorVisible handles visibility).
        if let Some(new_style) = Self::cursor_style_int(cursor.shape()) {
            if new_style != cs.cursor_style {
                cs.previous_cursor_style = cs.cursor_style;
                cs.cursor_style = new_style;
            }
        }

        if mode_changed && !pos_changed {
            // Only timestamp mode changes at the same position (e.g. vim
            // mode switch). Tab/pane switches change both DECTCEM and
            // position; those should not trigger mode-change effects.
            cs.time_mode_change = now;
        }
        cs.app_visible = app_cursor_visible;

        // Process extra cursors from kitty multi-cursor protocol.
        // All extra cursors are secondary; the main terminal cursor is primary.
        if extra_cursors.is_empty() {
            if cs.multi_cursor_count != 0 {
                // Cursor count transition (multi -> single): treat as mode change.
                cs.previous_multi_cursors = cs.multi_cursors;
                cs.previous_multi_cursor_count = cs.multi_cursor_count;
                cs.time_cursor_change = now;
                cs.multi_cursor_count = 0;
            }
        } else {
            let mut count = 0usize;
            let mut new_cursors = [[0.0f32; 4]; MAX_MULTI_CURSORS];

            for mc in extra_cursors.iter().take(MAX_MULTI_CURSORS) {
                let x = mc.col as f32 * size_info.cell_width() + size_info.padding_x();
                let y_top = mc.row as f32 * size_info.cell_height() + size_info.padding_y();
                let cell_w = size_info.cell_width();
                let cell_h = size_info.cell_height();

                new_cursors[count] = [x, y_top + cell_h, cell_w, cell_h];
                cs.multi_cursor_types[count] = 1; // All extra cursors are secondary.
                count += 1;
            }

            // Only save previous positions and update timestamp when cursors actually moved.
            let positions_changed = count as i32 != cs.multi_cursor_count
                || new_cursors[..count] != cs.multi_cursors[..count];
            if positions_changed {
                cs.previous_multi_cursors = cs.multi_cursors;
                cs.previous_multi_cursor_count = cs.multi_cursor_count;
                cs.time_cursor_change = now;

                // New cursors (indices beyond old count) have no previous position.
                // Initialize their previous to current to avoid trails from (0,0).
                let old_count = cs.multi_cursor_count.max(0) as usize;
                if count > old_count {
                    cs.previous_multi_cursors[old_count..count]
                        .copy_from_slice(&new_cursors[old_count..count]);
                }
            }
            cs.multi_cursors[..count].copy_from_slice(&new_cursors[..count]);

            // On mode change, suppress trails by syncing all previous to current.
            if mode_changed {
                cs.previous_multi_cursors[..count].copy_from_slice(&new_cursors[..count]);
            }

            cs.multi_cursor_count = count as i32;
        }

        true
    }

    /// Bind the scene FBO for off-screen rendering, switching to standard alpha blending.
    ///
    /// Returns `true` if the FBO was successfully bound. Returns `false` if FBO
    /// allocation failed (rendering falls back to direct-to-screen).
    fn bind_shader_fbo(&mut self, size_info: &SizeInfo) -> bool {
        if let Some(pipeline) = self.renderer.custom_shader_pipeline_mut() {
            let w = size_info.width() as i32;
            let h = size_info.height() as i32;
            if pipeline.ensure_fbo(w, h) {
                pipeline.bind_fbo();
                self.renderer.set_fbo_mode(true);
                self.damage_tracker.frame().mark_fully_damaged();
                return true;
            }
            warn!("Custom shader FBO allocation failed, rendering direct to screen");
        }
        false
    }

    /// Run the custom shader post-process chain, or clean up stale FBOs.
    fn run_custom_shader_pass(&mut self, effect_active: bool, size_info: &SizeInfo) {
        if effect_active {
            // Unbind FBO and restore dual-source blending.
            if let Some(pipeline) = self.renderer.custom_shader_pipeline_mut() {
                pipeline.unbind_fbo();
            }
            self.renderer.set_fbo_mode(false);

            let cs = &self.cursor_state;
            let time = cs.start_instant.elapsed().as_secs_f64() as f32;

            // Build consolidated cursor arrays: primary at index 0, secondaries at 1+.
            let mut current_cursors = [[0.0f32; 4]; MAX_MULTI_CURSORS];
            let mut previous_cursors = [[0.0f32; 4]; MAX_MULTI_CURSORS];
            let mut current_cursor_colors = [[0.0f32; 4]; MAX_MULTI_CURSORS];
            let mut current_cursor_styles = [0i32; MAX_MULTI_CURSORS];
            let mut previous_cursor_styles = [0i32; MAX_MULTI_CURSORS];
            let mut current_cursor_types = [0i32; MAX_MULTI_CURSORS];

            // Index 0: primary cursor (always populated for previous-position data).
            current_cursors[0] = cs.current;
            previous_cursors[0] = cs.previous;
            current_cursor_colors[0] = cs.color;
            current_cursor_styles[0] = cs.cursor_style;
            previous_cursor_styles[0] = cs.previous_cursor_style;
            // current_cursor_types[0] = 0 (primary) -- already zero-initialized.

            // Indices 1+: secondary cursors from kitty multi-cursor protocol.
            // Secondary cursors share the primary cursor's color and style.
            let sec = cs.multi_cursor_count.max(0) as usize;
            let previous_sec = cs.previous_multi_cursor_count.max(0) as usize;
            let max_sec = sec.max(previous_sec).min(MAX_MULTI_CURSORS - 1);
            for i in 0..max_sec {
                if i < sec {
                    current_cursors[1 + i] = cs.multi_cursors[i];
                    current_cursor_colors[1 + i] = cs.color;
                    current_cursor_styles[1 + i] = cs.cursor_style;
                    current_cursor_types[1 + i] = cs.multi_cursor_types[i];
                }
                if i < previous_sec {
                    previous_cursors[1 + i] = cs.previous_multi_cursors[i];
                }
                previous_cursor_styles[1 + i] = cs.previous_cursor_style;
            }

            // Per spec: extra cursors share main cursor's blink state, but
            // main cursor visibility (DECTCEM) does not affect extra cursors.
            // Blink-off = !visible && app_visible. DECTCEM-off = !app_visible.
            let (current_cursor_count, previous_cursor_count) =
                if cs.visible || !cs.app_visible {
                    (1 + cs.multi_cursor_count, 1 + cs.previous_multi_cursor_count)
                } else {
                    // Blink-off: suppress current cursors but keep previous_cursor_count
                    // so shaders can animate cursor disappearance via iPreviousCursors.
                    (0, 1 + cs.previous_multi_cursor_count)
                };

            let uniforms = ShaderUniforms {
                resolution: [size_info.width(), size_info.height()],
                time,
                time_cursor_change: cs.time_cursor_change,
                time_mode_change: cs.time_mode_change,
                current_cursor: cs.current,
                previous_cursor: cs.previous,
                current_cursor_color: cs.color,
                previous_cursor_color: cs.previous_color,
                cursor_visible: if cs.visible { 1.0 } else { 0.0 },
                current_cursor_style: cs.cursor_style,
                previous_cursor_style: cs.previous_cursor_style,
                current_cursors,
                previous_cursors,
                current_cursor_colors,
                current_cursor_styles,
                previous_cursor_styles,
                current_cursor_types,
                current_cursor_count,
                previous_cursor_count,
            };

            if let Some(pipeline) = self.renderer.custom_shader_pipeline_mut() {
                pipeline.draw(size_info, &uniforms);
            }
        } else {
            // Free FBO when no shader is active (e.g., removed via config reload).
            if let Some(pipeline) = self.renderer.custom_shader_pipeline_mut() {
                if pipeline.has_fbo() {
                    pipeline.delete_fbo();
                }
            }
        }
    }

    /// Update font size and cell dimensions.
    ///
    /// This will return a tuple of the cell width and height.
    fn update_font_size(
        glyph_cache: &mut GlyphCache,
        config: &UiConfig,
        font: &Font,
    ) -> (f32, f32) {
        let _ = glyph_cache.update_font_size(font);

        // Compute new cell sizes.
        compute_cell_size(config, &glyph_cache.font_metrics())
    }

    /// Reset glyph cache.
    fn reset_glyph_cache(&mut self) {
        let cache = &mut self.glyph_cache;
        self.renderer.with_loader(|mut api| {
            cache.reset_glyph_cache(&mut api);
        });
    }

    // XXX: this function must not call to any `OpenGL` related tasks. Renderer updates are
    // performed in [`Self::process_renderer_update`] right before drawing.
    //
    /// Process update events.
    pub fn handle_update<T>(
        &mut self,
        terminal: &mut Term<T>,
        pty_resize_handle: &mut dyn OnResize,
        message_buffer: &MessageBuffer,
        search_state: &mut SearchState,
        config: &UiConfig,
    ) where
        T: EventListener,
    {
        let pending_update = mem::take(&mut self.pending_update);

        let (mut cell_width, mut cell_height) =
            (self.size_info.cell_width(), self.size_info.cell_height());

        if pending_update.font().is_some() || pending_update.cursor_dirty() {
            let renderer_update = self.pending_renderer_update.get_or_insert(Default::default());
            renderer_update.clear_font_cache = true
        }

        // Update font size and cell dimensions.
        if let Some(font) = pending_update.font() {
            let cell_dimensions = Self::update_font_size(&mut self.glyph_cache, config, font);
            cell_width = cell_dimensions.0;
            cell_height = cell_dimensions.1;

            info!("Cell size: {cell_width} x {cell_height}");

            // Mark entire terminal as damaged since glyph size could change without cell size
            // changes.
            self.damage_tracker.frame().mark_fully_damaged();
        }

        let (mut width, mut height) = (self.size_info.width(), self.size_info.height());
        if let Some(dimensions) = pending_update.dimensions() {
            width = dimensions.width as f32;
            height = dimensions.height as f32;
        }

        let padding = config.window.padding(self.window.scale_factor as f32);

        let mut new_size = SizeInfo::new(
            width,
            height,
            cell_width,
            cell_height,
            padding.0,
            padding.1,
            config.window.dynamic_padding,
        );

        // Update number of column/lines in the viewport.
        let search_active = search_state.history_index.is_some();
        let message_bar_lines = message_buffer.message().map_or(0, |m| m.text(&new_size).len());
        let search_lines = usize::from(search_active);
        new_size.reserve_lines(message_bar_lines + search_lines);

        // Update resize increments.
        if config.window.resize_increments {
            self.window.set_resize_increments(PhysicalSize::new(cell_width, cell_height));
        }

        // Resize when terminal when its dimensions have changed.
        if self.size_info.screen_lines() != new_size.screen_lines
            || self.size_info.columns() != new_size.columns()
        {
            // Resize PTY.
            pty_resize_handle.on_resize(new_size.into());

            // Resize terminal.
            terminal.resize(new_size);

            // Resize damage tracking.
            self.damage_tracker.resize(new_size.screen_lines(), new_size.columns());
        }

        // Check if dimensions have changed.
        if new_size != self.size_info {
            // Queue renderer update.
            let renderer_update = self.pending_renderer_update.get_or_insert(Default::default());
            renderer_update.resize = true;

            // Clear focused search match.
            search_state.clear_focused_match();
        }
        self.size_info = new_size;
    }

    // NOTE: Renderer updates are split off, since platforms like Wayland require resize and other
    // OpenGL operations to be performed right before rendering. Otherwise they could lock the
    // back buffer and render with the previous state. This also solves flickering during resizes.
    //
    /// Update the state of the renderer.
    pub fn process_renderer_update(&mut self) {
        let renderer_update = match self.pending_renderer_update.take() {
            Some(renderer_update) => renderer_update,
            _ => return,
        };

        // Resize renderer.
        if renderer_update.resize {
            let width = NonZeroU32::new(self.size_info.width() as u32).unwrap();
            let height = NonZeroU32::new(self.size_info.height() as u32).unwrap();
            self.surface.resize(&self.context, width, height);
        }

        // Ensure we're modifying the correct OpenGL context.
        self.make_current();

        if renderer_update.clear_font_cache {
            self.reset_glyph_cache();
        }

        self.renderer.resize(&self.size_info);

        info!("Padding: {} x {}", self.size_info.padding_x(), self.size_info.padding_y());
        info!("Width: {}, Height: {}", self.size_info.width(), self.size_info.height());
    }

    /// Draw the screen.
    ///
    /// A reference to Term whose state is being drawn must be provided.
    ///
    /// This call may block if vsync is enabled.
    pub fn draw<T: EventListener>(
        &mut self,
        mut terminal: MutexGuard<'_, Term<T>>,
        scheduler: &mut Scheduler,
        message_buffer: &MessageBuffer,
        config: &UiConfig,
        search_state: &mut SearchState,
    ) {
        // Collect renderable content before the terminal is dropped.
        let mut content = RenderableContent::new(config, self, &terminal, search_state);
        let mut grid_cells = Vec::new();
        for cell in &mut content {
            grid_cells.push(cell);
        }
        let selection_range = content.selection_range();
        let foreground_color = content.color(NamedColor::Foreground as usize);
        let background_color = content.color(NamedColor::Background as usize);
        let display_offset = content.display_offset();
        let cursor = content.cursor();

        let cursor_point = terminal.grid().cursor.point;
        let total_lines = terminal.grid().total_lines();
        let metrics = self.glyph_cache.font_metrics();
        let size_info = self.size_info;

        // Update custom shader state and check if post-process shaders are active.
        let extra_cursors = terminal.multi_cursors();
        let has_extra_cursors = !extra_cursors.is_empty();
        let app_cursor_visible = terminal.mode().contains(TermMode::SHOW_CURSOR);
        let effect_active = self.update_custom_shader_state(
            &cursor,
            &size_info,
            config,
            extra_cursors,
            app_cursor_visible,
        );

        let vi_mode = terminal.mode().contains(TermMode::VI);
        let vi_cursor_point = if vi_mode { Some(terminal.vi_mode_cursor.point) } else { None };

        // Add damage from the terminal.
        match terminal.damage() {
            TermDamage::Full => self.damage_tracker.frame().mark_fully_damaged(),
            TermDamage::Partial(damaged_lines) => {
                for damage in damaged_lines {
                    self.damage_tracker.frame().damage_line(damage);
                }
            },
        }
        terminal.reset_damage();

        // Drop terminal as early as possible to free lock.
        drop(terminal);

        // Invalidate highlighted hints if grid has changed.
        self.validate_hint_highlights(display_offset);

        // Add damage from alacritty's UI elements overlapping terminal.

        let requires_full_damage = self.visual_bell.intensity() != 0.
            || self.hint_state.active()
            || search_state.regex().is_some();
        if requires_full_damage {
            self.damage_tracker.frame().mark_fully_damaged();
            self.damage_tracker.next_frame().mark_fully_damaged();
        }

        let vi_cursor_viewport_point =
            vi_cursor_point.and_then(|cursor| term::point_to_viewport(display_offset, cursor));
        self.damage_tracker.damage_vi_cursor(vi_cursor_viewport_point);
        self.damage_tracker.damage_selection(selection_range, display_offset);

        // Make sure this window's OpenGL context is active.
        self.make_current();

        // Bind FBO and switch to standard alpha blending for custom shaders.
        let effect_active = if effect_active { self.bind_shader_fbo(&size_info) } else { false };

        self.renderer.clear(background_color, config.window_opacity());
        let mut lines = RenderLines::new();

        // Optimize loop hint comparator.
        let has_highlighted_hint =
            self.highlighted_hint.is_some() || self.vi_highlighted_hint.is_some();

        // Draw grid.
        {
            let _sampler = self.meter.sampler();

            // Ensure macOS hasn't reset our viewport.
            #[cfg(target_os = "macos")]
            self.renderer.set_viewport(&size_info);

            let glyph_cache = &mut self.glyph_cache;
            let highlighted_hint = &self.highlighted_hint;
            let vi_highlighted_hint = &self.vi_highlighted_hint;
            let damage_tracker = &mut self.damage_tracker;

            let cells = grid_cells.into_iter().map(|mut cell| {
                // Underline hints hovered by mouse or vi mode cursor.
                if has_highlighted_hint {
                    let point = term::viewport_to_point(display_offset, cell.point);
                    let hyperlink = cell.extra.as_ref().and_then(|extra| extra.hyperlink.as_ref());

                    let should_highlight = |hint: &Option<HintMatch>| {
                        hint.as_ref().is_some_and(|hint| hint.should_highlight(point, hyperlink))
                    };
                    if should_highlight(highlighted_hint) || should_highlight(vi_highlighted_hint) {
                        damage_tracker.frame().damage_point(cell.point);
                        cell.flags.insert(Flags::UNDERLINE);
                    }
                }

                // Update underline/strikeout.
                lines.update(&cell);

                cell
            });
            self.renderer.draw_cells(&size_info, glyph_cache, cells);
        }

        let mut rects = lines.rects(&metrics, &size_info);

        if let Some(vi_cursor_point) = vi_cursor_point {
            // Indicate vi mode by showing the cursor's position in the top right corner.
            let line = (-vi_cursor_point.line.0 + size_info.bottommost_line().0) as usize;
            let obstructed_column = Some(vi_cursor_point)
                .filter(|point| point.line == -(display_offset as i32))
                .map(|point| point.column);
            self.draw_line_indicator(config, total_lines, obstructed_column, line);
        } else if search_state.regex().is_some() {
            // Show current display offset in vi-less search to indicate match position.
            self.draw_line_indicator(config, total_lines, None, display_offset);
        };

        // Draw cursor. When multi-cursor protocol is active, the shader handles
        // all cursor rendering via iCurrentCursors[] -- suppress the native cursor rect
        // to avoid visual overlap.
        if !has_extra_cursors {
            rects.extend(cursor.rects(&size_info, config.cursor.thickness()));
        }

        // Push visual bell after url/underline/strikeout rects.
        let visual_bell_intensity = self.visual_bell.intensity();
        if visual_bell_intensity != 0. {
            let visual_bell_rect = RenderRect::new(
                0.,
                0.,
                size_info.width(),
                size_info.height(),
                config.bell.color,
                visual_bell_intensity as f32,
            );
            rects.push(visual_bell_rect);
        }

        // Handle IME positioning and search bar rendering.
        let ime_position = match search_state.regex() {
            Some(regex) => {
                let search_label = match search_state.direction() {
                    Direction::Right => FORWARD_SEARCH_LABEL,
                    Direction::Left => BACKWARD_SEARCH_LABEL,
                };

                let search_text = Self::format_search(regex, search_label, size_info.columns());

                // Render the search bar.
                self.draw_search(config, &search_text);

                // Draw search bar cursor.
                let line = size_info.screen_lines();
                let column = Column(search_text.chars().count() - 1);

                // Add cursor to search bar if IME is not active.
                if self.ime.preedit().is_none() {
                    let fg = config.colors.footer_bar_foreground();
                    let shape = CursorShape::Underline;
                    let cursor_width = NonZeroU32::new(1).unwrap();
                    let cursor =
                        RenderableCursor::new(Point::new(line, column), shape, fg, cursor_width);
                    rects.extend(cursor.rects(&size_info, config.cursor.thickness()));
                }

                Some(Point::new(line, column))
            },
            None => {
                let num_lines = self.size_info.screen_lines();
                match vi_cursor_viewport_point {
                    None => term::point_to_viewport(display_offset, cursor_point)
                        .filter(|point| point.line < num_lines),
                    point => point,
                }
            },
        };

        // Handle IME.
        if self.ime.is_enabled() {
            if let Some(point) = ime_position {
                let (fg, bg) = if search_state.regex().is_some() {
                    (config.colors.footer_bar_foreground(), config.colors.footer_bar_background())
                } else {
                    (foreground_color, background_color)
                };

                self.draw_ime_preview(point, fg, bg, &mut rects, config);
            }
        }

        if let Some(message) = message_buffer.message() {
            let search_offset = usize::from(search_state.regex().is_some());
            let text = message.text(&size_info);

            // Create a new rectangle for the background.
            let start_line = size_info.screen_lines() + search_offset;
            let y = size_info.cell_height().mul_add(start_line as f32, size_info.padding_y());

            let bg = match message.ty() {
                MessageType::Error => config.colors.normal.red,
                MessageType::Warning => config.colors.normal.yellow,
            };

            let x = 0;
            let width = size_info.width() as i32;
            let height = (size_info.height() - y) as i32;
            let message_bar_rect =
                RenderRect::new(x as f32, y, width as f32, height as f32, bg, 1.);

            // Push message_bar in the end, so it'll be above all other content.
            rects.push(message_bar_rect);

            // Always damage message bar, since it could have messages of the same size in it.
            self.damage_tracker.frame().add_viewport_rect(&size_info, x, y as i32, width, height);

            // Draw rectangles.
            self.renderer.draw_rects(&size_info, &metrics, rects);

            // Relay messages to the user.
            let glyph_cache = &mut self.glyph_cache;
            let fg = config.colors.primary.background;
            for (i, message_text) in text.iter().enumerate() {
                let point = Point::new(start_line + i, Column(0));
                self.renderer.draw_string(
                    point,
                    fg,
                    bg,
                    message_text.chars(),
                    &size_info,
                    glyph_cache,
                );
            }
        } else {
            // Draw rectangles.
            self.renderer.draw_rects(&size_info, &metrics, rects);
        }

        // Run custom shader post-process chain (or clean up stale FBOs).
        self.run_custom_shader_pass(effect_active, &size_info);

        // Restore padded viewport after shader pass (which uses full-window viewport).
        // Required for render timer, hyperlink preview, and damage highlight positioning.
        if effect_active {
            self.renderer.set_viewport(&size_info);
        }

        // Render timer draws after the shader pass, so it appears on top of
        // post-processed output and is not affected by custom shader effects.
        self.draw_render_timer(config);

        // Draw hyperlink uri preview.
        if has_highlighted_hint {
            let cursor_point = vi_cursor_point.or(Some(cursor_point));
            self.draw_hyperlink_preview(config, cursor_point, display_offset);
        }

        // Notify winit that we're about to present.
        self.window.pre_present_notify();

        // Highlight damage for debugging.
        if self.damage_tracker.debug {
            let damage = self.damage_tracker.shape_frame_damage(self.size_info.into());
            let mut rects = Vec::with_capacity(damage.len());
            self.highlight_damage(&mut rects);
            self.renderer.draw_rects(&self.size_info, &metrics, rects);
        }

        // Clearing debug highlights from the previous frame requires full redraw.
        self.swap_buffers();

        if matches!(self.raw_window_handle, RawWindowHandle::Xcb(_) | RawWindowHandle::Xlib(_)) {
            // On X11 `swap_buffers` does not block for vsync. However the next OpenGl command
            // will block to synchronize (this is `glClear` in Alacritty), which causes a
            // permanent one frame delay.
            self.renderer.finish();
        }

        // XXX: Request the new frame after swapping buffers, so the
        // time to finish OpenGL operations is accounted for in the timeout.
        // On Wayland, only request frames when shaders are active (compositor controls
        // frame pacing otherwise). On X11/macOS, frames are always requested.
        if effect_active || !matches!(self.raw_window_handle, RawWindowHandle::Wayland(_)) {
            self.request_frame(scheduler);
        }

        self.damage_tracker.swap_damage();
    }

    /// Update to a new configuration.
    pub fn update_config(&mut self, config: &UiConfig) {
        self.damage_tracker.debug = config.debug.highlight_damage;
        self.visual_bell.update_config(&config.bell);
        self.colors = List::from(&config.colors);

        // Reload custom shader pipeline if config changed.
        let has_new_shaders = !config.general.custom_shader.is_empty();
        let has_current_pipeline = self.renderer.has_custom_shader_pipeline();

        match (has_new_shaders, has_current_pipeline) {
            (true, _) => {
                // Shaders configured -- reload entire pipeline.
                let pipeline = CustomShaderPipeline::new(&config.general.custom_shader.0);
                self.renderer.set_custom_shader_pipeline(pipeline);
                if self.renderer.custom_shader_pipeline_mut().is_none() {
                    warn!("Custom shader pipeline reload failed, effects disabled");
                }
            },
            (false, true) => {
                // Shaders removed from config -- clean up.
                self.renderer.set_custom_shader_pipeline(None);
                self.cursor_state = CursorState::default();
                info!("Custom shader pipeline disabled");
            },
            (false, false) => {},
        }

        // Toggle vsync based on shader state.
        let interval = if self.renderer.has_custom_shader_pipeline() {
            SwapInterval::Wait(NonZeroU32::new(1).unwrap())
        } else {
            SwapInterval::DontWait
        };
        if let Err(err) = self.surface.set_swap_interval(&self.context, interval) {
            info!("Failed to set swap interval: {err}");
        }
    }

    /// Update the mouse/vi mode cursor hint highlighting.
    ///
    /// This will return whether the highlighted hints changed.
    pub fn update_highlighted_hints<T>(
        &mut self,
        term: &Term<T>,
        config: &UiConfig,
        mouse: &Mouse,
        modifiers: ModifiersState,
    ) -> bool {
        // Update vi mode cursor hint.
        let vi_highlighted_hint = if term.mode().contains(TermMode::VI) {
            let mods = ModifiersState::all();
            let point = term.vi_mode_cursor.point;
            hint::highlighted_at(term, config, point, mods)
        } else {
            None
        };
        let mut dirty = vi_highlighted_hint != self.vi_highlighted_hint;
        self.vi_highlighted_hint = vi_highlighted_hint;
        self.vi_highlighted_hint_age = 0;

        // Force full redraw if the vi mode highlight was cleared.
        if dirty {
            self.damage_tracker.frame().mark_fully_damaged();
        }

        // Abort if mouse highlighting conditions are not met.
        if !self.window.mouse_visible()
            || !mouse.inside_text_area
            || !term.selection.as_ref().is_none_or(Selection::is_empty)
        {
            if self.highlighted_hint.take().is_some() {
                self.damage_tracker.frame().mark_fully_damaged();
                dirty = true;
            }
            return dirty;
        }

        // Find highlighted hint at mouse position.
        let point = mouse.point(&self.size_info, term.grid().display_offset());
        let highlighted_hint = hint::highlighted_at(term, config, point, modifiers);

        // Update cursor shape.
        if highlighted_hint.is_some() {
            // If mouse changed the line, we should update the hyperlink preview, since the
            // highlighted hint could be disrupted by the old preview.
            dirty = self.hint_mouse_point.is_some_and(|p| p.line != point.line);
            self.hint_mouse_point = Some(point);
            self.window.set_mouse_cursor(CursorIcon::Pointer);
        } else if self.highlighted_hint.is_some() {
            self.hint_mouse_point = None;
            if term.mode().intersects(TermMode::MOUSE_MODE) && !term.mode().contains(TermMode::VI) {
                self.window.set_mouse_cursor(CursorIcon::Default);
            } else {
                self.window.set_mouse_cursor(CursorIcon::Text);
            }
        }

        let mouse_highlight_dirty = self.highlighted_hint != highlighted_hint;
        dirty |= mouse_highlight_dirty;
        self.highlighted_hint = highlighted_hint;
        self.highlighted_hint_age = 0;

        // Force full redraw if the mouse cursor highlight was changed.
        if mouse_highlight_dirty {
            self.damage_tracker.frame().mark_fully_damaged();
        }

        dirty
    }

    #[inline(never)]
    fn draw_ime_preview(
        &mut self,
        point: Point<usize>,
        fg: Rgb,
        bg: Rgb,
        rects: &mut Vec<RenderRect>,
        config: &UiConfig,
    ) {
        let preedit = match self.ime.preedit() {
            Some(preedit) => preedit,
            None => {
                // In case we don't have preedit, just set the popup point.
                self.window.update_ime_position(point, &self.size_info);
                return;
            },
        };

        let num_cols = self.size_info.columns();

        // Get the visible preedit.
        let visible_text: String = match (preedit.cursor_byte_offset, preedit.cursor_end_offset) {
            (Some(byte_offset), Some(end_offset)) if end_offset.0 > num_cols => StrShortener::new(
                &preedit.text[byte_offset.0..],
                num_cols,
                ShortenDirection::Right,
                Some(SHORTENER),
            ),
            _ => {
                StrShortener::new(&preedit.text, num_cols, ShortenDirection::Left, Some(SHORTENER))
            },
        }
        .collect();

        let visible_len = visible_text.chars().count();

        let end = cmp::min(point.column.0 + visible_len, num_cols);
        let start = end.saturating_sub(visible_len);

        let start = Point::new(point.line, Column(start));
        let end = Point::new(point.line, Column(end - 1));

        let glyph_cache = &mut self.glyph_cache;
        let metrics = glyph_cache.font_metrics();

        self.renderer.draw_string(
            start,
            fg,
            bg,
            visible_text.chars(),
            &self.size_info,
            glyph_cache,
        );

        // Damage preedit inside the terminal viewport.
        if point.line < self.size_info.screen_lines() {
            let damage = LineDamageBounds::new(start.line, 0, num_cols);
            self.damage_tracker.frame().damage_line(damage);
            self.damage_tracker.next_frame().damage_line(damage);
        }

        // Add underline for preedit text.
        let underline = RenderLine { start, end, color: fg };
        rects.extend(underline.rects(Flags::UNDERLINE, &metrics, &self.size_info));

        let ime_popup_point = match preedit.cursor_end_offset {
            Some(cursor_end_offset) => {
                // Use hollow block when multiple characters are changed at once.
                let (shape, width) = if let Some(width) =
                    NonZeroU32::new((cursor_end_offset.0 - cursor_end_offset.1) as u32)
                {
                    (CursorShape::HollowBlock, width)
                } else {
                    (CursorShape::Beam, NonZeroU32::new(1).unwrap())
                };

                let cursor_column = Column(
                    (end.column.0 as isize - cursor_end_offset.0 as isize + 1).max(0) as usize,
                );
                let cursor_point = Point::new(point.line, cursor_column);
                let cursor = RenderableCursor::new(cursor_point, shape, fg, width);
                rects.extend(cursor.rects(&self.size_info, config.cursor.thickness()));
                cursor_point
            },
            _ => end,
        };

        self.window.update_ime_position(ime_popup_point, &self.size_info);
    }

    /// Format search regex to account for the cursor and fullwidth characters.
    fn format_search(search_regex: &str, search_label: &str, max_width: usize) -> String {
        let label_len = search_label.len();

        // Skip `search_regex` formatting if only label is visible.
        if label_len > max_width {
            return search_label[..max_width].to_owned();
        }

        // The search string consists of `search_label` + `search_regex` + `cursor`.
        let mut bar_text = String::from(search_label);
        bar_text.extend(StrShortener::new(
            search_regex,
            max_width.wrapping_sub(label_len + 1),
            ShortenDirection::Left,
            Some(SHORTENER),
        ));

        // Add place for cursor.
        bar_text.push(' ');

        bar_text
    }

    /// Draw preview for the currently highlighted `Hyperlink`.
    #[inline(never)]
    fn draw_hyperlink_preview(
        &mut self,
        config: &UiConfig,
        cursor_point: Option<Point>,
        display_offset: usize,
    ) {
        let num_cols = self.size_info.columns();
        let uris: Vec<_> = self
            .highlighted_hint
            .iter()
            .chain(&self.vi_highlighted_hint)
            .filter_map(|hint| hint.hyperlink().map(|hyperlink| hyperlink.uri()))
            .map(|uri| StrShortener::new(uri, num_cols, ShortenDirection::Right, Some(SHORTENER)))
            .collect();

        if uris.is_empty() {
            return;
        }

        // The maximum amount of protected lines including the ones we'll show preview on.
        let max_protected_lines = uris.len() * 2;

        // Lines we shouldn't show preview on, because it'll obscure the highlighted hint.
        let mut protected_lines = Vec::with_capacity(max_protected_lines);
        if self.size_info.screen_lines() > max_protected_lines {
            // Prefer to show preview even when it'll likely obscure the highlighted hint, when
            // there's no place left for it.
            protected_lines.push(self.hint_mouse_point.map(|point| point.line));
            protected_lines.push(cursor_point.map(|point| point.line));
        }

        // Find the line in viewport we can draw preview on without obscuring protected lines.
        let viewport_bottom = self.size_info.bottommost_line() - Line(display_offset as i32);
        let viewport_top = viewport_bottom - (self.size_info.screen_lines() - 1);
        let uri_lines = (viewport_top.0..=viewport_bottom.0)
            .rev()
            .map(|line| Some(Line(line)))
            .filter_map(|line| {
                if protected_lines.contains(&line) {
                    None
                } else {
                    protected_lines.push(line);
                    line
                }
            })
            .take(uris.len())
            .flat_map(|line| term::point_to_viewport(display_offset, Point::new(line, Column(0))));

        let fg = config.colors.footer_bar_foreground();
        let bg = config.colors.footer_bar_background();
        for (uri, point) in uris.into_iter().zip(uri_lines) {
            // Damage the uri preview.
            let damage = LineDamageBounds::new(point.line, point.column.0, num_cols);
            self.damage_tracker.frame().damage_line(damage);

            // Damage the uri preview for the next frame as well.
            self.damage_tracker.next_frame().damage_line(damage);

            self.renderer.draw_string(point, fg, bg, uri, &self.size_info, &mut self.glyph_cache);
        }
    }

    /// Draw current search regex.
    #[inline(never)]
    fn draw_search(&mut self, config: &UiConfig, text: &str) {
        // Assure text length is at least num_cols.
        let num_cols = self.size_info.columns();
        let text = format!("{text:<num_cols$}");

        let point = Point::new(self.size_info.screen_lines(), Column(0));

        let fg = config.colors.footer_bar_foreground();
        let bg = config.colors.footer_bar_background();

        self.renderer.draw_string(
            point,
            fg,
            bg,
            text.chars(),
            &self.size_info,
            &mut self.glyph_cache,
        );
    }

    /// Draw render timer.
    #[inline(never)]
    fn draw_render_timer(&mut self, config: &UiConfig) {
        if !config.debug.render_timer {
            return;
        }

        // Accumulate samples, publish stats every 1 second.
        let elapsed = self.render_timer_stats.start_instant.elapsed().as_secs_f64() as f32;
        let stats = &mut self.render_timer_stats;

        // Skip first frame to avoid a huge initial delta.
        if stats.last_frame_time == 0.0 {
            stats.last_frame_time = elapsed;
            return;
        }

        let dt = (elapsed - stats.last_frame_time).max(0.0001);
        stats.last_frame_time = elapsed;
        let render_usec = self.meter.average() as f32;
        stats.accum_time += dt;
        stats.samples.push(render_usec);

        if stats.accum_time >= 1.0 {
            let n = stats.samples.len();
            if n > 0 {
                stats.samples.sort_unstable_by(|a, b| a.total_cmp(b));
                let sum: f32 = stats.samples.iter().sum();
                stats.display_avg = sum / n as f32;
                // 1% percentiles.
                let p1 = (n / 100).max(1) - 1;
                let p99 = n.saturating_sub((n / 100).max(1));
                stats.display_p1 = stats.samples[p1];
                stats.display_p99 = stats.samples[p99];
            }
            // Sample memory only once per second.
            stats.display_mem = process_memory_mb();
            stats.accum_time = 0.0;
            stats.samples.clear();
            stats.samples.shrink_to(256);
        }

        let timing = format!(
            "{:.0} usec (p1: {:.0} / p99: {:.0}) | {:.0} MB",
            stats.display_avg, stats.display_p1, stats.display_p99, stats.display_mem,
        );
        let point = Point::new(self.size_info.screen_lines().saturating_sub(2), Column(0));
        let fg = config.colors.primary.background;
        let bg = config.colors.normal.red;

        // Damage render timer for current and next frame.
        let damage = LineDamageBounds::new(point.line, point.column.0, timing.len());
        self.damage_tracker.frame().damage_line(damage);
        self.damage_tracker.next_frame().damage_line(damage);

        let glyph_cache = &mut self.glyph_cache;
        self.renderer.draw_string(point, fg, bg, timing.chars(), &self.size_info, glyph_cache);
    }

    /// Draw an indicator for the position of a line in history.
    #[inline(never)]
    fn draw_line_indicator(
        &mut self,
        config: &UiConfig,
        total_lines: usize,
        obstructed_column: Option<Column>,
        line: usize,
    ) {
        let columns = self.size_info.columns();
        let text = format!("[{}/{}]", line, total_lines - 1);
        let column = Column(self.size_info.columns().saturating_sub(text.len()));
        let point = Point::new(0, column);

        // Damage the line indicator for current and next frame.
        let damage = LineDamageBounds::new(point.line, point.column.0, columns - 1);
        self.damage_tracker.frame().damage_line(damage);
        self.damage_tracker.next_frame().damage_line(damage);

        let colors = &config.colors;
        let fg = colors.line_indicator.foreground.unwrap_or(colors.primary.background);
        let bg = colors.line_indicator.background.unwrap_or(colors.primary.foreground);

        // Do not render anything if it would obscure the vi mode cursor.
        if obstructed_column.is_none_or(|obstructed_column| obstructed_column < column) {
            let glyph_cache = &mut self.glyph_cache;
            self.renderer.draw_string(point, fg, bg, text.chars(), &self.size_info, glyph_cache);
        }
    }

    /// Highlight damaged rects.
    ///
    /// This function is for debug purposes only.
    fn highlight_damage(&self, render_rects: &mut Vec<RenderRect>) {
        for damage_rect in &self.damage_tracker.shape_frame_damage(self.size_info.into()) {
            let x = damage_rect.x as f32;
            let height = damage_rect.height as f32;
            let width = damage_rect.width as f32;
            let y = damage_y_to_viewport_y(&self.size_info, damage_rect) as f32;
            let render_rect = RenderRect::new(x, y, width, height, DAMAGE_RECT_COLOR, 0.5);

            render_rects.push(render_rect);
        }
    }

    /// Check whether a hint highlight needs to be cleared.
    fn validate_hint_highlights(&mut self, display_offset: usize) {
        let frame = self.damage_tracker.frame();
        let hints = [
            (&mut self.highlighted_hint, &mut self.highlighted_hint_age, true),
            (&mut self.vi_highlighted_hint, &mut self.vi_highlighted_hint_age, false),
        ];

        let num_lines = self.size_info.screen_lines();
        for (hint, hint_age, reset_mouse) in hints {
            let (start, end) = match hint {
                Some(hint) => (*hint.bounds().start(), *hint.bounds().end()),
                None => continue,
            };

            // Ignore hints that were created this frame.
            *hint_age += 1;
            if *hint_age == 1 {
                continue;
            }

            // Convert hint bounds to viewport coordinates.
            let start = term::point_to_viewport(display_offset, start)
                .filter(|point| point.line < num_lines)
                .unwrap_or_default();
            let end = term::point_to_viewport(display_offset, end)
                .filter(|point| point.line < num_lines)
                .unwrap_or_else(|| Point::new(num_lines - 1, self.size_info.last_column()));

            // Clear invalidated hints.
            if frame.intersects(start, end) {
                if reset_mouse {
                    self.window.set_mouse_cursor(CursorIcon::Default);
                }
                frame.mark_fully_damaged();
                *hint = None;
            }
        }
    }

    /// Request a new frame for a window on Wayland.
    fn request_frame(&mut self, scheduler: &mut Scheduler) {
        // Mark that we've used a frame.
        self.window.has_frame = false;

        // Get the display vblank interval.
        let monitor_vblank_interval = 1_000_000.
            / self
                .window
                .current_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz())
                .unwrap_or(60_000) as f64;

        // Now convert it to micro seconds.
        let monitor_vblank_interval =
            Duration::from_micros((1000. * monitor_vblank_interval) as u64);

        let swap_timeout = self.frame_timer.compute_timeout(monitor_vblank_interval);

        let window_id = self.window.id();
        let timer_id = TimerId::new(Topic::Frame, window_id);
        let event = Event::new(EventType::Frame, window_id);

        scheduler.schedule(event, swap_timeout, false, timer_id);
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // Switch OpenGL context before dropping, otherwise objects (like programs) from other
        // contexts might be deleted when dropping renderer.
        self.make_current();
        unsafe {
            ManuallyDrop::drop(&mut self.renderer);
            ManuallyDrop::drop(&mut self.context);
            ManuallyDrop::drop(&mut self.surface);
        }
    }
}

/// Input method state.
#[derive(Debug, Default)]
pub struct Ime {
    /// Whether the IME is enabled.
    enabled: bool,

    /// Current IME preedit.
    preedit: Option<Preedit>,
}

impl Ime {
    #[inline]
    pub fn set_enabled(&mut self, is_enabled: bool) {
        if is_enabled {
            self.enabled = is_enabled
        } else {
            // Clear state when disabling IME.
            *self = Default::default();
        }
    }

    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[inline]
    pub fn set_preedit(&mut self, preedit: Option<Preedit>) {
        self.preedit = preedit;
    }

    #[inline]
    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Preedit {
    /// The preedit text.
    text: String,

    /// Byte offset for cursor start into the preedit text.
    ///
    /// `None` means that the cursor is invisible.
    cursor_byte_offset: Option<(usize, usize)>,

    /// The cursor offset from the end of the start of the preedit in char width.
    cursor_end_offset: Option<(usize, usize)>,
}

impl Preedit {
    pub fn new(text: String, cursor_byte_offset: Option<(usize, usize)>) -> Self {
        let cursor_end_offset = if let Some(byte_offset) = cursor_byte_offset {
            // Convert byte offset into char offset.
            let start_to_end_offset =
                text[byte_offset.0..].chars().fold(0, |acc, ch| acc + ch.width().unwrap_or(1));
            let end_to_end_offset =
                text[byte_offset.1..].chars().fold(0, |acc, ch| acc + ch.width().unwrap_or(1));

            Some((start_to_end_offset, end_to_end_offset))
        } else {
            None
        };

        Self { text, cursor_byte_offset, cursor_end_offset }
    }
}

/// Pending renderer updates.
///
/// All renderer updates are cached to be applied just before rendering, to avoid platform-specific
/// rendering issues.
#[derive(Debug, Default, Copy, Clone)]
pub struct RendererUpdate {
    /// Should resize the window.
    resize: bool,

    /// Clear font caches.
    clear_font_cache: bool,
}

/// The frame timer state.
pub struct FrameTimer {
    /// Base timestamp used to compute sync points.
    base: Instant,

    /// The last timestamp we synced to.
    last_synced_timestamp: Instant,

    /// The refresh rate we've used to compute sync timestamps.
    refresh_interval: Duration,
}

impl FrameTimer {
    pub fn new() -> Self {
        let now = Instant::now();
        Self { base: now, last_synced_timestamp: now, refresh_interval: Duration::ZERO }
    }

    /// Compute the delay that we should use to achieve the target frame
    /// rate.
    pub fn compute_timeout(&mut self, refresh_interval: Duration) -> Duration {
        let now = Instant::now();

        // Handle refresh rate change.
        if self.refresh_interval != refresh_interval {
            self.base = now;
            self.last_synced_timestamp = now;
            self.refresh_interval = refresh_interval;
            return refresh_interval;
        }

        let next_frame = self.last_synced_timestamp + self.refresh_interval;

        if next_frame < now {
            // Redraw immediately if we haven't drawn in over `refresh_interval` microseconds.
            let elapsed_micros = (now - self.base).as_micros() as u64;
            let refresh_micros = self.refresh_interval.as_micros() as u64;
            self.last_synced_timestamp =
                now - Duration::from_micros(elapsed_micros % refresh_micros);
            Duration::ZERO
        } else {
            // Redraw on the next `refresh_interval` clock tick.
            self.last_synced_timestamp = next_frame;
            next_frame - now
        }
    }
}

/// Calculate the cell dimensions based on font metrics.
///
/// This will return a tuple of the cell width and height.
#[inline]
fn compute_cell_size(config: &UiConfig, metrics: &crossfont::Metrics) -> (f32, f32) {
    let offset_x = f64::from(config.font.offset.x);
    let offset_y = f64::from(config.font.offset.y);
    (
        (metrics.average_advance + offset_x).floor().max(1.) as f32,
        (metrics.line_height + offset_y).floor().max(1.) as f32,
    )
}

/// Calculate the size of the window given padding, terminal dimensions and cell size.
fn window_size(
    config: &UiConfig,
    dimensions: Dimensions,
    cell_width: f32,
    cell_height: f32,
    scale_factor: f32,
) -> PhysicalSize<u32> {
    let padding = config.window.padding(scale_factor);

    let grid_width = cell_width * dimensions.columns.max(MIN_COLUMNS) as f32;
    let grid_height = cell_height * dimensions.lines.max(MIN_SCREEN_LINES) as f32;

    let width = (padding.0).mul_add(2., grid_width).floor();
    let height = (padding.1).mul_add(2., grid_height).floor();

    PhysicalSize::new(width as u32, height as u32)
}
