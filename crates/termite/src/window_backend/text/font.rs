use std::fs;

use ab_glyph::{FontArc, PxScale};
use termite_core::Style;

use crate::runner::FontConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FontRole {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontRole {
    pub(super) fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }
}

pub(super) struct LoadedFont {
    pub(super) bytes: Vec<u8>,
    pub(super) font: FontArc,
    pub(super) role: FontRole,
    pub(super) symbol: bool,
    pub(super) scale: PxScale,
}

pub(super) fn load_fonts(font: FontConfig) -> Vec<LoadedFont> {
    let FontConfig::GlyphAtlas { paths, size } = font else {
        return Vec::new();
    };

    let mut loaded = Vec::new();
    for path in paths {
        if let Ok(bytes) = fs::read(&path)
            && let Ok(font) = FontArc::try_from_vec(bytes.clone())
        {
            loaded.push(LoadedFont {
                bytes,
                font,
                role: font_role_from_path(&path),
                symbol: is_symbol_font_path(&path),
                scale: PxScale::from(size),
            });
        }
    }
    loaded
}

pub(super) fn preferred_font_roles(style: Style) -> &'static [FontRole] {
    match (style.bold(), style.italic()) {
        (true, true) => &[
            FontRole::BoldItalic,
            FontRole::Bold,
            FontRole::Italic,
            FontRole::Regular,
        ],
        (true, false) => &[FontRole::Bold, FontRole::BoldItalic, FontRole::Regular],
        (false, true) => &[FontRole::Italic, FontRole::BoldItalic, FontRole::Regular],
        (false, false) => &[FontRole::Regular],
    }
}

pub(super) fn font_style_index(style: Style) -> usize {
    usize::from(style.attribute_bits() & 0b11)
}

fn font_role_from_path(path: &str) -> FontRole {
    let lower = path.to_ascii_lowercase();
    let bold = lower.contains("bold");
    let italic = lower.contains("italic") || lower.contains("oblique");
    match (bold, italic) {
        (true, true) => FontRole::BoldItalic,
        (true, false) => FontRole::Bold,
        (false, true) => FontRole::Italic,
        (false, false) => FontRole::Regular,
    }
}

fn is_symbol_font_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("symbolsnerd")
        || lower.contains("symbols-nerd")
        || lower.contains("nerd-font")
        || lower.contains("standardsymbols")
}
