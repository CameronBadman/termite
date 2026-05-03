use crate::{
    plugins::{CursorLine, CursorLineConfig, CursorTrail, CursorTrailColor, CursorTrailConfig},
    plugins::{ScreenOpacity, ScreenOpacityConfig},
    runner::{Runner, RunnerPart, bitmap_font, font_file, parts, theme},
    theme::Theme,
};

const USE_TTF_FONT: bool = false;
const TTF_FONT_PATH: &str = "/usr/share/fonts/liberation-fonts/LiberationMono-Regular.ttf";

pub(crate) fn runner() -> Runner {
    Runner::new()
        .with(terminal_font())
        .with(terminal_theme())
        .with(terminal_plugins())
}

fn terminal_font() -> impl RunnerPart {
    if USE_TTF_FONT {
        font_file(TTF_FONT_PATH)
    } else {
        bitmap_font()
    }
}

fn terminal_theme() -> impl RunnerPart {
    theme(Theme {
        foreground: [224, 228, 232],
        background: [10, 12, 16],
        ansi: [
            [12, 12, 12],
            [230, 75, 95],
            [82, 196, 120],
            [229, 181, 103],
            [91, 156, 235],
            [190, 118, 235],
            [74, 207, 207],
            [210, 214, 220],
            [118, 124, 136],
            [255, 105, 125],
            [115, 225, 145],
            [245, 209, 125],
            [125, 180, 255],
            [215, 145, 255],
            [105, 235, 235],
            [245, 247, 250],
        ],
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
    ScreenOpacityConfig { opacity: 0.86 }
}

fn cursor_line_plugin() -> CursorLine {
    CursorLine::new(cursor_line_config())
}

fn cursor_line_config() -> CursorLineConfig {
    CursorLineConfig {
        row_color: [32, 80, 96],
        row_alpha: 48,
        cell_color: [255, 205, 96],
        cell_alpha: 64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{FontConfig, font_file, parts};

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
                path: "/tmp/font.ttf".to_owned(),
                size: 14.0,
            }
        );
    }

    #[test]
    fn runner_config_can_select_theme() {
        let runner = Runner::new().with(terminal_theme());

        assert_eq!(runner.theme().background, [10, 12, 16]);
        assert_eq!(runner.theme().ansi[1], [230, 75, 95]);
    }
}
