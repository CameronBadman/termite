use super::{Plugin, PluginFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenTintConfig {
    pub(crate) color: [u8; 3],
    pub(crate) alpha: u8,
}

impl Default for ScreenTintConfig {
    fn default() -> Self {
        Self {
            color: [0, 0, 0],
            alpha: 18,
        }
    }
}

pub(crate) struct ScreenTint {
    config: ScreenTintConfig,
}

impl ScreenTint {
    pub(crate) fn new(config: ScreenTintConfig) -> Self {
        Self { config }
    }
}

impl Default for ScreenTint {
    fn default() -> Self {
        Self::new(ScreenTintConfig::default())
    }
}

impl Plugin for ScreenTint {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        if self.config.alpha != 0 {
            frame.overlay_screen(self.config.color, self.config.alpha);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use c_term_core::TerminalCore;

    use super::*;

    #[test]
    fn screen_tint_emits_one_full_screen_overlay() {
        let terminal = TerminalCore::new(4, 2);
        let mut plugin = ScreenTint::new(ScreenTintConfig {
            color: [1, 2, 3],
            alpha: 24,
        });
        let mut frame = PluginFrame {
            grid: terminal.grid(),
            now: Instant::now(),
            overlays: Vec::new(),
        };

        assert!(!plugin.draw(&mut frame));

        assert_eq!(frame.overlays.len(), 1);
        assert_eq!(frame.overlays[0].color, [1, 2, 3]);
        assert_eq!(frame.overlays[0].alpha, 24);
    }
}
