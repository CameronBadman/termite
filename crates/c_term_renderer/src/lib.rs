use c_term_core::{DamageBatch, Grid};
use c_term_plugins_api::{ClipRect, CommandBuffer, DrawCommand, DrawPhase};
use c_term_plugins_host::{HostResult, PluginHost};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePolicy {
    EventDriven,
    AdaptiveContinuous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameReason {
    Damage,
    Resize,
    Exposed,
    CursorBlink,
    PluginAnimation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSnapshot {
    pub generation: u64,
    pub grid_width: u16,
    pub grid_height: u16,
}

impl RenderSnapshot {
    pub fn from_grid(grid: &Grid) -> Self {
        Self {
            generation: grid.generation(),
            grid_width: grid.width(),
            grid_height: grid.height(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderFrame {
    pub frame_index: u64,
    pub policy: FramePolicy,
    pub reason: FrameReason,
    pub snapshot_generation: u64,
    pub commands: Vec<DrawCommand>,
    pub plugin_draws: usize,
}

#[derive(Debug, Default)]
pub struct GlyphAtlas {
    generation: u64,
    glyph_count: usize,
}

impl GlyphAtlas {
    pub fn sync_visible_grid(&mut self, grid: &Grid, damage: &DamageBatch) {
        if damage.is_empty() {
            return;
        }
        self.generation = grid.generation();
        self.glyph_count = grid
            .visible_cells()
            .iter()
            .filter(|cell| cell.ch != ' ')
            .count();
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn glyph_count(&self) -> usize {
        self.glyph_count
    }
}

#[derive(Debug)]
pub struct Renderer {
    frame_index: u64,
    policy: FramePolicy,
    viewport_clip: ClipRect,
    atlas: GlyphAtlas,
}

impl Renderer {
    pub fn new(width_px: u32, height_px: u32) -> Self {
        Self {
            frame_index: 0,
            policy: FramePolicy::EventDriven,
            viewport_clip: ClipRect {
                x: 0,
                y: 0,
                width: width_px,
                height: height_px,
            },
            atlas: GlyphAtlas::default(),
        }
    }

    pub fn policy(&self) -> FramePolicy {
        self.policy
    }

    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    pub fn set_viewport_size(&mut self, width_px: u32, height_px: u32) {
        self.viewport_clip.width = width_px;
        self.viewport_clip.height = height_px;
    }

    pub fn sync_policy(&mut self, plugins: &PluginHost) {
        self.policy = if plugins.requires_frame_callbacks() {
            FramePolicy::AdaptiveContinuous
        } else {
            FramePolicy::EventDriven
        };
    }

    pub fn should_render(&self, damage: &DamageBatch, reason: FrameReason) -> bool {
        match self.policy {
            FramePolicy::EventDriven => {
                !damage.is_empty() || reason != FrameReason::PluginAnimation
            }
            FramePolicy::AdaptiveContinuous => true,
        }
    }

    pub fn render(
        &mut self,
        grid: &Grid,
        damage: &DamageBatch,
        reason: FrameReason,
        plugins: &mut PluginHost,
    ) -> HostResult<Option<RenderFrame>> {
        self.sync_policy(plugins);
        if !self.should_render(damage, reason) {
            return Ok(None);
        }

        self.frame_index += 1;
        self.atlas.sync_visible_grid(grid, damage);

        let snapshot = RenderSnapshot::from_grid(grid);
        let mut commands = CommandBuffer::default();

        self.render_builtin_layers(&mut commands);
        let plugin_draws = self.render_plugin_layers(plugins, &mut commands)?;

        Ok(Some(RenderFrame {
            frame_index: self.frame_index,
            policy: self.policy,
            reason,
            snapshot_generation: snapshot.generation,
            commands: commands.commands().to_vec(),
            plugin_draws,
        }))
    }

    fn render_builtin_layers(&self, commands: &mut CommandBuffer) {
        commands.push(DrawCommand::SetClip(self.viewport_clip));
        commands.push(DrawCommand::DrawQuad {
            clip: self.viewport_clip,
        });
    }

    fn render_plugin_layers(
        &mut self,
        plugins: &mut PluginHost,
        commands: &mut CommandBuffer,
    ) -> HostResult<usize> {
        let mut draws = 0;
        for phase in [
            DrawPhase::BackgroundOverlay,
            DrawPhase::BehindText,
            DrawPhase::AboveText,
            DrawPhase::AboveCursor,
            DrawPhase::UiOverlay,
        ] {
            draws += plugins.draw_phase(phase, self.frame_index, self.viewport_clip, commands)?;
        }
        Ok(draws)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use c_term_core::TerminalCore;
    use c_term_plugins_api::SubscriptionMask;
    use c_term_plugins_host::{HostedPlugin, PluginRegistration};

    struct NoopPlugin;

    impl HostedPlugin for NoopPlugin {}

    #[test]
    fn renderer_remains_event_driven_without_frame_plugins() {
        let mut renderer = Renderer::new(800, 600);
        let host = PluginHost::new();

        renderer.sync_policy(&host);

        assert_eq!(renderer.policy(), FramePolicy::EventDriven);
    }

    #[test]
    fn renderer_enters_adaptive_mode_for_frame_plugins() {
        let mut renderer = Renderer::new(800, 600);
        let mut host = PluginHost::new();
        host.register(
            PluginRegistration::new(
                "animator",
                SubscriptionMask::NONE,
                vec![DrawPhase::AboveText],
                Box::new(NoopPlugin),
            )
            .with_frame_callbacks(),
        )
        .unwrap();

        renderer.sync_policy(&host);

        assert_eq!(renderer.policy(), FramePolicy::AdaptiveContinuous);
    }

    #[test]
    fn damage_updates_glyph_atlas() {
        let mut terminal = TerminalCore::new(5, 1);
        let tick = terminal.process_pty_input(b"abc");
        let mut renderer = Renderer::new(800, 600);
        let mut host = PluginHost::new();

        let frame = renderer
            .render(
                terminal.grid(),
                &tick.damage,
                FrameReason::Damage,
                &mut host,
            )
            .unwrap()
            .unwrap();

        assert_eq!(frame.snapshot_generation, terminal.grid().generation());
        assert_eq!(renderer.atlas().glyph_count(), 3);
    }
}
