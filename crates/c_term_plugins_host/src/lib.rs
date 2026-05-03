use c_term_core::CoreEvent;
use c_term_plugins_api::{
    ClipRect, CommandBuffer, DrawCommand, DrawPhase, PluginEvent, ResourceLimits, SubscriptionMask,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginHostError {
    DuplicatePluginId(String),
    ClipEscaped {
        plugin_id: String,
        allowed: ClipRect,
        requested: ClipRect,
    },
    ResourceLimitExceeded {
        plugin_id: String,
        resource: &'static str,
    },
}

pub type HostResult<T> = Result<T, PluginHostError>;

pub trait HostedPlugin: Send {
    fn on_events(&mut self, _events: &[PluginEvent]) -> HostResult<()> {
        Ok(())
    }

    fn draw(&mut self, _context: &mut HostDrawContext<'_>) -> HostResult<()> {
        Ok(())
    }
}

pub struct HostDrawContext<'a> {
    plugin_id: &'a str,
    phase: DrawPhase,
    frame_index: u64,
    allowed_clip: ClipRect,
    commands: &'a mut CommandBuffer,
}

impl<'a> HostDrawContext<'a> {
    pub fn new(
        plugin_id: &'a str,
        phase: DrawPhase,
        frame_index: u64,
        allowed_clip: ClipRect,
        commands: &'a mut CommandBuffer,
    ) -> Self {
        Self {
            plugin_id,
            phase,
            frame_index,
            allowed_clip,
            commands,
        }
    }

    pub fn phase(&self) -> DrawPhase {
        self.phase
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn allowed_clip(&self) -> ClipRect {
        self.allowed_clip
    }

    pub fn draw_quad(&mut self, clip: ClipRect) -> HostResult<()> {
        if !self.allowed_clip.contains(clip) {
            return Err(PluginHostError::ClipEscaped {
                plugin_id: self.plugin_id.to_owned(),
                allowed: self.allowed_clip,
                requested: clip,
            });
        }

        self.commands.push(DrawCommand::DrawQuad { clip });
        Ok(())
    }
}

pub struct PluginRegistration {
    pub id: String,
    pub subscriptions: SubscriptionMask,
    pub draw_phases: Vec<DrawPhase>,
    pub requires_frame_callbacks: bool,
    pub resource_limits: ResourceLimits,
    pub plugin: Box<dyn HostedPlugin>,
}

impl PluginRegistration {
    pub fn new(
        id: impl Into<String>,
        subscriptions: SubscriptionMask,
        draw_phases: Vec<DrawPhase>,
        plugin: Box<dyn HostedPlugin>,
    ) -> Self {
        Self {
            id: id.into(),
            subscriptions,
            draw_phases,
            requires_frame_callbacks: false,
            resource_limits: ResourceLimits::default(),
            plugin,
        }
    }

    pub fn with_frame_callbacks(mut self) -> Self {
        self.requires_frame_callbacks = true;
        self.subscriptions = self.subscriptions.union(SubscriptionMask::FRAME);
        self
    }
}

pub struct PluginHost {
    plugins: Vec<LoadedPlugin>,
    aggregate_subscriptions: SubscriptionMask,
}

struct LoadedPlugin {
    load_order: usize,
    registration: PluginRegistration,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            aggregate_subscriptions: SubscriptionMask::NONE,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn requires_frame_callbacks(&self) -> bool {
        self.aggregate_subscriptions
            .contains(SubscriptionMask::FRAME)
    }

    pub fn aggregate_subscriptions(&self) -> SubscriptionMask {
        self.aggregate_subscriptions
    }

    pub fn register(&mut self, registration: PluginRegistration) -> HostResult<()> {
        if self
            .plugins
            .iter()
            .any(|plugin| plugin.registration.id == registration.id)
        {
            return Err(PluginHostError::DuplicatePluginId(registration.id));
        }

        self.aggregate_subscriptions = self
            .aggregate_subscriptions
            .union(registration.subscriptions);
        self.plugins.push(LoadedPlugin {
            load_order: self.plugins.len(),
            registration,
        });
        Ok(())
    }

    pub fn dispatch_core_events(&mut self, events: &[CoreEvent]) -> HostResult<usize> {
        if self.plugins.is_empty() || events.is_empty() {
            return Ok(0);
        }

        let mapped: Vec<_> = events.iter().map(map_core_event).collect();
        self.dispatch_plugin_events(&mapped)
    }

    pub fn dispatch_plugin_events(&mut self, events: &[PluginEvent]) -> HostResult<usize> {
        if self.plugins.is_empty() || events.is_empty() {
            return Ok(0);
        }

        let event_mask = events.iter().fold(SubscriptionMask::NONE, |mask, event| {
            mask | event.kind().mask()
        });

        if !self.aggregate_subscriptions.contains_any(event_mask) {
            return Ok(0);
        }

        let mut deliveries = 0;
        for plugin in &mut self.plugins {
            let interested: Vec<_> = events
                .iter()
                .filter(|event| {
                    plugin
                        .registration
                        .subscriptions
                        .contains(event.kind().mask())
                })
                .cloned()
                .collect();
            if interested.is_empty() {
                continue;
            }
            plugin.registration.plugin.on_events(&interested)?;
            deliveries += 1;
        }
        Ok(deliveries)
    }

    pub fn draw_phase(
        &mut self,
        phase: DrawPhase,
        frame_index: u64,
        clip: ClipRect,
        commands: &mut CommandBuffer,
    ) -> HostResult<usize> {
        if self.plugins.is_empty() {
            return Ok(0);
        }

        let mut order: Vec<_> = self
            .plugins
            .iter_mut()
            .filter(|plugin| plugin.registration.draw_phases.contains(&phase))
            .collect();
        order.sort_by(|left, right| {
            left.registration
                .draw_phases
                .cmp(&right.registration.draw_phases)
                .then(left.load_order.cmp(&right.load_order))
                .then(left.registration.id.cmp(&right.registration.id))
        });

        let mut draws = 0;
        for plugin in order {
            let mut context =
                HostDrawContext::new(&plugin.registration.id, phase, frame_index, clip, commands);
            plugin.registration.plugin.draw(&mut context)?;
            draws += 1;
        }
        Ok(draws)
    }
}

trait ContainsAny {
    fn contains_any(self, other: Self) -> bool;
}

impl ContainsAny for SubscriptionMask {
    fn contains_any(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }
}

fn map_core_event(event: &CoreEvent) -> PluginEvent {
    match event {
        CoreEvent::KeyPress(keypress) => PluginEvent::KeyPress {
            logical_key: keypress.logical_key.clone(),
        },
        CoreEvent::CursorMoved { old, new } => PluginEvent::CursorMoved {
            old: (old.x, old.y),
            new: (new.x, new.y),
        },
        CoreEvent::CellChanged { x, y, cell } => PluginEvent::CellChanged {
            x: *x,
            y: *y,
            ch: cell.ch,
        },
        CoreEvent::LineChanged { y } => PluginEvent::LineChanged { y: *y },
        CoreEvent::ViewportChanged => PluginEvent::ViewportChanged,
        CoreEvent::ModeChanged { name, enabled } => PluginEvent::ModeChanged {
            name,
            enabled: *enabled,
        },
        CoreEvent::SelectionChanged => PluginEvent::SelectionChanged,
        CoreEvent::TitleChanged(title) => PluginEvent::TitleChanged(title.clone()),
        CoreEvent::Bell => PluginEvent::Bell,
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResourceLedger {
    limits: ResourceLimits,
    textures: u32,
    buffers: u32,
    bytes: u64,
}

impl ResourceLedger {
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            textures: 0,
            buffers: 0,
            bytes: 0,
        }
    }

    pub fn reserve_texture(
        &mut self,
        plugin_id: &str,
        width: u32,
        height: u32,
        bytes: u64,
    ) -> HostResult<()> {
        if self.textures + 1 > self.limits.max_textures
            || width > self.limits.max_texture_edge
            || height > self.limits.max_texture_edge
            || self.bytes.saturating_add(bytes) > self.limits.max_bytes
        {
            return Err(PluginHostError::ResourceLimitExceeded {
                plugin_id: plugin_id.to_owned(),
                resource: "texture",
            });
        }
        self.textures += 1;
        self.bytes += bytes;
        Ok(())
    }

    pub fn reserve_buffer(&mut self, plugin_id: &str, bytes: u64) -> HostResult<()> {
        if self.buffers + 1 > self.limits.max_buffers
            || self.bytes.saturating_add(bytes) > self.limits.max_bytes
        {
            return Err(PluginHostError::ResourceLimitExceeded {
                plugin_id: plugin_id.to_owned(),
                resource: "buffer",
            });
        }
        self.buffers += 1;
        self.bytes += bytes;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingPlugin {
        events: Arc<Mutex<Vec<PluginEvent>>>,
    }

    impl HostedPlugin for RecordingPlugin {
        fn on_events(&mut self, events: &[PluginEvent]) -> HostResult<()> {
            self.events.lock().unwrap().extend_from_slice(events);
            Ok(())
        }
    }

    struct DrawingPlugin;

    impl HostedPlugin for DrawingPlugin {
        fn draw(&mut self, context: &mut HostDrawContext<'_>) -> HostResult<()> {
            context.draw_quad(context.allowed_clip())
        }
    }

    #[test]
    fn empty_host_has_zero_dispatch_cost_path() {
        let mut host = PluginHost::new();

        assert_eq!(
            host.dispatch_plugin_events(&[PluginEvent::Bell]).unwrap(),
            0
        );
    }

    #[test]
    fn events_are_delivered_only_to_subscribers() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut host = PluginHost::new();
        host.register(PluginRegistration::new(
            "recorder",
            SubscriptionMask::BELL,
            Vec::new(),
            Box::new(RecordingPlugin {
                events: events.clone(),
            }),
        ))
        .unwrap();

        assert_eq!(
            host.dispatch_plugin_events(&[
                PluginEvent::KeyPress {
                    logical_key: "a".to_owned()
                },
                PluginEvent::Bell,
            ])
            .unwrap(),
            1
        );
        assert_eq!(events.lock().unwrap().as_slice(), &[PluginEvent::Bell]);
    }

    #[test]
    fn draw_context_enforces_clip() {
        let mut commands = CommandBuffer::default();
        let mut host = PluginHost::new();
        host.register(PluginRegistration::new(
            "drawer",
            SubscriptionMask::NONE,
            vec![DrawPhase::AboveText],
            Box::new(DrawingPlugin),
        ))
        .unwrap();

        let clip = ClipRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(
            host.draw_phase(DrawPhase::AboveText, 1, clip, &mut commands)
                .unwrap(),
            1
        );
        assert_eq!(commands.commands().len(), 1);
    }

    #[test]
    fn resource_ledger_rejects_over_budget_textures() {
        let mut ledger = ResourceLedger::new(ResourceLimits {
            max_textures: 1,
            max_buffers: 1,
            max_texture_edge: 16,
            max_bytes: 128,
        });

        assert!(ledger.reserve_texture("p", 16, 16, 64).is_ok());
        assert!(matches!(
            ledger.reserve_texture("p", 16, 16, 64),
            Err(PluginHostError::ResourceLimitExceeded { .. })
        ));
    }
}
