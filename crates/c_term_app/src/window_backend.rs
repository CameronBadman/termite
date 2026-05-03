use std::{
    borrow::Cow,
    collections::HashMap,
    error::Error,
    fs,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};
use c_term_core::{
    Cell, Color, CursorShape, DamageBatch, DamageRegion, Grid, MouseState, MouseTracking, Style,
    TerminalCore,
};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use crate::{PtyChild, set_pty_winsize, spawn_shell};
use crate::{
    plugins::{OverlayCommand, OverlayKind, PluginFrame, PluginHost},
    runner::{FontConfig, Runner},
};

pub(crate) const CELL_WIDTH: u32 = 8;
pub(crate) const CELL_HEIGHT: u32 = 16;
const ANIMATION_FRAME_MS: u64 = 8;
const FRAME_INTERVAL_MS: u64 = 8;
const DELAYED_RENDER_LOWER_US: u64 = 150;
const DELAYED_RENDER_UPPER_NS: u64 = 4_000_000;
const APP_SYNC_TIMEOUT_MS: u64 = 1_000;
const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 540;
const MAX_OVERLAYS: usize = 16;
const OVERLAY_BYTES: usize = 64;

#[derive(Debug)]
enum UserEvent {
    PtyBytes(Vec<u8>),
    ChildExited,
}

pub(crate) fn run(runner: Runner) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let (shell, plugins, font) = runner.into_parts();
    let mut state = WindowBackend::new(shell, event_loop.create_proxy(), plugins, font);
    event_loop.run_app(&mut state)?;
    Ok(())
}

struct WindowBackend {
    shell: String,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    terminal: Option<TerminalCore>,
    plugins: PluginHost,
    child: Option<PtyChild>,
    modifiers: ModifiersState,
    cols: u16,
    rows: u16,
    render_cache: RenderCache,
    present_frame: Vec<u8>,
    mouse_buttons: u8,
    mouse_position: Option<(u16, u16)>,
    animation_deadline: Option<Instant>,
    render_lower_deadline: Option<Instant>,
    render_upper_deadline: Option<Instant>,
    app_sync_deadline: Option<Instant>,
    frame_deadline: Option<Instant>,
    last_render: Option<Instant>,
    redraw_pending: bool,
    scroll_offset: usize,
}

struct RenderCache {
    frame: Vec<u8>,
    dirty_rows: Vec<bool>,
    upload_rows: Vec<bool>,
    upload_full: bool,
    scrolls: Vec<ScrollDamage>,
    upload_scrolls: Vec<ScrollDamage>,
    dirty: bool,
    scroll_start: Option<usize>,
    text: TextRenderer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScrollDamage {
    top: u16,
    bottom: u16,
    count: u16,
    down: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowBand {
    start: u16,
    end: u16,
}

#[derive(Debug, Default)]
struct TextureUpdate {
    full: bool,
    scrolls: Vec<ScrollDamage>,
    rows: Vec<RowBand>,
}

impl TextureUpdate {
    fn full() -> Self {
        Self {
            full: true,
            ..Default::default()
        }
    }
}

struct GpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    scratch_texture: wgpu::Texture,
    texture_width: u32,
    texture_height: u32,
    cursor_buffer: wgpu::Buffer,
    overlay_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl GpuRenderer {
    fn new(
        window: Arc<Window>,
        surface_size: PhysicalSize<u32>,
        texture_width: u32,
        texture_height: u32,
    ) -> Result<Self, Box<dyn Error>> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("c-term device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))?;
        let surface_width = surface_size.width.max(1);
        let surface_height = surface_size.height.max(1);
        let config = surface
            .get_default_config(&adapter, surface_width, surface_height)
            .ok_or("surface is not supported by the selected GPU adapter")?;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("c-term blit shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(BLIT_SHADER)),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("c-term blit pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState::from(config.format))],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("c-term nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let cursor_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("c-term global uniform"),
            size: 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let overlay_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("c-term overlay storage"),
            size: (MAX_OVERLAYS * OVERLAY_BYTES) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let texture = create_frame_texture(&device, texture_width, texture_height);
        let scratch_texture = create_scratch_texture(&device, texture_width, texture_height);
        let bind_group = create_frame_bind_group(
            &device,
            &pipeline,
            &sampler,
            &texture,
            &cursor_buffer,
            &overlay_buffer,
        );

        Ok(Self {
            surface,
            device,
            queue,
            config,
            texture,
            scratch_texture,
            texture_width,
            texture_height,
            cursor_buffer,
            overlay_buffer,
            bind_group,
            pipeline,
        })
    }

    fn resize_surface(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    fn resize_texture(&mut self, width: u32, height: u32) {
        if width == self.texture_width && height == self.texture_height {
            return;
        }
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("c-term nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let texture = create_frame_texture(&self.device, width, height);
        let scratch_texture = create_scratch_texture(&self.device, width, height);
        let bind_group = create_frame_bind_group(
            &self.device,
            &self.pipeline,
            &sampler,
            &texture,
            &self.cursor_buffer,
            &self.overlay_buffer,
        );
        self.texture = texture;
        self.scratch_texture = scratch_texture;
        self.bind_group = bind_group;
        self.texture_width = width;
        self.texture_height = height;
    }

    fn render(
        &mut self,
        frame: &[u8],
        update: &TextureUpdate,
        cursor: [f32; 4],
        overlays: &[OverlayCommand],
    ) -> Result<(), &'static str> {
        let expected_len = self.texture_width as usize * self.texture_height as usize * 4;
        if frame.len() != expected_len {
            return Err("frame size does not match GPU texture size");
        }

        self.write_globals(cursor, overlays.len().min(MAX_OVERLAYS));
        self.write_overlays(overlays);
        if update.full {
            self.write_frame(frame);
        } else {
            self.copy_scrolled_rows(&update.scrolls);
            self.write_row_bands(frame, &update.rows);
        }

        self.present()
    }

    fn write_globals(&self, cursor: [f32; 4], overlay_count: usize) {
        let mut bytes = [0_u8; 32];
        for (chunk, value) in bytes[..16].chunks_exact_mut(4).zip(cursor) {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        bytes[16..20].copy_from_slice(&(overlay_count as f32).to_ne_bytes());
        self.queue.write_buffer(&self.cursor_buffer, 0, &bytes);
    }

    fn write_overlays(&self, overlays: &[OverlayCommand]) {
        let mut bytes = vec![0_u8; MAX_OVERLAYS * OVERLAY_BYTES];
        for (index, overlay) in overlays.iter().take(MAX_OVERLAYS).enumerate() {
            write_overlay(
                &mut bytes[index * OVERLAY_BYTES..(index + 1) * OVERLAY_BYTES],
                overlay,
            );
        }
        self.queue.write_buffer(&self.overlay_buffer, 0, &bytes);
    }

    fn write_frame(&self, frame: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            frame,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.texture_width * 4),
                rows_per_image: Some(self.texture_height),
            },
            wgpu::Extent3d {
                width: self.texture_width,
                height: self.texture_height,
                depth_or_array_layers: 1,
            },
        );
    }

    fn write_row_bands(&self, frame: &[u8], rows: &[RowBand]) {
        let row_bytes = self.texture_width as usize * 4;
        for band in rows {
            let start_px = u32::from(band.start) * CELL_HEIGHT;
            let height_px = u32::from(band.end.saturating_sub(band.start)) * CELL_HEIGHT;
            if height_px == 0 {
                continue;
            }
            let start = start_px as usize * row_bytes;
            let len = height_px as usize * row_bytes;
            let Some(data) = frame.get(start..start + len) else {
                continue;
            };
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: start_px,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.texture_width * 4),
                    rows_per_image: Some(height_px),
                },
                wgpu::Extent3d {
                    width: self.texture_width,
                    height: height_px,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn copy_scrolled_rows(&self, scrolls: &[ScrollDamage]) {
        if scrolls.is_empty() {
            return;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("c-term scroll copy encoder"),
            });
        for scroll in scrolls {
            let rows = scroll.bottom.saturating_sub(scroll.top).saturating_add(1);
            let count = scroll.count.min(rows);
            let copy_rows = rows.saturating_sub(count);
            if copy_rows == 0 {
                continue;
            }
            let top_px = u32::from(scroll.top) * CELL_HEIGHT;
            let count_px = u32::from(count) * CELL_HEIGHT;
            let copy_height = u32::from(copy_rows) * CELL_HEIGHT;
            let source_y = if scroll.down {
                top_px
            } else {
                top_px + count_px
            };
            let destination_y = if scroll.down {
                top_px + count_px
            } else {
                top_px
            };
            let size = wgpu::Extent3d {
                width: self.texture_width,
                height: copy_height,
                depth_or_array_layers: 1,
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: source_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.scratch_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: source_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                size,
            );
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.scratch_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: source_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: destination_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                size,
            );
        }
        self.queue.submit(Some(encoder.finish()));
    }

    fn present(&mut self) -> Result<(), &'static str> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err("failed to acquire GPU surface"),
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("c-term render encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("c-term render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

fn create_frame_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("c-term frame texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn create_scratch_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("c-term scroll scratch texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn create_frame_bind_group(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    sampler: &wgpu::Sampler,
    texture: &wgpu::Texture,
    cursor_buffer: &wgpu::Buffer,
    overlay_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let layout = pipeline.get_bind_group_layout(0);
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("c-term frame bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: cursor_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: overlay_buffer.as_entire_binding(),
            },
        ],
    })
}

fn write_overlay(bytes: &mut [u8], overlay: &OverlayCommand) {
    let kind = match overlay.kind {
        OverlayKind::Rect => 0.0,
        OverlayKind::Quad => 1.0,
        OverlayKind::QuadRing => 2.0,
    };
    let values = [
        kind,
        0.0,
        0.0,
        0.0,
        f32::from(overlay.color[0]) / 255.0,
        f32::from(overlay.color[1]) / 255.0,
        f32::from(overlay.color[2]) / 255.0,
        f32::from(overlay.alpha) / 255.0,
        overlay.corners[0].0,
        overlay.corners[0].1,
        overlay.corners[1].0,
        overlay.corners[1].1,
        overlay.corners[2].0,
        overlay.corners[2].1,
        overlay.corners[3].0,
        overlay.corners[3].1,
    ];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_ne_bytes());
    }
}

const BLIT_SHADER: &str = r#"
struct Globals {
    rect: vec4<f32>,
    overlay: vec4<f32>,
};

struct Overlay {
    data: vec4<f32>,
    color: vec4<f32>,
    corners_a: vec4<f32>,
    corners_b: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[index], 0.0, 1.0);
    out.uv = uvs[index];
    return out;
}

@group(0) @binding(0) var frame_texture: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;
@group(0) @binding(2) var<uniform> globals: Globals;
@group(0) @binding(3) var<storage, read> overlays: array<Overlay, 16>;

fn smoothstep_local(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = clamp((value - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn distance_to_segment(point: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let delta = b - a;
    let len2 = dot(delta, delta);
    if (len2 <= 0.0001) {
        return distance(point, a);
    }
    let t = clamp(dot(point - a, delta) / len2, 0.0, 1.0);
    return distance(point, a + delta * t);
}

fn point_in_quad(point: vec2<f32>, corners: array<vec2<f32>, 4>) -> bool {
    var winding = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let a = corners[i];
        let b = corners[(i + 1u) % 4u];
        let cross = (b.x - a.x) * (point.y - a.y) - (b.y - a.y) * (point.x - a.x);
        if (abs(cross) < 0.001) {
            continue;
        }
        if (winding == 0.0) {
            winding = sign(cross);
        } else if (winding * cross < 0.0) {
            return false;
        }
    }
    return true;
}

fn quad_edge_distance(point: vec2<f32>, corners: array<vec2<f32>, 4>) -> f32 {
    var result = 999999.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        result = min(result, distance_to_segment(point, corners[i], corners[(i + 1u) % 4u]));
    }
    return result;
}

fn overlay_alpha(point: vec2<f32>, overlay: Overlay) -> f32 {
    let kind = overlay.data.x;
    let corners = array<vec2<f32>, 4>(
        overlay.corners_a.xy,
        overlay.corners_a.zw,
        overlay.corners_b.xy,
        overlay.corners_b.zw,
    );
    if (kind < 0.5) {
        let left = min(corners[2].x, corners[0].x);
        let right = max(corners[2].x, corners[0].x);
        let top = min(corners[3].y, corners[1].y);
        let bottom = max(corners[3].y, corners[1].y);
        if (point.x >= left && point.x < right && point.y >= top && point.y < bottom) {
            return overlay.color.a;
        }
        return 0.0;
    }
    if (!point_in_quad(point, corners)) {
        return 0.0;
    }
    let edge_distance = quad_edge_distance(point, corners);
    if (kind < 1.5) {
        return overlay.color.a * smoothstep_local(0.0, 1.25, edge_distance);
    }
    return overlay.color.a * (1.0 - smoothstep_local(1.0, 3.0, edge_distance));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var color = textureSample(frame_texture, frame_sampler, in.uv);
    let dims = vec2<f32>(textureDimensions(frame_texture));
    let pixel = in.uv * dims;
    let count = min(u32(globals.overlay.x), 16u);
    for (var i = 0u; i < count; i = i + 1u) {
        let overlay = overlays[i];
        let alpha = overlay_alpha(pixel, overlay);
        if (alpha > 0.0) {
            color = vec4<f32>(mix(color.rgb, overlay.color.rgb, alpha), color.a);
        }
    }
    let rect = globals.rect;
    if (rect.z > 0.0 &&
        pixel.x >= rect.x && pixel.x < rect.x + rect.z &&
        pixel.y >= rect.y && pixel.y < rect.y + rect.w) {
        color = vec4<f32>(vec3<f32>(1.0) - color.rgb, color.a);
    }
    return color;
}
"#;

impl RenderCache {
    fn new(font: FontConfig) -> Self {
        Self {
            frame: Vec::new(),
            dirty_rows: Vec::new(),
            upload_rows: Vec::new(),
            upload_full: true,
            scrolls: Vec::new(),
            upload_scrolls: Vec::new(),
            dirty: true,
            scroll_start: None,
            text: TextRenderer::new(font),
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.frame = vec![0; frame_len(cols, rows)];
        self.dirty_rows = vec![false; usize::from(rows)];
        self.upload_rows = vec![false; usize::from(rows)];
        self.scrolls.clear();
        self.upload_scrolls.clear();
        self.dirty = true;
        self.upload_full = true;
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        self.upload_full = true;
    }

    fn apply_damage(&mut self, damage: &DamageBatch, rows: u16) {
        for region in &damage.regions {
            match *region {
                DamageRegion::Viewport => self.invalidate(),
                DamageRegion::Scroll {
                    top,
                    bottom,
                    count,
                    down,
                } => {
                    if top <= bottom && count > 0 && rows > 0 {
                        let bottom = bottom.min(rows - 1);
                        if let Some(last) = self.scrolls.last_mut()
                            && last.top == top
                            && last.bottom == bottom
                            && last.down == down
                        {
                            last.count = last.count.saturating_add(count);
                            continue;
                        }
                        self.scrolls.push(ScrollDamage {
                            top,
                            bottom,
                            count,
                            down,
                        });
                    }
                }
                DamageRegion::Cells { y, height, .. } => {
                    let end = y.saturating_add(height).min(rows);
                    for row in y..end {
                        if let Some(dirty) = self.dirty_rows.get_mut(usize::from(row)) {
                            *dirty = true;
                        }
                    }
                }
                DamageRegion::Cursor { .. } => {}
            }
        }
    }

    fn update(&mut self, grid: &Grid) -> &[u8] {
        self.scroll_start = None;
        self.update_rows(grid.width(), grid.height(), |y| grid.row(y))
    }

    fn update_scrollback(&mut self, terminal: &TerminalCore, scroll_offset: usize) -> &[u8] {
        let grid = terminal.grid();
        let cols = grid.width();
        let rows = grid.height();
        let height = usize::from(rows);
        let history_len = terminal.scrollback_len();
        let total_rows = history_len + height;
        let start = total_rows
            .saturating_sub(height)
            .saturating_sub(scroll_offset.min(history_len));

        self.ensure_shape(cols, rows);
        let width = usize::from(cols) * CELL_WIDTH as usize;
        let previous = self.scroll_start;

        if self.dirty || previous.is_none() {
            self.frame.fill(0);
            for y in 0..rows {
                if let Some(row) = scrollback_row_at(terminal, start + usize::from(y)) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y, cols);
                }
            }
            self.dirty_rows.fill(false);
            self.dirty = false;
            self.scroll_start = Some(start);
            self.upload_full = true;
            return &self.frame;
        }

        let previous = previous.unwrap_or(start);
        if previous == start {
            return &self.frame;
        }

        let row_bytes = CELL_HEIGHT as usize * width * 4;
        let delta = start as isize - previous as isize;
        let distance = delta.unsigned_abs();
        if distance >= height {
            self.frame.fill(0);
            for y in 0..rows {
                if let Some(row) = scrollback_row_at(terminal, start + usize::from(y)) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y, cols);
                }
            }
        } else if delta > 0 {
            self.frame
                .copy_within(distance * row_bytes..height * row_bytes, 0);
            for y in height - distance..height {
                clear_grid_row(&mut self.frame, width, y as u16);
                if let Some(row) = scrollback_row_at(terminal, start + y) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y as u16, cols);
                }
            }
        } else {
            self.frame
                .copy_within(0..(height - distance) * row_bytes, distance * row_bytes);
            for y in 0..distance {
                clear_grid_row(&mut self.frame, width, y as u16);
                if let Some(row) = scrollback_row_at(terminal, start + y) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y as u16, cols);
                }
            }
        }

        self.dirty_rows.fill(false);
        self.dirty = false;
        self.upload_full = true;
        self.scroll_start = Some(start);
        &self.frame
    }

    fn update_rows<'a>(
        &mut self,
        cols: u16,
        rows: u16,
        mut row_at: impl FnMut(u16) -> Option<&'a [Cell]>,
    ) -> &[u8] {
        let expected_len = frame_len(cols, rows);
        if self.frame.len() != expected_len {
            self.frame.resize(expected_len, 0);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.dirty_rows.len() != usize::from(rows) {
            self.dirty_rows.resize(usize::from(rows), true);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.upload_rows.len() != usize::from(rows) {
            self.upload_rows.resize(usize::from(rows), false);
            self.upload_full = true;
        }

        let width = usize::from(cols) * CELL_WIDTH as usize;
        if self.dirty {
            self.frame.fill(0);
            for y in 0..rows {
                if let Some(row) = row_at(y) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y, cols);
                }
            }
            self.dirty_rows.fill(false);
            self.dirty = false;
            self.scrolls.clear();
            self.upload_scrolls.clear();
            self.upload_rows.fill(false);
            self.upload_full = true;
            return &self.frame;
        }

        self.apply_scrolls(width, cols, rows, &mut row_at);

        for y in 0..rows {
            let Some(dirty) = self.dirty_rows.get_mut(usize::from(y)) else {
                continue;
            };
            if !*dirty {
                continue;
            }
            clear_grid_row(&mut self.frame, width, y);
            if let Some(row) = row_at(y) {
                self.text
                    .draw_row_to_frame(row, &mut self.frame, width, y, cols);
            }
            *dirty = false;
            if let Some(upload) = self.upload_rows.get_mut(usize::from(y)) {
                *upload = true;
            }
        }

        &self.frame
    }

    fn apply_scrolls<'a>(
        &mut self,
        width: usize,
        cols: u16,
        rows: u16,
        row_at: &mut impl FnMut(u16) -> Option<&'a [Cell]>,
    ) {
        let scrolls = std::mem::take(&mut self.scrolls);
        for scroll in scrolls {
            let top = scroll.top.min(rows.saturating_sub(1));
            let bottom = scroll.bottom.min(rows.saturating_sub(1));
            if top > bottom {
                continue;
            }
            let row_count = bottom - top + 1;
            let count = scroll.count.min(row_count);
            if count == 0 {
                continue;
            }

            let row_bytes = CELL_HEIGHT as usize * width * 4;
            let start = usize::from(top) * row_bytes;
            let end = (usize::from(bottom) + 1) * row_bytes;
            let count_bytes = usize::from(count) * row_bytes;
            if usize::from(count) >= usize::from(row_count) {
                for y in top..=bottom {
                    self.redraw_uploaded_row(width, cols, y, row_at);
                }
                continue;
            }

            if scroll.down {
                self.frame
                    .copy_within(start..end - count_bytes, start + count_bytes);
                for y in top..top + count {
                    self.redraw_uploaded_row(width, cols, y, row_at);
                }
            } else {
                self.frame.copy_within(start + count_bytes..end, start);
                for y in bottom + 1 - count..=bottom {
                    self.redraw_uploaded_row(width, cols, y, row_at);
                }
            }
            self.upload_scrolls.push(ScrollDamage {
                top,
                bottom,
                count,
                down: scroll.down,
            });
        }
    }

    fn redraw_uploaded_row<'a>(
        &mut self,
        width: usize,
        cols: u16,
        y: u16,
        row_at: &mut impl FnMut(u16) -> Option<&'a [Cell]>,
    ) {
        clear_grid_row(&mut self.frame, width, y);
        if let Some(row) = row_at(y) {
            self.text
                .draw_row_to_frame(row, &mut self.frame, width, y, cols);
        }
        if let Some(upload) = self.upload_rows.get_mut(usize::from(y)) {
            *upload = true;
        }
    }

    fn take_texture_update(&mut self, rows: u16) -> TextureUpdate {
        if self.upload_full {
            self.upload_full = false;
            self.upload_rows.fill(false);
            self.upload_scrolls.clear();
            return TextureUpdate::full();
        }

        let mut update = TextureUpdate {
            scrolls: std::mem::take(&mut self.upload_scrolls),
            ..Default::default()
        };
        let row_limit = usize::from(rows).min(self.upload_rows.len());
        let mut y = 0;
        while y < row_limit {
            if !self.upload_rows[y] {
                y += 1;
                continue;
            }
            let start = y;
            while y < row_limit && self.upload_rows[y] {
                self.upload_rows[y] = false;
                y += 1;
            }
            update.rows.push(RowBand {
                start: start as u16,
                end: y as u16,
            });
        }
        update
    }

    fn ensure_shape(&mut self, cols: u16, rows: u16) {
        let expected_len = frame_len(cols, rows);
        if self.frame.len() != expected_len {
            self.frame.resize(expected_len, 0);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.dirty_rows.len() != usize::from(rows) {
            self.dirty_rows.resize(usize::from(rows), true);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.upload_rows.len() != usize::from(rows) {
            self.upload_rows.resize(usize::from(rows), false);
            self.upload_full = true;
        }
    }
}

impl WindowBackend {
    fn new(
        shell: String,
        proxy: EventLoopProxy<UserEvent>,
        plugins: PluginHost,
        font: FontConfig,
    ) -> Self {
        Self {
            shell,
            proxy,
            window: None,
            renderer: None,
            terminal: None,
            plugins,
            child: None,
            modifiers: ModifiersState::empty(),
            cols: 1,
            rows: 1,
            render_cache: RenderCache::new(font),
            present_frame: Vec::new(),
            mouse_buttons: 0,
            mouse_position: None,
            animation_deadline: None,
            render_lower_deadline: None,
            render_upper_deadline: None,
            app_sync_deadline: None,
            frame_deadline: None,
            last_render: None,
            redraw_pending: false,
            scroll_offset: 0,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        if self.window.is_some() {
            return Ok(());
        }

        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("c-term")
                    .with_inner_size(LogicalSize::new(INITIAL_WIDTH, INITIAL_HEIGHT))
                    .with_min_inner_size(LogicalSize::new(320, 200)),
            )?,
        );

        let size = window.inner_size();
        let (cols, rows) = grid_size(size);
        let renderer = GpuRenderer::new(
            window.clone(),
            size,
            buffer_width(cols),
            buffer_height(rows),
        )?;
        let mut child = spawn_shell(&self.shell, cols, rows)?;
        spawn_pty_reader(&mut child, self.proxy.clone())?;

        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        self.present_frame = vec![0; frame_len(cols, rows)];
        self.terminal = Some(TerminalCore::new(cols, rows));
        self.renderer = Some(renderer);
        self.child = Some(child);
        self.window = Some(window);
        self.mark_dirty();
        Ok(())
    }

    fn mark_dirty(&mut self) {
        self.request_frame(Instant::now());
    }

    fn request_frame(&mut self, now: Instant) {
        if self.redraw_pending {
            return;
        }

        if let Some(last_render) = self.last_render {
            let next_frame = last_render + Duration::from_millis(FRAME_INTERVAL_MS);
            if now < next_frame {
                self.frame_deadline = Some(
                    self.frame_deadline
                        .map_or(next_frame, |deadline| deadline.min(next_frame)),
                );
                return;
            }
        }

        self.request_redraw_now();
    }

    fn request_redraw_now(&mut self) {
        if self.redraw_pending {
            return;
        }
        if let Some(window) = &self.window {
            self.redraw_pending = true;
            self.frame_deadline = None;
            window.request_redraw();
        }
    }

    fn schedule_animation(&mut self, now: Instant) {
        self.animation_deadline = Some(now + Duration::from_millis(ANIMATION_FRAME_MS));
    }

    fn schedule_delayed_render(&mut self, now: Instant) {
        self.render_lower_deadline = Some(now + Duration::from_micros(DELAYED_RENDER_LOWER_US));
        if self.render_upper_deadline.is_none() {
            self.render_upper_deadline = Some(now + Duration::from_nanos(DELAYED_RENDER_UPPER_NS));
        }
    }

    fn disarm_delayed_render(&mut self) {
        self.render_lower_deadline = None;
        self.render_upper_deadline = None;
    }

    fn apply_damage(&mut self, damage: &DamageBatch) {
        if self.scroll_offset == 0 {
            self.render_cache.apply_damage(damage, self.rows);
        } else {
            self.render_cache.invalidate();
        }
    }

    fn handle_pty_bytes(&mut self, bytes: Vec<u8>) {
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        let tick = terminal.process_pty_input(&bytes);
        let synchronized = terminal.grid().is_synchronized();
        if terminal.is_alternate_screen() && self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.render_cache.invalidate();
        }
        let max_scroll_offset = terminal.scrollback_len();
        if self.scroll_offset > max_scroll_offset {
            self.scroll_offset = max_scroll_offset;
            self.render_cache.invalidate();
        }
        let output = tick.output;
        self.apply_damage(&tick.damage);
        let now = Instant::now();
        if synchronized {
            self.disarm_delayed_render();
            self.app_sync_deadline = Some(now + Duration::from_millis(APP_SYNC_TIMEOUT_MS));
        } else {
            self.app_sync_deadline = None;
            self.schedule_delayed_render(now);
        }
        if !output.is_empty()
            && let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&output)
        {
            eprintln!("c-term: failed to write terminal response to PTY: {error}");
        }
    }

    fn handle_resize(&mut self, size: PhysicalSize<u32>) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };

        let width = size.width.max(1);
        let height = size.height.max(1);
        renderer.resize_surface(width, height);

        let (cols, rows) = grid_size(size);
        if cols == self.cols && rows == self.rows {
            self.mark_dirty();
            return;
        }

        if let Some(child) = &mut self.child
            && let Err(error) = set_pty_winsize(child.master.as_raw_fd(), cols, rows)
        {
            eprintln!("c-term: failed to resize PTY: {error}");
        }

        if let Some(terminal) = &mut self.terminal {
            let tick = terminal.resize(cols, rows);
            self.apply_damage(&tick.damage);
        }

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.resize_texture(buffer_width(cols), buffer_height(rows));

        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        self.present_frame = vec![0; frame_len(cols, rows)];
        self.mark_dirty();
    }

    fn scroll_view(&mut self, delta: isize) -> bool {
        let Some(terminal) = &self.terminal else {
            return false;
        };
        if terminal.is_alternate_screen() {
            return false;
        }

        let max_offset = terminal.scrollback_len();
        let next = if delta >= 0 {
            self.scroll_offset.saturating_add(delta as usize)
        } else {
            self.scroll_offset.saturating_sub(delta.unsigned_abs())
        }
        .min(max_offset);

        if next == self.scroll_offset {
            return false;
        }
        self.scroll_offset = next;
        self.mark_dirty();
        true
    }

    fn snap_to_live(&mut self) {
        if self.scroll_offset == 0 {
            return;
        }
        self.scroll_offset = 0;
        self.render_cache.invalidate();
        self.mark_dirty();
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.modifiers.control_key() && matches!(event.logical_key.as_ref(), Key::Character("q"))
        {
            event_loop.exit();
            return;
        }

        if self.modifiers.shift_key() {
            match event.logical_key.as_ref() {
                Key::Named(NamedKey::PageUp) => {
                    let lines = self.rows.saturating_sub(1).max(1) as isize;
                    let _ = self.scroll_view(lines);
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    let lines = self.rows.saturating_sub(1).max(1) as isize;
                    let _ = self.scroll_view(-lines);
                    return;
                }
                _ => {}
            }
        }

        let Some(bytes) = encode_window_key(&event, self.modifiers) else {
            return;
        };

        self.snap_to_live();
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("c-term: failed to write key to PTY: {error}");
        }
    }

    fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        let Some(cell) = self.mouse_position else {
            return;
        };
        let Some(button_code) = mouse_button_code(button) else {
            return;
        };

        match state {
            ElementState::Pressed => self.mouse_buttons |= 1 << button_code,
            ElementState::Released => self.mouse_buttons &= !(1 << button_code),
        }

        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        if mouse.tracking == MouseTracking::None {
            return;
        }

        let code = if state == ElementState::Pressed {
            button_code
        } else {
            3
        };
        self.write_mouse_event(mouse, code, cell, state == ElementState::Released);
    }

    fn handle_mouse_move(&mut self, position: PhysicalPosition<f64>) {
        let Some(cell) = mouse_cell(position, self.cols, self.rows) else {
            self.mouse_position = None;
            return;
        };
        if self.mouse_position == Some(cell) {
            return;
        }
        self.mouse_position = Some(cell);

        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        let code = match mouse.tracking {
            MouseTracking::Any => 35,
            MouseTracking::Drag if self.mouse_buttons != 0 => {
                active_mouse_button(self.mouse_buttons) + 32
            }
            _ => return,
        };
        self.write_mouse_event(mouse, code, cell, false);
    }

    fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        if mouse.tracking == MouseTracking::None {
            for lines in wheel_scroll_lines(delta) {
                let _ = self.scroll_view(lines);
            }
            return;
        }

        let Some(cell) = self.mouse_position else {
            return;
        };
        for code in wheel_codes(delta) {
            self.write_mouse_event(mouse, code, cell, false);
        }
    }

    fn write_mouse_event(&mut self, mouse: MouseState, code: u8, cell: (u16, u16), release: bool) {
        let bytes = encode_mouse_event(mouse, code, cell.0, cell.1, self.modifiers, release);
        if bytes.is_empty() {
            return;
        }
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("c-term: failed to write mouse event to PTY: {error}");
        }
    }

    fn render(&mut self) {
        self.redraw_pending = false;
        self.frame_deadline = None;
        let render_started = Instant::now();
        self.disarm_delayed_render();
        let Some(terminal) = &self.terminal else {
            return;
        };

        let viewing_history = self.scroll_offset > 0 && !terminal.is_alternate_screen();
        if viewing_history {
            self.render_cache
                .update_scrollback(terminal, self.scroll_offset);
        } else {
            self.render_cache.update(terminal.grid());
        }
        let texture_update = self.render_cache.take_texture_update(self.rows);

        let Some(renderer) = &mut self.renderer else {
            return;
        };

        self.present_frame.copy_from_slice(&self.render_cache.frame);
        let frame = self.present_frame.as_mut_slice();
        let (plugin_active, overlays) = if !viewing_history {
            let mut plugin_frame = PluginFrame {
                frame,
                width_px: usize::from(terminal.grid().width()) * CELL_WIDTH as usize,
                grid: terminal.grid(),
                now: render_started,
                overlays: Vec::new(),
            };
            let active = self.plugins.draw(&mut plugin_frame);
            (active, plugin_frame.overlays)
        } else {
            (false, Vec::new())
        };
        let cursor = if viewing_history {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            cursor_uniform(terminal.grid())
        };
        if let Err(error) = renderer.render(
            self.render_cache.frame.as_slice(),
            &texture_update,
            cursor,
            &overlays,
        ) {
            eprintln!("c-term: GPU render failed: {error}");
        }
        self.last_render = Some(Instant::now());
        if plugin_active {
            self.schedule_animation(render_started);
        } else {
            self.animation_deadline = None;
        }
    }

    fn timers_if_due(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();

        if self
            .app_sync_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            self.app_sync_deadline = None;
            if let Some(terminal) = &mut self.terminal {
                terminal.disable_synchronized_update();
            }
            self.render_cache.invalidate();
            self.request_frame(now);
        }

        let render_due = self
            .render_lower_deadline
            .is_some_and(|deadline| deadline <= now)
            || self
                .render_upper_deadline
                .is_some_and(|deadline| deadline <= now);
        if render_due {
            self.disarm_delayed_render();
            self.request_frame(now);
        }

        if self
            .animation_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            self.animation_deadline = None;
            self.request_frame(now);
        }

        if self.frame_deadline.is_some_and(|deadline| deadline <= now) {
            self.request_redraw_now();
        }

        if let Some(deadline) = self.next_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.animation_deadline,
            self.render_lower_deadline,
            self.render_upper_deadline,
            self.app_sync_deadline,
            self.frame_deadline,
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

impl ApplicationHandler<UserEvent> for WindowBackend {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.initialize(event_loop) {
            eprintln!("c-term: failed to initialize window backend: {error}");
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytes(bytes) => self.handle_pty_bytes(bytes),
            UserEvent::ChildExited => event_loop.exit(),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.timers_if_due(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.handle_resize(size),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => self.handle_mouse_move(position),
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => self.handle_key(event_loop, event),
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

fn spawn_pty_reader(child: &mut PtyChild, proxy: EventLoopProxy<UserEvent>) -> io::Result<()> {
    let mut reader = child.master.try_clone()?;
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let n = match reader.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(0) | Err(_) => {
                    let _ = proxy.send_event(UserEvent::ChildExited);
                    break;
                }
                Ok(n) => n,
            };
            if proxy
                .send_event(UserEvent::PtyBytes(buffer[..n].to_vec()))
                .is_err()
            {
                break;
            }
        }
    });
    Ok(())
}

fn grid_size(size: PhysicalSize<u32>) -> (u16, u16) {
    let cols = (size.width.max(CELL_WIDTH) / CELL_WIDTH).clamp(1, u32::from(u16::MAX)) as u16;
    let rows = (size.height.max(CELL_HEIGHT) / CELL_HEIGHT).clamp(1, u32::from(u16::MAX)) as u16;
    (cols, rows)
}

fn buffer_width(cols: u16) -> u32 {
    u32::from(cols) * CELL_WIDTH
}

fn buffer_height(rows: u16) -> u32 {
    u32::from(rows) * CELL_HEIGHT
}

fn frame_len(cols: u16, rows: u16) -> usize {
    buffer_width(cols) as usize * buffer_height(rows) as usize * 4
}

fn scrollback_row_at(terminal: &TerminalCore, row: usize) -> Option<&[Cell]> {
    let grid = terminal.grid();
    let history_len = terminal.scrollback_len();
    if row < history_len {
        terminal.scrollback_row(row)
    } else {
        grid.row((row - history_len) as u16)
    }
}

fn encode_window_key(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if modifiers.alt_key() {
        bytes.push(0x1b);
    }

    match event.logical_key.as_ref() {
        Key::Character(ch) if modifiers.control_key() => {
            let mut chars = ch.chars();
            if let Some(ch) = chars.next().filter(|_| chars.next().is_none()) {
                bytes.push(ctrl_byte(ch)?);
            } else {
                return None;
            }
        }
        Key::Character(ch) => bytes.extend_from_slice(ch.as_bytes()),
        Key::Named(NamedKey::Space) if modifiers.control_key() => bytes.push(0x00),
        Key::Named(NamedKey::Space) => bytes.push(b' '),
        Key::Named(NamedKey::Enter) => bytes.push(b'\r'),
        Key::Named(NamedKey::Backspace) => bytes.push(0x7f),
        Key::Named(NamedKey::Tab) => bytes.push(b'\t'),
        Key::Named(NamedKey::Escape) => bytes.push(0x1b),
        Key::Named(NamedKey::ArrowLeft) => bytes.extend_from_slice(b"\x1b[D"),
        Key::Named(NamedKey::ArrowRight) => bytes.extend_from_slice(b"\x1b[C"),
        Key::Named(NamedKey::ArrowUp) => bytes.extend_from_slice(b"\x1b[A"),
        Key::Named(NamedKey::ArrowDown) => bytes.extend_from_slice(b"\x1b[B"),
        Key::Named(NamedKey::Home) => bytes.extend_from_slice(b"\x1b[H"),
        Key::Named(NamedKey::End) => bytes.extend_from_slice(b"\x1b[F"),
        Key::Named(NamedKey::PageUp) => bytes.extend_from_slice(b"\x1b[5~"),
        Key::Named(NamedKey::PageDown) => bytes.extend_from_slice(b"\x1b[6~"),
        Key::Named(NamedKey::Delete) => bytes.extend_from_slice(b"\x1b[3~"),
        Key::Named(NamedKey::Insert) => bytes.extend_from_slice(b"\x1b[2~"),
        _ => return None,
    }

    Some(bytes)
}

fn ctrl_byte(ch: char) -> Option<u8> {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_alphabetic() {
        Some((lower as u8) - b'a' + 1)
    } else {
        match lower {
            ' ' | '@' | '2' => Some(0x00),
            '[' => Some(0x1b),
            '3' => Some(0x1b),
            '\\' => Some(0x1c),
            '4' => Some(0x1c),
            ']' => Some(0x1d),
            '5' => Some(0x1d),
            '^' => Some(0x1e),
            '6' => Some(0x1e),
            '_' | '/' | '-' | '7' => Some(0x1f),
            '?' => Some(0x7f),
            '8' => Some(0x7f),
            '`' => Some(0x00),
            '~' => Some(0x1e),
            '=' => Some(0x1f),
            _ => None,
        }
    }
}

fn mouse_cell(position: PhysicalPosition<f64>, cols: u16, rows: u16) -> Option<(u16, u16)> {
    if position.x < 0.0 || position.y < 0.0 {
        return None;
    }
    let x = (position.x as u32 / CELL_WIDTH) as u16;
    let y = (position.y as u32 / CELL_HEIGHT) as u16;
    (x < cols && y < rows).then_some((x, y))
}

fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

fn active_mouse_button(buttons: u8) -> u8 {
    for button in 0..3 {
        if buttons & (1 << button) != 0 {
            return button;
        }
    }
    3
}

fn wheel_codes(delta: MouseScrollDelta) -> Vec<u8> {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => wheel_axis_codes(x, y),
        MouseScrollDelta::PixelDelta(position) => {
            wheel_axis_codes(position.x as f32, position.y as f32)
        }
    }
}

fn wheel_axis_codes(x: f32, y: f32) -> Vec<u8> {
    let mut codes = Vec::new();
    if y > 0.0 {
        codes.push(64);
    } else if y < 0.0 {
        codes.push(65);
    }
    if x > 0.0 {
        codes.push(67);
    } else if x < 0.0 {
        codes.push(66);
    }
    codes
}

fn wheel_scroll_lines(delta: MouseScrollDelta) -> Vec<isize> {
    let raw = match delta {
        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
        MouseScrollDelta::PixelDelta(position) => position.y / f64::from(CELL_HEIGHT),
    };
    let lines = if raw.abs() < 1.0 {
        raw.signum() as isize
    } else {
        raw.round() as isize
    };
    let lines = if lines == 0 { 1 } else { lines };
    vec![lines.clamp(-12, 12)]
}

fn encode_mouse_event(
    mouse: MouseState,
    code: u8,
    x: u16,
    y: u16,
    modifiers: ModifiersState,
    release: bool,
) -> Vec<u8> {
    if mouse.tracking == MouseTracking::None {
        return Vec::new();
    }

    let code = code + mouse_modifier_bits(modifiers);
    let col = x.saturating_add(1);
    let row = y.saturating_add(1);
    if mouse.sgr {
        let final_byte = if release { 'm' } else { 'M' };
        format!("\x1b[<{code};{col};{row}{final_byte}").into_bytes()
    } else {
        let Ok(code) = u8::try_from(u16::from(code) + 32) else {
            return Vec::new();
        };
        let Ok(col) = u8::try_from(col.saturating_add(32)) else {
            return Vec::new();
        };
        let Ok(row) = u8::try_from(row.saturating_add(32)) else {
            return Vec::new();
        };
        vec![0x1b, b'[', b'M', code, col, row]
    }
}

fn mouse_modifier_bits(modifiers: ModifiersState) -> u8 {
    let mut bits = 0;
    if modifiers.shift_key() {
        bits |= 4;
    }
    if modifiers.alt_key() {
        bits |= 8;
    }
    if modifiers.control_key() {
        bits |= 16;
    }
    bits
}

fn clear_grid_row(frame: &mut [u8], width: usize, y: u16) {
    let row_start = usize::from(y) * CELL_HEIGHT as usize * width * 4;
    let row_len = CELL_HEIGHT as usize * width * 4;
    if let Some(row) = frame.get_mut(row_start..row_start + row_len) {
        row.fill(0);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphKey {
    font: usize,
    ch: char,
}

struct LoadedFont {
    font: FontArc,
    scale: PxScale,
    ascent: f32,
    height: f32,
}

struct GlyphBitmap {
    left: i16,
    top: i16,
    width: u16,
    height: u16,
    alpha: Vec<u8>,
}

struct TextRenderer {
    fonts: Vec<LoadedFont>,
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
}

impl TextRenderer {
    fn new(font: FontConfig) -> Self {
        Self {
            fonts: load_fonts(font),
            glyphs: HashMap::new(),
        }
    }

    fn draw_row_to_frame(
        &mut self,
        row: &[Cell],
        frame: &mut [u8],
        width: usize,
        y: u16,
        cols: u16,
    ) {
        for x in 0..cols {
            let cell = row.get(usize::from(x)).copied().unwrap_or_default();
            let ch = if cell.spacer { ' ' } else { cell.ch };
            self.draw_cell(frame, width, x, y, ch, cell.style);
        }
    }

    fn draw_cell(
        &mut self,
        frame: &mut [u8],
        width: usize,
        cell_x: u16,
        cell_y: u16,
        ch: char,
        style: Style,
    ) {
        let fg = rgb(style.foreground, [220, 224, 232]);
        let bg = rgb(style.background, [16, 18, 24]);
        if draw_box_cell(frame, width, cell_x, cell_y, ch, fg, bg) {
            return;
        }

        fill_cell(frame, width, cell_x, cell_y, bg);
        if ch != ' ' {
            if let Some(key) = self.glyph_key(ch) {
                self.ensure_glyph(key);
                if let Some(glyph) = self.glyphs.get(&key) {
                    draw_glyph_bitmap(frame, width, cell_x, cell_y, glyph, fg, 0);
                    if style.bold {
                        draw_glyph_bitmap(frame, width, cell_x, cell_y, glyph, fg, 1);
                    }
                }
            } else {
                draw_font8x8_cell(frame, width, cell_x, cell_y, ch, fg, bg);
            }
        }
        if style.underline {
            draw_underline(frame, width, cell_x, cell_y, fg);
        }
    }

    fn glyph_key(&self, ch: char) -> Option<GlyphKey> {
        self.font_index(ch)
            .map(|font| GlyphKey { font, ch })
            .or_else(|| self.font_index('?').map(|font| GlyphKey { font, ch: '?' }))
    }

    fn font_index(&self, ch: char) -> Option<usize> {
        self.fonts
            .iter()
            .position(|font| font.font.glyph_id(ch) != GlyphId(0))
    }

    fn ensure_glyph(&mut self, key: GlyphKey) {
        if self.glyphs.contains_key(&key) {
            return;
        }
        let glyph = rasterize_glyph(&self.fonts[key.font], key.ch);
        self.glyphs.insert(key, glyph);
    }
}

fn load_fonts(font: FontConfig) -> Vec<LoadedFont> {
    let FontConfig::GlyphAtlas { path, size } = font else {
        return Vec::new();
    };

    let mut loaded = Vec::new();
    if let Ok(bytes) = fs::read(path)
        && let Ok(font) = FontArc::try_from_vec(bytes)
    {
        let scaled = font.as_scaled(size);
        let ascent = scaled.ascent();
        let height = scaled.height();
        loaded.push(LoadedFont {
            font,
            scale: PxScale::from(size),
            ascent,
            height,
        });
    }
    loaded
}

fn rasterize_glyph(font: &LoadedFont, ch: char) -> GlyphBitmap {
    let scaled = font.font.as_scaled(font.scale);
    let glyph_id = scaled.glyph_id(ch);
    let advance = scaled.h_advance(glyph_id);
    let x = ((CELL_WIDTH as f32 - advance) * 0.5).floor().max(-1.0);
    let baseline = ((CELL_HEIGHT as f32 - font.height) * 0.5 + font.ascent).round();
    let glyph = glyph_id.with_scale_and_position(font.scale, point(x, baseline));
    let Some(outlined) = scaled.outline_glyph(glyph) else {
        return GlyphBitmap {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            alpha: Vec::new(),
        };
    };

    let bounds = outlined.px_bounds();
    let width = bounds.width().max(0.0) as u16;
    let height = bounds.height().max(0.0) as u16;
    let mut alpha = vec![0; usize::from(width) * usize::from(height)];
    outlined.draw(|x, y, coverage| {
        let index =
            usize::try_from(y).unwrap_or(0) * usize::from(width) + usize::try_from(x).unwrap_or(0);
        if let Some(alpha) = alpha.get_mut(index) {
            *alpha = coverage.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8;
        }
    });

    GlyphBitmap {
        left: bounds.min.x.floor() as i16,
        top: bounds.min.y.floor() as i16,
        width,
        height,
        alpha,
    }
}

fn fill_cell(frame: &mut [u8], width: usize, cell_x: u16, cell_y: u16, color: [u8; 3]) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for py in 0..CELL_HEIGHT as usize {
        for px in 0..CELL_WIDTH as usize {
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

fn draw_glyph_bitmap(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    glyph: &GlyphBitmap,
    color: [u8; 3],
    x_shift: i16,
) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for y in 0..glyph.height {
        let cell_py = glyph.top + y as i16;
        if !(0..CELL_HEIGHT as i16).contains(&cell_py) {
            continue;
        }
        for x in 0..glyph.width {
            let cell_px = glyph.left + x as i16 + x_shift;
            if !(0..CELL_WIDTH as i16).contains(&cell_px) {
                continue;
            }
            let alpha = glyph.alpha[usize::from(y) * usize::from(glyph.width) + usize::from(x)];
            if alpha == 0 {
                continue;
            }
            let index = ((origin_y + cell_py as usize) * width + origin_x + cell_px as usize) * 4;
            blend_pixel(&mut frame[index..index + 4], color, alpha);
        }
    }
}

fn blend_pixel(pixel: &mut [u8], color: [u8; 3], alpha: u8) {
    let alpha = u16::from(alpha);
    let inv = 255 - alpha;
    for (channel, color) in pixel[..3].iter_mut().zip(color) {
        *channel = ((u16::from(*channel) * inv + u16::from(color) * alpha) / 255) as u8;
    }
    pixel[3] = 0xff;
}

fn draw_font8x8_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
) {
    let glyph = BASIC_FONTS
        .get(ch)
        .or_else(|| BASIC_FONTS.get('?'))
        .unwrap_or([0; 8]);
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for py in 0..CELL_HEIGHT as usize {
        let glyph_row = glyph[py / 2];
        for px in 0..CELL_WIDTH as usize {
            let color = if ((glyph_row >> px) & 1) != 0 { fg } else { bg };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

fn draw_underline(frame: &mut [u8], width: usize, cell_x: u16, cell_y: u16, color: [u8; 3]) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    let y = origin_y + CELL_HEIGHT as usize - 2;
    for px in 0..CELL_WIDTH as usize {
        let index = (y * width + origin_x + px) * 4;
        frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
    }
}

fn draw_box_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
) -> bool {
    let Some((left, right, up, down)) = box_segments(ch) else {
        return false;
    };
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    let center_x = CELL_WIDTH as usize / 2;
    let center_y = CELL_HEIGHT as usize / 2;
    let thickness = 2;

    for py in 0..CELL_HEIGHT as usize {
        for px in 0..CELL_WIDTH as usize {
            let horizontal = py.abs_diff(center_y) < thickness
                && ((left && px <= center_x) || (right && px >= center_x));
            let vertical = px.abs_diff(center_x) < thickness
                && ((up && py <= center_y) || (down && py >= center_y));
            let color = if horizontal || vertical { fg } else { bg };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
    true
}

fn box_segments(ch: char) -> Option<(bool, bool, bool, bool)> {
    match ch {
        '─' | '━' | '╌' | '╍' | '⎺' | '⎻' | '⎼' | '⎽' => {
            Some((true, true, false, false))
        }
        '╴' => Some((true, false, false, false)),
        '╶' => Some((false, true, false, false)),
        '│' | '┃' | '╎' | '╏' | '┆' | '┇' | '┊' | '┋' => {
            Some((false, false, true, true))
        }
        '╵' => Some((false, false, true, false)),
        '╷' => Some((false, false, false, true)),
        '┌' | '┏' | '╭' => Some((false, true, false, true)),
        '┐' | '┓' | '╮' => Some((true, false, false, true)),
        '└' | '┗' | '╰' => Some((false, true, true, false)),
        '┘' | '┛' | '╯' => Some((true, false, true, false)),
        '├' | '┣' => Some((false, true, true, true)),
        '┤' | '┫' => Some((true, false, true, true)),
        '┬' | '┳' => Some((true, true, false, true)),
        '┴' | '┻' => Some((true, true, true, false)),
        '┼' | '╋' => Some((true, true, true, true)),
        _ => None,
    }
}

fn cursor_uniform(grid: &Grid) -> [f32; 4] {
    let cursor = grid.cursor();
    if !cursor.visible {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let (x_start, y_start, cursor_width, cursor_height) = match cursor.shape {
        CursorShape::Block => (0, 0, CELL_WIDTH as usize, CELL_HEIGHT as usize),
        CursorShape::Beam => (0, 0, (CELL_WIDTH as usize / 4).max(1), CELL_HEIGHT as usize),
        CursorShape::Underline => (
            0,
            CELL_HEIGHT as usize - (CELL_HEIGHT as usize / 5).max(1),
            CELL_WIDTH as usize,
            (CELL_HEIGHT as usize / 5).max(1),
        ),
    };
    [
        (usize::from(cursor.x) * CELL_WIDTH as usize + x_start) as f32,
        (usize::from(cursor.y) * CELL_HEIGHT as usize + y_start) as f32,
        cursor_width as f32,
        cursor_height as f32,
    ]
}

pub(crate) fn rgb(color: Color, fallback: [u8; 3]) -> [u8; 3] {
    match color {
        Color::DefaultForeground | Color::DefaultBackground => fallback,
        Color::Indexed(index) => ANSI_COLORS
            .get(usize::from(index))
            .copied()
            .unwrap_or(fallback),
        Color::Rgb(r, g, b) => [r, g, b],
    }
}

const ANSI_COLORS: [[u8; 3]; 16] = [
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
];

#[cfg(test)]
mod tests {
    use super::*;
    use c_term_core::TerminalCore;

    #[test]
    fn box_drawing_characters_are_drawn_without_font_fallback() {
        let width = CELL_WIDTH as usize;
        let mut frame = vec![0; width * CELL_HEIGHT as usize * 4];

        assert!(draw_box_cell(
            &mut frame,
            width,
            0,
            0,
            '─',
            [220, 224, 232],
            [16, 18, 24],
        ));

        let center_y = CELL_HEIGHT as usize / 2;
        let center_index = (center_y * width + width / 2) * 4;
        assert_eq!(&frame[center_index..center_index + 3], &[220, 224, 232]);
        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [16, 18, 24])
        );
    }

    #[test]
    fn non_box_characters_use_font_path() {
        assert_eq!(box_segments('A'), None);
    }

    #[test]
    fn base_frame_cache_updates_only_dirty_rows() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"A\x1b[2;1HB");
        let mut cache = RenderCache::new(FontConfig::Bitmap8x8);

        let frame = cache.update(terminal.grid());
        let first_row = frame[..CELL_HEIGHT as usize * 4 * CELL_WIDTH as usize * 4].to_vec();

        let tick = terminal.process_pty_input(b"\x1b[2;2HC");
        cache.apply_damage(&tick.damage, terminal.grid().height());
        let frame = cache.update(terminal.grid());

        assert_eq!(
            &frame[..CELL_HEIGHT as usize * 4 * CELL_WIDTH as usize * 4],
            first_row.as_slice()
        );
    }

    #[test]
    fn scrollback_cache_handles_resized_history_rows() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");
        let _ = terminal.resize(5, 2);
        let mut cache = RenderCache::new(FontConfig::Bitmap8x8);

        let frame = cache.update_scrollback(&terminal, 1);

        assert_eq!(frame.len(), frame_len(5, 2));
    }

    #[test]
    fn scrollback_cache_keeps_incremental_scrolls_clean() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"aa\r\nbb\r\ncc\r\ndd");
        let mut cache = RenderCache::new(FontConfig::Bitmap8x8);

        let _ = cache.update_scrollback(&terminal, 1);
        let _ = cache.update_scrollback(&terminal, 2);

        assert_eq!(cache.scroll_start, Some(0));
        assert!(!cache.dirty);
    }

    #[test]
    fn sgr_mouse_encoding_uses_one_based_coordinates() {
        let mouse = MouseState {
            tracking: MouseTracking::Click,
            sgr: true,
        };

        assert_eq!(
            encode_mouse_event(mouse, 0, 2, 3, ModifiersState::empty(), false),
            b"\x1b[<0;3;4M"
        );
        assert_eq!(
            encode_mouse_event(mouse, 0, 2, 3, ModifiersState::empty(), true),
            b"\x1b[<0;3;4m"
        );
    }

    #[test]
    fn mouse_encoding_includes_modifiers() {
        let mouse = MouseState {
            tracking: MouseTracking::Click,
            sgr: true,
        };

        assert_eq!(
            encode_mouse_event(
                mouse,
                64,
                0,
                0,
                ModifiersState::SHIFT | ModifiersState::CONTROL,
                false,
            ),
            b"\x1b[<84;1;1M"
        );
    }
}
