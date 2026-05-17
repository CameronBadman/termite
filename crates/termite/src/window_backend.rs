use std::{error::Error, sync::Arc, time::Instant};

use termite_core::TerminalCore;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use crate::{PtyChild, spawn_shell};
use crate::{
    plugins::PluginHost,
    runner::{FontConfig, Runner, RuntimeConfig, TerminalMetrics, TextRenderConfig},
    theme::Theme,
};

mod frame;
mod gpu;
mod input;
mod interaction;
mod perf;
mod pty_events;
mod render_cache;
mod selection;
mod terminal_io;
mod text;
mod zoom;

use gpu::{GpuRenderer, GpuRendererConfig};
use perf::{PerfStats, duration_ms};
use pty_events::{UserEvent, spawn_pty_reader};
use render_cache::RenderCache;
use selection::Selection;
use zoom::{load_zoom_steps, normalize_zoom_steps, scaled_font, scaled_metrics};

const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 540;

pub(crate) fn run(runner: Runner) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut state = WindowBackend::new(event_loop.create_proxy(), runner.into_runtime_config());
    event_loop.run_app(&mut state)?;
    Ok(())
}

struct WindowBackend {
    shell: String,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    terminal: Option<TerminalCore>,
    plugins: PluginHost,
    child: Option<PtyChild>,
    modifiers: ModifiersState,
    cols: u16,
    rows: u16,
    font: FontConfig,
    base_font: FontConfig,
    metrics: TerminalMetrics,
    base_metrics: TerminalMetrics,
    zoom_steps: i16,
    default_zoom_steps: i16,
    persist_zoom: bool,
    text_render: TextRenderConfig,
    theme: Theme,
    render_cache: RenderCache,
    selection: Option<Selection>,
    mouse_buttons: u8,
    mouse_position: Option<(u16, u16)>,
    animation_deadline: Option<Instant>,
    render_lower_deadline: Option<Instant>,
    render_upper_deadline: Option<Instant>,
    app_sync_deadline: Option<Instant>,
    frame_deadline: Option<Instant>,
    last_render: Option<Instant>,
    redraw_pending: bool,
    scroll_offset: usize,
    perf: PerfStats,
}

impl WindowBackend {
    fn new(proxy: EventLoopProxy<UserEvent>, config: RuntimeConfig) -> Self {
        let base_metrics = config.metrics;
        let default_zoom_steps = normalize_zoom_steps(config.zoom.default_steps);
        let zoom_steps = if config.zoom.persist {
            load_zoom_steps().unwrap_or(default_zoom_steps)
        } else {
            default_zoom_steps
        };
        let metrics = scaled_metrics(base_metrics, zoom_steps);
        let scaled_font = scaled_font(&config.font, zoom_steps);
        Self {
            shell: config.shell,
            proxy,
            window: None,
            renderer: None,
            terminal: None,
            plugins: config.plugins,
            child: None,
            modifiers: ModifiersState::empty(),
            cols: 1,
            rows: 1,
            font: scaled_font.clone(),
            base_font: config.font.clone(),
            metrics,
            base_metrics,
            zoom_steps,
            default_zoom_steps,
            persist_zoom: config.zoom.persist,
            text_render: config.text_render,
            theme: config.theme,
            render_cache: RenderCache::new(scaled_font, config.theme, metrics, config.text_render),
            selection: None,
            mouse_buttons: 0,
            mouse_position: None,
            animation_deadline: None,
            render_lower_deadline: None,
            render_upper_deadline: None,
            app_sync_deadline: None,
            frame_deadline: None,
            last_render: None,
            redraw_pending: false,
            scroll_offset: 0,
            perf: PerfStats::from_env(),
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        if self.window.is_some() {
            return Ok(());
        }

        let profile = self.perf.enabled;
        let total_started = Instant::now();
        let started = Instant::now();
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("termite")
                    .with_transparent(true)
                    .with_inner_size(LogicalSize::new(INITIAL_WIDTH, INITIAL_HEIGHT))
                    .with_min_inner_size(LogicalSize::new(320, 200)),
            )?,
        );
        let window_ms = duration_ms(started.elapsed());

        let started = Instant::now();
        let size = window.inner_size();
        let (cols, rows) = grid_size(size, self.metrics);
        let grid_ms = duration_ms(started.elapsed());
        let started = Instant::now();
        let mut child = spawn_shell(&self.shell, cols, rows)?;
        let pty_spawn_ms = duration_ms(started.elapsed());
        let started = Instant::now();
        spawn_pty_reader(&mut child, self.proxy.clone())?;
        let pty_reader_ms = duration_ms(started.elapsed());
        let started = Instant::now();
        let renderer = GpuRenderer::new(
            window.clone(),
            GpuRendererConfig {
                surface_size: size,
                texture_width: buffer_width(cols, self.metrics),
                texture_height: buffer_height(rows, self.metrics),
                metrics: self.metrics,
                background: self.theme.background,
                cursor_color: self.theme.cursor,
                profile,
            },
        )?;
        let renderer_ms = duration_ms(started.elapsed());

        let started = Instant::now();
        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        let mut terminal = TerminalCore::new(cols, rows);
        terminal.set_profile_enabled(profile);
        self.terminal = Some(terminal);
        self.renderer = Some(renderer);
        self.child = Some(child);
        self.window = Some(window);
        self.schedule_delayed_render(Instant::now());
        let state_ms = duration_ms(started.elapsed());
        if profile {
            eprintln!(
                concat!(
                    "termite-profile startup ",
                    "total={:.2}ms window={:.2}ms grid={:.2}ms renderer={:.2}ms ",
                    "pty_spawn={:.2}ms pty_reader={:.2}ms state={:.2}ms cols={} rows={}"
                ),
                duration_ms(total_started.elapsed()),
                window_ms,
                grid_ms,
                renderer_ms,
                pty_spawn_ms,
                pty_reader_ms,
                state_ms,
                cols,
                rows,
            );
        }
        Ok(())
    }
}

impl ApplicationHandler<UserEvent> for WindowBackend {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.initialize(event_loop) {
            eprintln!("termite: failed to initialize window backend: {error}");
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytes(bytes) => self.handle_pty_bytes(bytes),
            UserEvent::ChildExited => {
                self.perf.report_final();
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.timers_if_due(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.handle_resize(size),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => self.handle_mouse_move(position),
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => self.handle_key(event_loop, event),
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

pub(super) fn grid_size(size: PhysicalSize<u32>, metrics: TerminalMetrics) -> (u16, u16) {
    let cols = (size.width.max(metrics.cell_width) / metrics.cell_width)
        .clamp(1, u32::from(u16::MAX)) as u16;
    let rows = (size.height.max(metrics.cell_height) / metrics.cell_height)
        .clamp(1, u32::from(u16::MAX)) as u16;
    (cols, rows)
}

pub(super) fn buffer_width(cols: u16, metrics: TerminalMetrics) -> u32 {
    u32::from(cols) * metrics.cell_width
}

pub(super) fn buffer_height(rows: u16, metrics: TerminalMetrics) -> u32 {
    u32::from(rows) * metrics.cell_height
}

#[cfg(test)]
mod tests {
    use std::{
        hint::black_box,
        time::{Duration, Instant},
    };

    use super::text::{
        CellPaint, ascii_glyph_fallback, bitmap_glyph, box_segments, draw_block_cell,
        draw_box_cell, draw_shade_cell, sample_bitmap_axis,
    };
    use super::zoom::{
        MAX_ZOOM_STEPS, MIN_ZOOM_STEPS, ZoomAction, parse_zoom_steps, scaled_font, scaled_metrics,
        zoom_key_action,
    };
    use super::*;
    use crate::window_backend::render_cache::frame_len;
    use termite_core::TerminalCore;
    use winit::keyboard::Key;

    const BENCH_COLS: u16 = 120;
    const BENCH_ROWS: u16 = 36;
    const BENCH_CHUNK: usize = 8192;

    #[test]
    fn box_drawing_characters_are_drawn_without_font_fallback() {
        let metrics = TerminalMetrics::default();
        let width = metrics.cell_width as usize;
        let mut frame = vec![0; width * metrics.cell_height as usize * 4];

        assert!(draw_box_cell(
            &mut frame,
            width,
            0,
            0,
            '─',
            CellPaint {
                fg: [220, 224, 232],
                bg: [16, 18, 24],
                background_opaque: true,
                metrics,
            },
        ));

        let center_y = metrics.cell_height as usize / 2;
        let center_index = (center_y * width + width / 2) * 4;
        assert_eq!(&frame[center_index..center_index + 3], &[220, 224, 232]);
        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [16, 18, 24])
        );
    }

    #[test]
    fn non_box_characters_use_font_path() {
        assert_eq!(box_segments('A'), None);
    }

    #[test]
    fn block_characters_are_drawn_geometrically() {
        let metrics = TerminalMetrics::default();
        let width = metrics.cell_width as usize;
        let mut frame = vec![0; width * metrics.cell_height as usize * 4];

        assert!(draw_block_cell(
            &mut frame,
            width,
            0,
            0,
            '▄',
            CellPaint {
                fg: [220, 224, 232],
                bg: [16, 18, 24],
                background_opaque: true,
                metrics,
            },
        ));

        let top_index = (metrics.cell_height as usize / 4 * width + width / 2) * 4;
        let bottom_index = (metrics.cell_height as usize * 3 / 4 * width + width / 2) * 4;
        assert_eq!(&frame[top_index..top_index + 3], &[16, 18, 24]);
        assert_eq!(&frame[bottom_index..bottom_index + 3], &[220, 224, 232]);
    }

    #[test]
    fn shade_characters_are_drawn_as_mixed_pixels() {
        let metrics = TerminalMetrics::default();
        let width = metrics.cell_width as usize;
        let mut frame = vec![0; width * metrics.cell_height as usize * 4];

        assert!(draw_shade_cell(
            &mut frame,
            width,
            0,
            0,
            '▒',
            CellPaint {
                fg: [220, 224, 232],
                bg: [16, 18, 24],
                background_opaque: true,
                metrics,
            },
        ));

        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [220, 224, 232])
        );
        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [16, 18, 24])
        );
    }

    #[test]
    fn bitmap_font_covers_extended_terminal_glyphs() {
        assert!(bitmap_glyph('A').is_some());
        assert!(bitmap_glyph('é').is_some());
        assert!(bitmap_glyph('═').is_some());
        assert!(bitmap_glyph('█').is_some());
        assert!(bitmap_glyph('░').is_some());
    }

    #[test]
    fn smart_punctuation_has_ascii_glyph_fallbacks() {
        assert_eq!(ascii_glyph_fallback('’'), Some('\''));
        assert_eq!(ascii_glyph_fallback('“'), Some('"'));
        assert_eq!(ascii_glyph_fallback('—'), Some('-'));
        assert_eq!(ascii_glyph_fallback('…'), Some('.'));
    }

    #[test]
    fn bitmap_fallback_sampling_is_centered_for_larger_cells() {
        let metrics = TerminalMetrics::default();
        let samples: Vec<_> = (0..metrics.cell_width as usize)
            .map(|x| sample_bitmap_axis(x, metrics.cell_width as usize))
            .collect();

        assert_eq!(samples.first(), Some(&0));
        assert_eq!(samples.last(), Some(&7));
        assert!(samples.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn zoom_scales_metrics_and_ttf_size_from_base_config() {
        assert_eq!(
            scaled_metrics(
                TerminalMetrics {
                    cell_width: 10,
                    cell_height: 18,
                },
                2,
            ),
            TerminalMetrics {
                cell_width: 12,
                cell_height: 22,
            }
        );

        assert_eq!(
            scaled_font(
                &FontConfig::GlyphAtlas {
                    paths: vec!["/tmp/a.ttf".to_owned()],
                    size: 15.0,
                },
                2,
            ),
            FontConfig::GlyphAtlas {
                paths: vec!["/tmp/a.ttf".to_owned()],
                size: 17.0,
            }
        );
    }

    #[test]
    fn zoom_steps_are_clamped_for_config_and_state() {
        assert_eq!(parse_zoom_steps("3\n"), Some(3));
        assert_eq!(parse_zoom_steps("99"), Some(MAX_ZOOM_STEPS));
        assert_eq!(parse_zoom_steps("-99"), Some(MIN_ZOOM_STEPS));
        assert_eq!(parse_zoom_steps("not zoom"), None);
    }

    #[test]
    fn shifted_minus_character_is_zoom_out_key() {
        assert_eq!(
            zoom_key_action(
                &Key::Character("_".into()),
                ModifiersState::CONTROL | ModifiersState::SHIFT,
            ),
            Some(ZoomAction::Adjust(-1))
        );
    }

    #[test]
    fn base_frame_cache_updates_only_dirty_rows() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"A\x1b[2;1HB");
        let metrics = TerminalMetrics::default();
        let mut cache = RenderCache::new(
            FontConfig::Bitmap8x8,
            Theme::default(),
            metrics,
            TextRenderConfig::default(),
        );

        let frame = cache.update(terminal.grid());
        let first_row =
            frame[..metrics.cell_height as usize * 4 * metrics.cell_width as usize * 4].to_vec();

        let tick = terminal.process_pty_input(b"\x1b[2;2HC");
        cache.apply_damage(&tick.damage, terminal.grid().height());
        let frame = cache.update(terminal.grid());

        assert_eq!(
            &frame[..metrics.cell_height as usize * 4 * metrics.cell_width as usize * 4],
            first_row.as_slice()
        );
    }

    #[test]
    fn scrollback_cache_handles_resized_history_rows() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");
        let _ = terminal.resize(5, 2);
        let metrics = TerminalMetrics::default();
        let mut cache = RenderCache::new(
            FontConfig::Bitmap8x8,
            Theme::default(),
            metrics,
            TextRenderConfig::default(),
        );

        let frame = cache.update_scrollback(&terminal, 1);

        assert_eq!(frame.len(), frame_len(5, 2, metrics));
    }

    #[test]
    fn scrollback_cache_keeps_incremental_scrolls_clean() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"aa\r\nbb\r\ncc\r\ndd");
        let mut cache = RenderCache::new(
            FontConfig::Bitmap8x8,
            Theme::default(),
            TerminalMetrics::default(),
            TextRenderConfig::default(),
        );

        let _ = cache.update_scrollback(&terminal, 1);
        let _ = cache.update_scrollback(&terminal, 2);

        assert_eq!(cache.scroll_start, Some(0));
        assert!(!cache.dirty);
    }

    #[test]
    #[ignore]
    fn render_cache_bench() {
        let config = crate::config::runner().into_runtime_config();
        let workloads = [
            ("plain-scroll", bench_payload_plain_scroll()),
            ("color-table", bench_payload_color_table()),
            ("unicode", bench_payload_unicode()),
        ];

        println!(
            "{:<14} {:>9} {:>10} {:>10} {:>10}",
            "workload", "bytes", "core+draw", "draw-only", "updates"
        );
        for (name, payload) in workloads {
            let result = run_render_bench(
                &payload,
                config.font.clone(),
                config.metrics,
                config.theme,
                config.text_render,
            );
            println!(
                "{:<14} {:>9} {:>9.2}ms {:>9.2}ms {:>10}",
                name,
                payload.len(),
                result.total.as_secs_f64() * 1000.0,
                result.draw.as_secs_f64() * 1000.0,
                result.updates
            );
        }
    }

    struct RenderBenchResult {
        total: Duration,
        draw: Duration,
        updates: usize,
    }

    fn run_render_bench(
        payload: &[u8],
        font: FontConfig,
        metrics: TerminalMetrics,
        theme: Theme,
        text_render: TextRenderConfig,
    ) -> RenderBenchResult {
        let mut terminal = TerminalCore::new(BENCH_COLS, BENCH_ROWS);
        let mut cache = RenderCache::new(font, theme, metrics, text_render);
        let started = Instant::now();
        let mut draw = Duration::ZERO;
        let mut updates = 0;

        for chunk in payload.chunks(BENCH_CHUNK) {
            let tick = terminal.process_pty_input(chunk);
            cache.apply_damage(&tick.damage, terminal.grid().height());
            let draw_started = Instant::now();
            let frame = cache.update(terminal.grid());
            black_box(frame.len());
            let update = cache.take_texture_update(terminal.grid().height());
            black_box(update.full);
            black_box(update.rows.len());
            black_box(update.scrolls.len());
            draw += draw_started.elapsed();
            updates += 1;
        }

        RenderBenchResult {
            total: started.elapsed(),
            draw,
            updates,
        }
    }

    fn bench_payload_plain_scroll() -> Vec<u8> {
        let mut bytes = Vec::new();
        for i in 0..30_000 {
            bytes.extend_from_slice(
                format!("render line {i:05} abcdefghijklmnopqrstuvwxyz\r\n").as_bytes(),
            );
        }
        bytes
    }

    fn bench_payload_color_table() -> Vec<u8> {
        let mut bytes = Vec::new();
        for row in 0..12_000 {
            for color in 0..16 {
                bytes.extend_from_slice(
                    format!("\x1b[{}m{:02x} ", 30 + color % 8, row + color).as_bytes(),
                );
            }
            bytes.extend_from_slice(b"\x1b[0m\r\n");
        }
        bytes
    }

    fn bench_payload_unicode() -> Vec<u8> {
        let mut bytes = Vec::new();
        let sample = "unicode 表 λ π ┌─┐ █ ░ we’ll — ok\r\n";
        for _ in 0..25_000 {
            bytes.extend_from_slice(sample.as_bytes());
        }
        bytes
    }
}
