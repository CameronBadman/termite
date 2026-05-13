use crate::{
    plugins::{CursorLine, CursorLineConfig, CursorTrail, CursorTrailColor, CursorTrailConfig},
    plugins::{ScreenOpacity, ScreenOpacityConfig},
    runner::{
        Runner, RunnerPart, TerminalMetrics, TextRenderConfig, ZoomConfig, bitmap_font,
        font_files_with_size, parts, terminal_metrics, terminal_zoom, text_render, theme,
    },
    theme::Theme,
};

const USE_TTF_FONT: bool = true;
const DEFAULT_ZOOM_STEPS: i16 = 0;
const PERSIST_ZOOM: bool = true;
const FONT_SIZE: f32 = 17.4;
const CELL_WIDTH_RATIO: f32 = 0.46;
const CELL_HEIGHT_RATIO: f32 = 1.32;
const TEXT_WEIGHT: f32 = 1.16;
const SYMBOL_WEIGHT: f32 = 1.0;
const TEXT_GAMMA: f32 = 0.92;
const SYMBOL_GAMMA: f32 = 1.0;
const THEME_FOREGROUND: [u8; 3] = [205, 214, 244];
const THEME_BACKGROUND: [u8; 3] = [30, 30, 46];
const THEME_ANSI: [[u8; 3]; 16] = [
    [69, 71, 90],
    [243, 139, 168],
    [166, 227, 161],
    [249, 226, 175],
    [137, 180, 250],
    [245, 194, 231],
    [148, 226, 213],
    [186, 194, 222],
    [88, 91, 112],
    [243, 139, 168],
    [166, 227, 161],
    [249, 226, 175],
    [137, 180, 250],
    [245, 194, 231],
    [148, 226, 213],
    [166, 173, 200],
];
const KITTY_CURSOR: [u8; 3] = [245, 224, 220];
const TTF_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/liberation-fonts/LiberationMono-Regular.ttf",
    "/usr/share/fonts/liberation-fonts/LiberationMono-Bold.ttf",
    "/usr/share/fonts/liberation-fonts/LiberationMono-Italic.ttf",
    "/usr/share/fonts/liberation-fonts/LiberationMono-BoldItalic.ttf",
    "/usr/share/fonts/symbols-nerd-font/SymbolsNerdFontMono-Regular.ttf",
    "/usr/share/fonts/urw-fonts/StandardSymbolsPS.ttf",
    "/usr/share/fonts/urw-fonts/NimbusMonoPS-Regular.otf",
    "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
];

pub(crate) fn runner() -> Runner {
    Runner::new()
        .with(terminal_font())
        .with(terminal_metrics(terminal_default_metrics()))
        .with(terminal_theme())
        .with(terminal_text_render())
        .with(terminal_zoom_config())
        .with(terminal_plugins())
}

fn terminal_font() -> impl RunnerPart {
    if USE_TTF_FONT {
        font_files_with_size(TTF_FONT_PATHS.iter().copied(), FONT_SIZE)
    } else {
        bitmap_font()
    }
}

fn terminal_theme() -> impl RunnerPart {
    theme(Theme {
        foreground: THEME_FOREGROUND,
        background: THEME_BACKGROUND,
        cursor: KITTY_CURSOR,
        ansi: THEME_ANSI,
    })
}

fn terminal_text_render() -> impl RunnerPart {
    text_render(TextRenderConfig {
        text_weight: TEXT_WEIGHT,
        symbol_weight: SYMBOL_WEIGHT,
        text_gamma: TEXT_GAMMA,
        symbol_gamma: SYMBOL_GAMMA,
    })
}

fn terminal_default_metrics() -> TerminalMetrics {
    metrics_for_font(FONT_SIZE, CELL_WIDTH_RATIO, CELL_HEIGHT_RATIO)
}

fn terminal_zoom_config() -> impl RunnerPart {
    terminal_zoom(ZoomConfig {
        default_steps: DEFAULT_ZOOM_STEPS,
        persist: PERSIST_ZOOM,
    })
}

fn terminal_plugins() -> impl RunnerPart {
    parts()
        .with(screen_opacity_plugin())
        .with(cursor_line_plugin())
        .with(cursor_trail_plugin())
}

fn screen_opacity_plugin() -> ScreenOpacity {
    ScreenOpacity::new(screen_opacity_config())
}

fn screen_opacity_config() -> ScreenOpacityConfig {
    ScreenOpacityConfig { opacity: 0.9 }
}

fn cursor_line_plugin() -> CursorLine {
    CursorLine::new(cursor_line_config())
}

fn cursor_line_config() -> CursorLineConfig {
    CursorLineConfig {
        row_color: KITTY_CURSOR,
        row_alpha: 0,
        cell_color: KITTY_CURSOR,
        cell_alpha: 40,
    }
}

fn cursor_trail_plugin() -> CursorTrail {
    CursorTrail::new(cursor_trail_config())
}

fn cursor_trail_config() -> CursorTrailConfig {
    CursorTrailConfig {
        hold_ms: 10,
        decay_ms: 320,
        fast_decay_ratio: 0.42,
        threshold: 2,
        color: CursorTrailColor::Rgb(KITTY_CURSOR),
    }
}

fn metrics_for_font(size: f32, width_ratio: f32, height_ratio: f32) -> TerminalMetrics {
    TerminalMetrics {
        cell_width: (size * width_ratio).round().max(1.0) as u32,
        cell_height: (size * height_ratio).round().max(1.0) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{FontConfig, font_file, font_files_with_size, parts};

    fn nested_plugins() -> impl RunnerPart {
        parts()
            .with(parts().with(screen_opacity_plugin()))
            .with(parts().with(cursor_line_plugin()))
            .with(parts().with(cursor_trail_plugin()))
    }

    #[test]
    fn runner_config_can_compose_plugin_groups() {
        let runner = Runner::new()
            .with(parts().with(screen_opacity_plugin()))
            .with(parts().with(cursor_line_plugin()))
            .with(parts().with(cursor_trail_plugin()));

        assert_eq!(runner.plugin_count(), 3);
    }

    #[test]
    fn runner_config_can_compose_nested_groups() {
        let runner = Runner::new().with(nested_plugins());

        assert_eq!(runner.plugin_count(), 3);
    }

    #[test]
    fn runner_config_can_select_font() {
        let runner = Runner::new().with(font_file("/tmp/font.ttf"));

        assert_eq!(
            runner.font(),
            &FontConfig::GlyphAtlas {
                paths: vec!["/tmp/font.ttf".to_owned()],
                size: 14.0,
            }
        );
    }

    #[test]
    fn runner_config_can_select_font_stack() {
        let runner = Runner::new().with(font_files_with_size(["/tmp/a.ttf", "/tmp/b.ttf"], 16.0));

        assert_eq!(
            runner.font(),
            &FontConfig::GlyphAtlas {
                paths: vec!["/tmp/a.ttf".to_owned(), "/tmp/b.ttf".to_owned()],
                size: 16.0,
            }
        );
    }

    #[test]
    fn runner_config_can_select_theme() {
        let runner = Runner::new().with(terminal_theme());

        assert_eq!(runner.theme().foreground, [205, 214, 244]);
        assert_eq!(runner.theme().background, [30, 30, 46]);
        assert_eq!(runner.theme().cursor, [245, 224, 220]);
        assert_eq!(runner.theme().ansi[1], [243, 139, 168]);
        assert_eq!(runner.theme().ansi[15], [166, 173, 200]);
    }

    #[test]
    fn runner_config_can_select_terminal_metrics() {
        let runner = Runner::new().with(terminal_metrics(TerminalMetrics {
            cell_width: 12,
            cell_height: 20,
        }));

        assert_eq!(
            runner.metrics(),
            TerminalMetrics {
                cell_width: 12,
                cell_height: 20,
            }
        );
    }

    #[test]
    fn runner_config_uses_font_derived_default_metrics() {
        let runner = Runner::new().with(terminal_metrics(terminal_default_metrics()));

        assert_eq!(
            runner.metrics(),
            TerminalMetrics {
                cell_width: 8,
                cell_height: 23,
            }
        );
    }

    #[test]
    fn runner_config_can_select_text_rendering() {
        let runner = Runner::new().with(terminal_text_render());

        assert_eq!(
            runner.text_render(),
            TextRenderConfig {
                text_weight: TEXT_WEIGHT,
                symbol_weight: SYMBOL_WEIGHT,
                text_gamma: TEXT_GAMMA,
                symbol_gamma: SYMBOL_GAMMA,
            }
        );
    }

    #[test]
    fn runner_config_can_select_default_zoom() {
        let runner = Runner::new().with(terminal_zoom_config());

        assert_eq!(
            runner.zoom(),
            ZoomConfig {
                default_steps: DEFAULT_ZOOM_STEPS,
                persist: PERSIST_ZOOM,
            }
        );
    }

    #[test]
    fn runner_config_clamps_zero_terminal_metrics() {
        let runner = Runner::new().with(terminal_metrics(TerminalMetrics {
            cell_width: 0,
            cell_height: 0,
        }));

        assert_eq!(
            runner.metrics(),
            TerminalMetrics {
                cell_width: 1,
                cell_height: 1,
            }
        );
    }
}
