use std::{borrow::Cow, error::Error, sync::Arc};

use winit::{dpi::PhysicalSize, window::Window};

use crate::{
    plugins::{OverlayCommand, OverlayKind},
    runner::TerminalMetrics,
};

use super::render_cache::{RowBand, ScrollDamage, TextureUpdate};

const MAX_OVERLAYS: usize = 16;
const OVERLAY_BYTES: usize = 64;

fn select_alpha_mode(modes: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    if modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else if modes.contains(&wgpu::CompositeAlphaMode::PostMultiplied) {
        wgpu::CompositeAlphaMode::PostMultiplied
    } else {
        wgpu::CompositeAlphaMode::Auto
    }
}

pub(super) struct GpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    scratch_texture: wgpu::Texture,
    texture_width: u32,
    texture_height: u32,
    metrics: TerminalMetrics,
    cursor_buffer: wgpu::Buffer,
    overlay_buffer: wgpu::Buffer,
    overlay_bytes: [u8; MAX_OVERLAYS * OVERLAY_BYTES],
    premultiply_alpha: bool,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl GpuRenderer {
    pub(super) fn new(
        window: Arc<Window>,
        surface_size: PhysicalSize<u32>,
        texture_width: u32,
        texture_height: u32,
        metrics: TerminalMetrics,
    ) -> Result<Self, Box<dyn Error>> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
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
        let caps = surface.get_capabilities(&adapter);
        let mut config = surface
            .get_default_config(&adapter, surface_width, surface_height)
            .ok_or("surface is not supported by the selected GPU adapter")?;
        config.alpha_mode = select_alpha_mode(&caps.alpha_modes);
        let premultiply_alpha = config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied;
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
            metrics,
            cursor_buffer,
            overlay_buffer,
            overlay_bytes: [0; MAX_OVERLAYS * OVERLAY_BYTES],
            premultiply_alpha,
            bind_group,
            pipeline,
        })
    }

    pub(super) fn resize_surface(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub(super) fn resize_texture(&mut self, width: u32, height: u32, metrics: TerminalMetrics) {
        self.metrics = metrics;
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

    pub(super) fn render(
        &mut self,
        frame: &[u8],
        update: &TextureUpdate,
        cursor: [f32; 4],
        overlays: &[OverlayCommand],
        screen_opacity: f32,
    ) -> Result<(), &'static str> {
        let expected_len = self.texture_width as usize * self.texture_height as usize * 4;
        if frame.len() != expected_len {
            return Err("frame size does not match GPU texture size");
        }

        self.write_globals(
            cursor,
            overlays.len().min(MAX_OVERLAYS),
            screen_opacity,
            self.premultiply_alpha,
        );
        self.write_overlays(overlays);
        if update.full {
            self.write_frame(frame);
        } else {
            self.copy_scrolled_rows(&update.scrolls);
            self.write_row_bands(frame, &update.rows);
        }

        self.present()
    }

    fn write_globals(
        &self,
        cursor: [f32; 4],
        overlay_count: usize,
        screen_opacity: f32,
        premultiply_alpha: bool,
    ) {
        let mut bytes = [0_u8; 32];
        for (chunk, value) in bytes[..16].chunks_exact_mut(4).zip(cursor) {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        bytes[16..20].copy_from_slice(&(overlay_count as f32).to_ne_bytes());
        bytes[20..24].copy_from_slice(&screen_opacity.clamp(0.0, 1.0).to_ne_bytes());
        bytes[24..28].copy_from_slice(&(premultiply_alpha as u8 as f32).to_ne_bytes());
        self.queue.write_buffer(&self.cursor_buffer, 0, &bytes);
    }

    fn write_overlays(&mut self, overlays: &[OverlayCommand]) {
        self.overlay_bytes.fill(0);
        for (index, overlay) in overlays.iter().take(MAX_OVERLAYS).enumerate() {
            write_overlay(
                &mut self.overlay_bytes[index * OVERLAY_BYTES..(index + 1) * OVERLAY_BYTES],
                overlay,
            );
        }
        self.queue
            .write_buffer(&self.overlay_buffer, 0, &self.overlay_bytes);
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
            let start_px = u32::from(band.start) * self.metrics.cell_height;
            let height_px =
                u32::from(band.end.saturating_sub(band.start)) * self.metrics.cell_height;
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
            let top_px = u32::from(scroll.top) * self.metrics.cell_height;
            let count_px = u32::from(count) * self.metrics.cell_height;
            let copy_height = u32::from(copy_rows) * self.metrics.cell_height;
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
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
    let screen_alpha = clamp(globals.overlay.y, 0.0, 1.0);
    color = vec4<f32>(color.rgb, color.a * screen_alpha);
    if (globals.overlay.z > 0.5) {
        return vec4<f32>(color.rgb * color.a, color.a);
    }
    return color;
}
"#;
