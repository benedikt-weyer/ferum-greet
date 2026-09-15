//! A tiny pipeline for two kinds of 2D draws: an opaque textured
//! full-background image, and solid-color rounded-rect panels (the login
//! card, input field highlights). Both share one shader; a per-draw uniform
//! selects the mode.
//!
//! Usage is two-phase to keep the borrow checker happy without leaking
//! memory: call `prepare_*` for every quad in the frame *before* opening the
//! render pass, collecting the results into a `Vec<PreparedQuad>` that lives
//! at least as long as the pass, then call `render` once per prepared quad
//! from inside the pass.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) local: vec2<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
};

struct Uniform {
    color: vec4<f32>,
    half_size: vec2<f32>,
    radius: f32,
    mode: f32,
};

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(1) @binding(0) var<uniform> u: Uniform;

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.local = in.local;
    return out;
}

fn rounded_rect_alpha(local: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(local) - half_size + vec2<f32>(radius, radius);
    let dist = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
    return 1.0 - smoothstep(-1.0, 1.0, dist);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if (u.mode < 0.5) {
        // Mode 0: opaque textured background.
        return textureSample(tex, samp, in.uv);
    }
    // Mode 1: solid-color rounded rectangle.
    let alpha = rounded_rect_alpha(in.local, u.half_size, u.radius);
    return vec4<f32>(u.color.rgb, u.color.a * alpha);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    local: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct QuadUniform {
    color: [f32; 4],
    half_size: [f32; 2],
    radius: f32,
    mode: f32,
}

pub struct QuadRenderer {
    pipeline: wgpu::RenderPipeline,
    tex_bind_layout: wgpu::BindGroupLayout,
    uniform_bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white_tex_bind_group: wgpu::BindGroup,
}

/// A texture ready to be drawn as a full-screen background.
pub struct BackgroundTexture {
    bind_group: wgpu::BindGroup,
}

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

pub struct SolidQuad {
    pub rect: Rect,
    pub color: [f32; 4],
    pub radius: f32,
}

/// A single draw's worth of GPU resources, built before the render pass
/// opens and consumed by `QuadRenderer::render` from inside it.
pub struct PreparedQuad {
    vertex_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
}

impl QuadRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let tex_bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad-tex-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_bind_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("quad-uniform-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad-pipeline-layout"),
            bind_group_layouts: &[Some(&tex_bind_layout), Some(&uniform_bind_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("quad-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let white_tex_bind_group =
            make_texture_bind_group(device, queue, &tex_bind_layout, &sampler, 1, 1, &[255, 255, 255, 255]);

        QuadRenderer {
            pipeline,
            tex_bind_layout,
            uniform_bind_layout,
            sampler,
            white_tex_bind_group,
        }
    }

    pub fn upload_background(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> BackgroundTexture {
        let bind_group = make_texture_bind_group(
            device,
            queue,
            &self.tex_bind_layout,
            &self.sampler,
            width,
            height,
            rgba,
        );
        BackgroundTexture { bind_group }
    }

    pub fn prepare_background(
        &self,
        device: &wgpu::Device,
        screen_w: f32,
        screen_h: f32,
    ) -> PreparedQuad {
        let rect = Rect { x: 0.0, y: 0.0, w: screen_w, h: screen_h };
        let uniform = QuadUniform {
            color: [1.0, 1.0, 1.0, 1.0],
            half_size: [screen_w / 2.0, screen_h / 2.0],
            radius: 0.0,
            mode: 0.0,
        };
        self.prepare(device, &rect, uniform, screen_w, screen_h)
    }

    pub fn prepare_solid(
        &self,
        device: &wgpu::Device,
        quad: &SolidQuad,
        screen_w: f32,
        screen_h: f32,
    ) -> PreparedQuad {
        let uniform = QuadUniform {
            color: quad.color,
            half_size: [quad.rect.w / 2.0, quad.rect.h / 2.0],
            radius: quad.radius,
            mode: 1.0,
        };
        self.prepare(device, &quad.rect, uniform, screen_w, screen_h)
    }

    fn prepare(
        &self,
        device: &wgpu::Device,
        rect: &Rect,
        uniform: QuadUniform,
        screen_w: f32,
        screen_h: f32,
    ) -> PreparedQuad {
        let to_ndc_x = |px: f32| (px / screen_w) * 2.0 - 1.0;
        let to_ndc_y = |px: f32| 1.0 - (px / screen_h) * 2.0;

        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
        let (hw, hh) = (rect.w / 2.0, rect.h / 2.0);
        let verts = [
            Vertex { position: [to_ndc_x(x0), to_ndc_y(y0)], uv: [0.0, 0.0], local: [-hw, -hh] },
            Vertex { position: [to_ndc_x(x1), to_ndc_y(y0)], uv: [1.0, 0.0], local: [hw, -hh] },
            Vertex { position: [to_ndc_x(x1), to_ndc_y(y1)], uv: [1.0, 1.0], local: [hw, hh] },
            Vertex { position: [to_ndc_x(x0), to_ndc_y(y0)], uv: [0.0, 0.0], local: [-hw, -hh] },
            Vertex { position: [to_ndc_x(x1), to_ndc_y(y1)], uv: [1.0, 1.0], local: [hw, hh] },
            Vertex { position: [to_ndc_x(x0), to_ndc_y(y1)], uv: [0.0, 1.0], local: [-hw, hh] },
        ];

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad-vertices"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad-uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad-uniform-bind-group"),
            layout: &self.uniform_bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        PreparedQuad { vertex_buffer, uniform_bind_group }
    }

    /// Draws a quad prepared with `prepare_solid`.
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, quad: &'pass PreparedQuad) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.white_tex_bind_group, &[]);
        pass.set_bind_group(1, &quad.uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, quad.vertex_buffer.slice(..));
        pass.draw(0..6, 0..1);
    }

    /// Draws a quad prepared with `prepare_background`, sampling `bg`.
    pub fn render_background<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        quad: &'pass PreparedQuad,
        bg: &'pass BackgroundTexture,
    ) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bg.bind_group, &[]);
        pass.set_bind_group(1, &quad.uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, quad.vertex_buffer.slice(..));
        pass.draw(0..6, 0..1);
    }
}

fn make_texture_bind_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::BindGroup {
    let size = wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("quad-texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * size.width),
            rows_per_image: Some(size.height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("quad-texture-bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
