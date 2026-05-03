use super::{Plugin, PluginFrame};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScreenOpacityConfig {
    pub(crate) opacity: f32,
}

impl Default for ScreenOpacityConfig {
    fn default() -> Self {
        Self { opacity: 0.86 }
    }
}

pub(crate) struct ScreenOpacity {
    config: ScreenOpacityConfig,
}

impl ScreenOpacity {
    pub(crate) fn new(config: ScreenOpacityConfig) -> Self {
        Self { config }
    }
}

impl Default for ScreenOpacity {
    fn default() -> Self {
        Self::new(ScreenOpacityConfig::default())
    }
}

impl Plugin for ScreenOpacity {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        frame.set_screen_opacity(self.config.opacity);
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use c_term_core::TerminalCore;

    use super::*;

    #[test]
    fn screen_opacity_sets_frame_opacity() {
        let terminal = TerminalCore::new(4, 2);
        let mut plugin = ScreenOpacity::new(ScreenOpacityConfig { opacity: 0.7 });
        let mut frame = PluginFrame {
            grid: terminal.grid(),
            now: Instant::now(),
            overlays: Vec::new(),
            screen_opacity: 1.0,
        };

        assert!(!plugin.draw(&mut frame));

        assert_eq!(frame.screen_opacity, 0.7);
    }
}
