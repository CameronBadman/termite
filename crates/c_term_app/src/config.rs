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
const MIN_THEME_CONTRAST: f32 = 7.0;
const ANSI_SATURATION: f32 = 1.28;
const TEXT_WEIGHT: f32 = 1.16;
const SYMBOL_WEIGHT: f32 = 1.0;
const TEXT_GAMMA: f32 = 0.92;
const SYMBOL_GAMMA: f32 = 1.0;
const THEME_FOREGROUND: [u8; 3] = [255, 255, 255];
const THEME_BACKGROUND: [u8; 3] = [6, 7, 10];
const THEME_ANSI: [[u8; 3]; 16] = [
    [7, 8, 12],
    [255, 45, 76],
    [30, 255, 110],
    [255, 205, 45],
    [50, 150, 255],
    [220, 70, 255],
    [28, 238, 255],
    [232, 236, 248],
    [125, 132, 154],
    [255, 72, 105],
    [78, 255, 142],
    [255, 226, 72],
    [92, 180, 255],
    [236, 104, 255],
    [80, 250, 255],
    [255, 255, 255],
];
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
    theme(readable_theme(
        THEME_FOREGROUND,
        THEME_BACKGROUND,
        THEME_ANSI,
        ANSI_SATURATION,
        MIN_THEME_CONTRAST,
    ))
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
        row_color: [22, 112, 146],
        row_alpha: 0,
        cell_color: [255, 218, 88],
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
        color: CursorTrailColor::Auto,
    }
}

fn metrics_for_font(size: f32, width_ratio: f32, height_ratio: f32) -> TerminalMetrics {
    TerminalMetrics {
        cell_width: (size * width_ratio).round().max(1.0) as u32,
        cell_height: (size * height_ratio).round().max(1.0) as u32,
    }
}

fn readable_theme(
    foreground: [u8; 3],
    background: [u8; 3],
    ansi: [[u8; 3]; 16],
    saturation: f32,
    min_contrast: f32,
) -> Theme {
    Theme {
        foreground: ensure_contrast(foreground, background, min_contrast),
        background,
        ansi: ansi.map(|color| ensure_contrast(saturate(color, saturation), background, 3.0)),
    }
}

fn saturate(color: [u8; 3], amount: f32) -> [u8; 3] {
    let gray =
        0.2126 * f32::from(color[0]) + 0.7152 * f32::from(color[1]) + 0.0722 * f32::from(color[2]);
    color.map(|channel| {
        (gray + (f32::from(channel) - gray) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

fn ensure_contrast(mut foreground: [u8; 3], background: [u8; 3], min_ratio: f32) -> [u8; 3] {
    let target = if luminance(background) < 0.5 {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    };
    for _ in 0..16 {
        if contrast_ratio(foreground, background) >= min_ratio {
            return foreground;
        }
        foreground = mix(foreground, target, 0.18);
    }
    foreground
}

fn mix(color: [u8; 3], target: [u8; 3], amount: f32) -> [u8; 3] {
    [
        mix_channel(color[0], target[0], amount),
        mix_channel(color[1], target[1], amount),
        mix_channel(color[2], target[2], amount),
    ]
}

fn mix_channel(value: u8, target: u8, amount: f32) -> u8 {
    (f32::from(value) + (f32::from(target) - f32::from(value)) * amount)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f32 {
    let a = luminance(a) + 0.05;
    let b = luminance(b) + 0.05;
    if a > b { a / b } else { b / a }
}

fn luminance(color: [u8; 3]) -> f32 {
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

fn linear(channel: u8) -> f32 {
    let channel = f32::from(channel) / 255.0;
    if channel <= 0.03928 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
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

        assert_eq!(runner.theme().background, [6, 7, 10]);
        assert_eq!(
            runner.theme().ansi[1],
            saturate([255, 45, 76], ANSI_SATURATION)
        );
        assert!(
            contrast_ratio(runner.theme().foreground, runner.theme().background)
                >= MIN_THEME_CONTRAST
        );
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
