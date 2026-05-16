use c_term_core::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Theme {
    pub(crate) foreground: [u8; 3],
    pub(crate) background: [u8; 3],
    pub(crate) cursor: [u8; 3],
    pub(crate) ansi: [[u8; 3]; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            foreground: [220, 224, 232],
            background: [16, 18, 24],
            cursor: [245, 224, 220],
            ansi: [
                [12, 12, 12],
                [197, 15, 31],
                [19, 161, 14],
                [193, 156, 0],
                [0, 55, 218],
                [136, 23, 152],
                [58, 150, 221],
                [204, 204, 204],
                [118, 118, 118],
                [231, 72, 86],
                [22, 198, 12],
                [249, 241, 165],
                [59, 120, 255],
                [180, 0, 158],
                [97, 214, 214],
                [242, 242, 242],
            ],
        }
    }
}

impl Theme {
    pub(crate) fn color(self, color: Color) -> [u8; 3] {
        match color {
            Color::DefaultForeground => self.foreground,
            Color::DefaultBackground => self.background,
            Color::Indexed(index) => self.indexed_color(index),
            Color::Rgb(r, g, b) => [r, g, b],
        }
    }

    fn indexed_color(self, index: u8) -> [u8; 3] {
        if let Some(color) = self.ansi.get(usize::from(index)) {
            return *color;
        }

        if index < 232 {
            let cube_index = index - 16;
            return [
                color_cube_channel(cube_index / 36),
                color_cube_channel((cube_index / 6) % 6),
                color_cube_channel(cube_index % 6),
            ];
        }

        let level = 8 + (index - 232) * 10;
        [level, level, level]
    }
}

fn color_cube_channel(value: u8) -> u8 {
    if value == 0 { 0 } else { 55 + value * 40 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_maps_standard_ansi_indexes_from_configured_palette() {
        let theme = Theme::default();

        assert_eq!(theme.color(Color::Indexed(1)), theme.ansi[1]);
        assert_eq!(theme.color(Color::Indexed(15)), theme.ansi[15]);
    }

    #[test]
    fn theme_maps_xterm_256_color_cube_indexes() {
        let theme = Theme::default();

        assert_eq!(theme.color(Color::Indexed(16)), [0, 0, 0]);
        assert_eq!(theme.color(Color::Indexed(21)), [0, 0, 255]);
        assert_eq!(theme.color(Color::Indexed(196)), [255, 0, 0]);
        assert_eq!(theme.color(Color::Indexed(46)), [0, 255, 0]);
    }

    #[test]
    fn theme_maps_xterm_grayscale_indexes() {
        let theme = Theme::default();

        assert_eq!(theme.color(Color::Indexed(232)), [8, 8, 8]);
        assert_eq!(theme.color(Color::Indexed(255)), [238, 238, 238]);
    }
}
