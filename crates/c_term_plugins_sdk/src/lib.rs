pub use c_term_plugins_api as api;

use api::{CommandBuffer, DrawPhase, PluginEvent, ResourceLimits, SubscriptionMask};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMetadata {
    pub id: &'static str,
    pub name: &'static str,
    pub subscriptions: SubscriptionMask,
    pub draw_phases: &'static [DrawPhase],
    pub requires_frame_callbacks: bool,
    pub resource_limits: ResourceLimits,
}

impl PluginMetadata {
    pub const fn new(
        id: &'static str,
        name: &'static str,
        subscriptions: SubscriptionMask,
        draw_phases: &'static [DrawPhase],
    ) -> Self {
        Self {
            id,
            name,
            subscriptions,
            draw_phases,
            requires_frame_callbacks: false,
            resource_limits: ResourceLimits {
                max_textures: 16,
                max_buffers: 16,
                max_texture_edge: 4096,
                max_bytes: 64 * 1024 * 1024,
            },
        }
    }

    pub const fn with_frame_callbacks(mut self) -> Self {
        self.requires_frame_callbacks = true;
        self.subscriptions = self.subscriptions.union(SubscriptionMask::FRAME);
        self
    }
}

pub trait TerminalPlugin {
    fn metadata(&self) -> PluginMetadata;

    fn on_events(&mut self, _events: &[PluginEvent]) {}

    fn draw(&mut self, _context: &mut PluginDrawContext<'_>) {}
}

pub struct PluginDrawContext<'a> {
    phase: DrawPhase,
    frame_index: u64,
    commands: &'a mut CommandBuffer,
}

impl<'a> PluginDrawContext<'a> {
    pub fn new(phase: DrawPhase, frame_index: u64, commands: &'a mut CommandBuffer) -> Self {
        Self {
            phase,
            frame_index,
            commands,
        }
    }

    pub fn phase(&self) -> DrawPhase {
        self.phase
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn commands(&mut self) -> &mut CommandBuffer {
        self.commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_callback_metadata_adds_frame_subscription() {
        static PHASES: [DrawPhase; 1] = [DrawPhase::AboveText];
        let metadata =
            PluginMetadata::new("example", "Example", SubscriptionMask::KEY_PRESS, &PHASES)
                .with_frame_callbacks();

        assert!(metadata.requires_frame_callbacks);
        assert!(metadata.subscriptions.contains(SubscriptionMask::FRAME));
    }
}
