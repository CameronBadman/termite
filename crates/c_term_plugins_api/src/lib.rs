use std::ffi::{c_char, c_void};

pub const ABI_VERSION: AbiVersion = AbiVersion { major: 0, minor: 1 };

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiVersion {
    pub major: u16,
    pub minor: u16,
}

impl AbiVersion {
    pub const fn is_compatible_with(self, host: AbiVersion) -> bool {
        self.major == host.major && self.minor <= host.minor
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiStatus {
    Ok = 0,
    Error = 1,
    Unsupported = 2,
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubscriptionMask(pub u64);

impl SubscriptionMask {
    pub const NONE: Self = Self(0);
    pub const KEY_PRESS: Self = Self(1 << 0);
    pub const CURSOR_MOVED: Self = Self(1 << 1);
    pub const CELL_CHANGED: Self = Self(1 << 2);
    pub const LINE_CHANGED: Self = Self(1 << 3);
    pub const VIEWPORT_CHANGED: Self = Self(1 << 4);
    pub const MODE_CHANGED: Self = Self(1 << 5);
    pub const SELECTION_CHANGED: Self = Self(1 << 6);
    pub const TITLE_CHANGED: Self = Self(1 << 7);
    pub const BELL: Self = Self(1 << 8);
    pub const FRAME: Self = Self(1 << 9);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for SubscriptionMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DrawPhase {
    BackgroundOverlay = 0,
    BehindText = 10,
    AboveText = 20,
    AboveCursor = 30,
    UiOverlay = 40,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    KeyPress,
    CursorMoved,
    CellChanged,
    LineChanged,
    ViewportChanged,
    ModeChanged,
    SelectionChanged,
    TitleChanged,
    Bell,
    Frame,
}

impl EventKind {
    pub const fn mask(self) -> SubscriptionMask {
        match self {
            Self::KeyPress => SubscriptionMask::KEY_PRESS,
            Self::CursorMoved => SubscriptionMask::CURSOR_MOVED,
            Self::CellChanged => SubscriptionMask::CELL_CHANGED,
            Self::LineChanged => SubscriptionMask::LINE_CHANGED,
            Self::ViewportChanged => SubscriptionMask::VIEWPORT_CHANGED,
            Self::ModeChanged => SubscriptionMask::MODE_CHANGED,
            Self::SelectionChanged => SubscriptionMask::SELECTION_CHANGED,
            Self::TitleChanged => SubscriptionMask::TITLE_CHANGED,
            Self::Bell => SubscriptionMask::BELL,
            Self::Frame => SubscriptionMask::FRAME,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClipRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl ClipRect {
    pub fn contains(self, other: ClipRect) -> bool {
        let right = i64::from(self.x) + i64::from(self.width);
        let bottom = i64::from(self.y) + i64::from(self.height);
        let other_right = i64::from(other.x) + i64::from(other.width);
        let other_bottom = i64::from(other.y) + i64::from(other.height);

        other.x >= self.x && other.y >= self.y && other_right <= right && other_bottom <= bottom
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_textures: u32,
    pub max_buffers: u32,
    pub max_texture_edge: u32,
    pub max_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_textures: 16,
            max_buffers: 16,
            max_texture_edge: 4096,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostCapabilities {
    pub terminal_snapshots: bool,
    pub gpu_draw_context: bool,
    pub frame_timing: bool,
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TextureHandle(pub u64);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct BufferHandle(pub u64);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ShaderHandle(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEvent {
    KeyPress { logical_key: String },
    CursorMoved { old: (u16, u16), new: (u16, u16) },
    CellChanged { x: u16, y: u16, ch: char },
    LineChanged { y: u16 },
    ViewportChanged,
    ModeChanged { name: &'static str, enabled: bool },
    SelectionChanged,
    TitleChanged(String),
    Bell,
    Frame { frame_index: u64 },
}

impl PluginEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::KeyPress { .. } => EventKind::KeyPress,
            Self::CursorMoved { .. } => EventKind::CursorMoved,
            Self::CellChanged { .. } => EventKind::CellChanged,
            Self::LineChanged { .. } => EventKind::LineChanged,
            Self::ViewportChanged => EventKind::ViewportChanged,
            Self::ModeChanged { .. } => EventKind::ModeChanged,
            Self::SelectionChanged => EventKind::SelectionChanged,
            Self::TitleChanged(_) => EventKind::TitleChanged,
            Self::Bell => EventKind::Bell,
            Self::Frame { .. } => EventKind::Frame,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawCommand {
    SetClip(ClipRect),
    UseTexture(TextureHandle),
    DrawQuad { clip: ClipRect },
}

#[derive(Debug, Default)]
pub struct CommandBuffer {
    commands: Vec<DrawCommand>,
}

impl CommandBuffer {
    pub fn push(&mut self, command: DrawCommand) {
        self.commands.push(command);
    }

    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    pub fn clear(&mut self) {
        self.commands.clear();
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AbiEventBatch {
    pub events: *const c_void,
    pub len: usize,
}

#[repr(C)]
#[derive(Debug)]
pub struct AbiDrawContext {
    pub phase: DrawPhase,
    pub clip: ClipRect,
    pub frame_index: u64,
    pub host_context: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginVTable {
    pub create: unsafe extern "C" fn(host: *const HostApi) -> *mut c_void,
    pub destroy: unsafe extern "C" fn(instance: *mut c_void),
    pub on_events:
        unsafe extern "C" fn(instance: *mut c_void, events: *const AbiEventBatch) -> AbiStatus,
    pub draw:
        unsafe extern "C" fn(instance: *mut c_void, context: *mut AbiDrawContext) -> AbiStatus,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginDescriptor {
    pub abi_version: AbiVersion,
    pub plugin_id: *const c_char,
    pub plugin_name: *const c_char,
    pub subscriptions: SubscriptionMask,
    pub draw_phases: *const DrawPhase,
    pub draw_phase_len: usize,
    pub requires_frame_callbacks: bool,
    pub resource_limits: ResourceLimits,
    pub vtable: PluginVTable,
}

#[repr(C)]
#[derive(Debug)]
pub struct HostApi {
    pub abi_version: AbiVersion,
    pub capabilities: HostCapabilities,
    pub reserved: *mut c_void,
}

pub type PluginDescriptorFn = unsafe extern "C" fn() -> PluginDescriptor;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_masks_compose() {
        let mask = SubscriptionMask::KEY_PRESS | SubscriptionMask::FRAME;

        assert!(mask.contains(SubscriptionMask::KEY_PRESS));
        assert!(mask.contains(SubscriptionMask::FRAME));
        assert!(!mask.contains(SubscriptionMask::BELL));
    }

    #[test]
    fn clip_rect_containment_is_enforced_geometrically() {
        let outer = ClipRect {
            x: 10,
            y: 10,
            width: 100,
            height: 100,
        };
        let inner = ClipRect {
            x: 20,
            y: 20,
            width: 10,
            height: 10,
        };
        let escaping = ClipRect {
            x: 0,
            y: 20,
            width: 10,
            height: 10,
        };

        assert!(outer.contains(inner));
        assert!(!outer.contains(escaping));
    }
}
