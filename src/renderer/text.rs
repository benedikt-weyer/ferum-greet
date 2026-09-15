//! Thin wrapper around glyphon/cosmic-text for shaping and GPU-rendering the
//! handful of text labels the greeter needs.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

pub struct Label {
    pub buffer: Buffer,
    pub left: f32,
    pub top: f32,
    pub color: Color,
}

pub struct TextEngine {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
}

impl TextEngine {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        TextEngine {
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
        }
    }

    pub fn make_label(&mut self, text: &str, size_px: f32, left: f32, top: f32, color: Color) -> Label {
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(size_px, size_px * 1.3));
        buffer.set_size(Some(2000.0), Some(size_px * 2.0));
        buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        Label { buffer, left, top, color }
    }

    pub fn prepare_and_render<'pass>(
        &'pass mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_w: u32,
        screen_h: u32,
        labels: &'pass [Label],
        pass: &mut wgpu::RenderPass<'pass>,
    ) {
        self.viewport.update(
            queue,
            Resolution {
                width: screen_w,
                height: screen_h,
            },
        );

        let areas = labels.iter().map(|label| TextArea {
            buffer: &label.buffer,
            left: label.left,
            top: label.top,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: screen_w as i32,
                bottom: screen_h as i32,
            },
            default_color: label.color,
            custom_glyphs: &[],
        });

        if let Err(err) = self.renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        ) {
            log::error!("text prepare failed: {err}");
            return;
        }

        if let Err(err) = self.renderer.render(&self.atlas, &self.viewport, pass) {
            log::error!("text render failed: {err}");
        }

        self.atlas.trim();
    }
}
