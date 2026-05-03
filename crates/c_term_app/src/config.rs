use crate::{
    plugins::{CursorLine, CursorLineConfig, CursorTrail, CursorTrailColor, CursorTrailConfig},
    runner::{Runner, RunnerPart, bitmap_font, font_file, parts},
};

const USE_TTF_FONT: bool = false;
const TTF_FONT_PATH: &str = "/usr/share/fonts/liberation-fonts/LiberationMono-Regular.ttf";

pub(crate) fn runner() -> Runner {
    Runner::new().with(basic_terminal())
}

fn basic_terminal() -> impl RunnerPart {
    parts().with(terminal_font()).with(cursor_overlays())
}

fn terminal_font() -> impl RunnerPart {
    if USE_TTF_FONT {
        font_file(TTF_FONT_PATH)
    } else {
        bitmap_font()
    }
}

fn cursor_overlays() -> impl RunnerPart {
    parts().with(cursor_line()).with(cursor_trail())
}

fn cursor_line() -> CursorLine {
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

fn cursor_trail() -> CursorTrail {
    CursorTrail::new(cursor_trail_config())
}

fn cursor_trail_config() -> CursorTrailConfig {
    CursorTrailConfig {
        hold_ms: 10,
        decay_ms: 320,
        fast_decay_ratio: 0.42,
        threshold: 2,
        length: 1.0,
        color: CursorTrailColor::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{FontConfig, font_file, parts};

    fn nested_plugins() -> impl RunnerPart {
        parts()
            .with(parts().with(cursor_line()))
            .with(parts().with(cursor_trail()))
    }

    #[test]
    fn runner_config_can_compose_plugin_groups() {
        let runner = Runner::new()
            .with(parts().with(cursor_line()))
            .with(parts().with(cursor_trail()));

        assert_eq!(runner.plugin_count(), 2);
    }

    #[test]
    fn runner_config_can_compose_nested_groups() {
        let runner = Runner::new().with(nested_plugins());

        assert_eq!(runner.plugin_count(), 2);
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
}
