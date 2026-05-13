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
            Color::Indexed(index) => self
                .ansi
                .get(usize::from(index))
                .copied()
                .unwrap_or(self.foreground),
            Color::Rgb(r, g, b) => [r, g, b],
        }
    }
}
