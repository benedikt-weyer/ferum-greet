//! Offscreen wgpu rendering. There is no window system here: the whole
//! frame is rendered into a plain `wgpu::Texture`, read back to a CPU
//! buffer, and handed to `drm_backend::DrmBackend::present` for scanout.

mod quad;
mod text;

use anyhow::{Context, Result};

pub use quad::{BackgroundTexture, Rect, SolidQuad};
pub use text::Label;

use glyphon::Color as TextColor;
use quad::QuadRenderer;
use text::TextEngine;

/// Bgra8Unorm matches the byte order DRM expects for `DrmFourcc::Xrgb8888`
/// (B, G, R, X per pixel, little-endian), so the final readback can be
/// memcpy'd straight into the scanout buffer with no per-pixel swizzling.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            ..Default::default()
        }))
        .context("no compatible GPU adapter found (need Vulkan or GLES/EGL)")?;

        log::info!("using GPU adapter: {:?}", adapter.get_info());

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ferum-greet-device"),
            ..Default::default()
        }))
        .context("failed to create wgpu device")?;

        Ok(Gpu { device, queue })
    }
}

/// A row-padded CPU-visible copy of the rendered frame.
pub struct FrameReadback {
    pub data: Vec<u8>,
    pub bytes_per_row: u32,
}

pub struct Renderer {
    width: u32,
    height: u32,
    target: wgpu::Texture,
    readback_buffer: wgpu::Buffer,
    padded_bytes_per_row: u32,
    quad: QuadRenderer,
    pub text: TextEngine,
}

impl Renderer {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let unpadded_bytes_per_row = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

        let readback_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame-readback"),
            size: (padded_bytes_per_row as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let quad = QuadRenderer::new(&gpu.device, &gpu.queue, TARGET_FORMAT);
        let text = TextEngine::new(&gpu.device, &gpu.queue, TARGET_FORMAT);

        Renderer {
            width,
            height,
            target,
            readback_buffer,
            padded_bytes_per_row,
            quad,
            text,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn upload_background(&self, gpu: &Gpu, width: u32, height: u32, rgba: &[u8]) -> BackgroundTexture {
        self.quad.upload_background(&gpu.device, &gpu.queue, width, height, rgba)
    }

    pub fn make_label(&mut self, text: &str, size_px: f32, left: f32, top: f32, color: [u8; 3]) -> Label {
        self.text.make_label(
            text,
            size_px,
            left,
            top,
            TextColor::rgb(color[0], color[1], color[2]),
        )
    }

    /// Renders one frame: the background image, then a set of solid-color
    /// panels (login card, focus highlight), then a set of text labels on
    /// top. Returns a CPU-readable copy of the result.
    pub fn render_frame(
        &mut self,
        gpu: &Gpu,
        background: &BackgroundTexture,
        panels: &[SolidQuad],
        labels: &[Label],
    ) -> Result<FrameReadback> {
        let (w, h) = (self.width as f32, self.height as f32);

        // Phase 1: build all per-quad GPU resources up front so the render
        // pass below can borrow them without triggering re-allocation while
        // borrowed (see quad.rs's doc comment).
        let bg_quad = self.quad.prepare_background(&gpu.device, w, h);
        let panel_quads: Vec<_> = panels
            .iter()
            .map(|p| self.quad.prepare_solid(&gpu.device, p, w, h))
            .collect();

        let view = self.target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame-encoder") });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame-pass"),
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

            self.quad.render_background(&mut pass, &bg_quad, background);
            for quad in &panel_quads {
                self.quad.render(&mut pass, quad);
            }

            self.text.prepare_and_render(&gpu.device, &gpu.queue, self.width, self.height, labels, &mut pass);
        }

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );

        gpu.queue.submit(Some(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        self.readback_buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        gpu.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .context("polling device for map_async")?;
        rx.recv()
            .context("map_async callback never fired")?
            .context("mapping readback buffer failed")?;

        let data = self
            .readback_buffer
            .slice(..)
            .get_mapped_range()
            .context("getting mapped readback range")?
            .to_vec();
        self.readback_buffer.unmap();

        Ok(FrameReadback {
            data,
            bytes_per_row: self.padded_bytes_per_row,
        })
    }
}
