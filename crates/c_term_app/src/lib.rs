use c_term_core::{CoreTick, DamageBatch, KeyPress, TerminalCore};
use c_term_plugins_host::{HostResult, PluginHost};
use c_term_renderer::{FrameReason, RenderFrame, Renderer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    PtyBytes(Vec<u8>),
    KeyPress(KeyPress),
    ResizeCells { width: u16, height: u16 },
    ResizePixels { width: u32, height: u32 },
    Exposed,
    CursorBlink,
    PluginAnimationFrame,
}

pub struct TerminalApp {
    core: TerminalCore,
    renderer: Renderer,
    plugins: PluginHost,
    pending_damage: Option<DamageBatch>,
}

impl TerminalApp {
    pub fn new(cols: u16, rows: u16, width_px: u32, height_px: u32) -> Self {
        Self {
            core: TerminalCore::new(cols, rows),
            renderer: Renderer::new(width_px, height_px),
            plugins: PluginHost::new(),
            pending_damage: None,
        }
    }

    pub fn core(&self) -> &TerminalCore {
        &self.core
    }

    pub fn plugins(&self) -> &PluginHost {
        &self.plugins
    }

    pub fn plugins_mut(&mut self) -> &mut PluginHost {
        &mut self.plugins
    }

    pub fn handle_action(&mut self, action: AppAction) -> HostResult<Option<RenderFrame>> {
        match action {
            AppAction::PtyBytes(bytes) => {
                let tick = self.core.process_pty_input(&bytes);
                self.after_core_tick(tick, FrameReason::Damage)
            }
            AppAction::KeyPress(keypress) => {
                let tick = self.core.handle_keypress(keypress);
                self.after_core_tick(tick, FrameReason::Damage)
            }
            AppAction::ResizeCells { width, height } => {
                let tick = self.core.resize(width, height);
                self.after_core_tick(tick, FrameReason::Resize)
            }
            AppAction::ResizePixels { width, height } => {
                self.renderer.set_viewport_size(width, height);
                self.render_from_pending_damage(FrameReason::Resize)
            }
            AppAction::Exposed => self.render_from_pending_damage(FrameReason::Exposed),
            AppAction::CursorBlink => self.render_from_pending_damage(FrameReason::CursorBlink),
            AppAction::PluginAnimationFrame => {
                self.render_from_pending_damage(FrameReason::PluginAnimation)
            }
        }
    }

    fn after_core_tick(
        &mut self,
        tick: CoreTick,
        reason: FrameReason,
    ) -> HostResult<Option<RenderFrame>> {
        self.plugins.dispatch_core_events(&tick.events)?;
        self.pending_damage = Some(tick.damage);
        self.render_from_pending_damage(reason)
    }

    fn render_from_pending_damage(
        &mut self,
        reason: FrameReason,
    ) -> HostResult<Option<RenderFrame>> {
        let damage = self.pending_damage.take().unwrap_or_else(|| DamageBatch {
            generation: self.core.grid().generation(),
            regions: Vec::new(),
        });
        self.renderer
            .render(self.core.grid(), &damage, reason, &mut self.plugins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_bytes_flow_from_core_to_renderer() {
        let mut app = TerminalApp::new(10, 2, 800, 600);
        let frame = app
            .handle_action(AppAction::PtyBytes(b"hello".to_vec()))
            .unwrap()
            .unwrap();

        assert_eq!(app.core().grid().cell(0, 0).unwrap().ch, 'h');
        assert_eq!(frame.plugin_draws, 0);
    }

    #[test]
    fn plugin_animation_does_not_render_without_frame_plugin_or_damage() {
        let mut app = TerminalApp::new(10, 2, 800, 600);
        let _ = app.handle_action(AppAction::PtyBytes(Vec::new())).unwrap();

        assert!(
            app.handle_action(AppAction::PluginAnimationFrame)
                .unwrap()
                .is_none()
        );
    }
}
