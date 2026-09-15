//! Loads the configured background into a tightly-packed RGBA8 buffer sized
//! exactly `width x height`, ready to upload as a texture. Raster images are
//! cropped/scaled to cover the screen (like CSS `background-size: cover`);
//! SVGs are rasterized directly at the target size via resvg.

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

use crate::config::BackgroundConfig;

pub fn load(cfg: &BackgroundConfig, width: u32, height: u32) -> Result<RgbaImage> {
    match cfg {
        BackgroundConfig::Color { r, g, b } => Ok(solid(*r, *g, *b, width, height)),
        BackgroundConfig::Default => {
            load_path(crate::config::BUILTIN_DEFAULT_WALLPAPER.as_ref(), width, height).or_else(
                |err| {
                    log::warn!("no built-in wallpaper available ({err}), using solid background");
                    Ok(solid(30, 34, 45, width, height))
                },
            )
        }
        BackgroundConfig::Image { path } => load_path(path, width, height),
    }
}

fn solid(r: u8, g: u8, b: u8, width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_pixel(width.max(1), height.max(1), image::Rgba([r, g, b, 255]))
}

fn load_path(path: &std::path::Path, width: u32, height: u32) -> Result<RgbaImage> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let is_svg = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
    if is_svg {
        render_svg(&bytes, width, height)
    } else {
        let img = image::load_from_memory(&bytes)
            .with_context(|| format!("decoding image {}", path.display()))?;
        Ok(cover(img, width, height))
    }
}

/// Resizes+crops `img` so it exactly covers `width x height`, cropping the
/// overflow evenly from the centered axis.
fn cover(img: DynamicImage, width: u32, height: u32) -> RgbaImage {
    img.resize_to_fill(width.max(1), height.max(1), FilterType::Lanczos3)
        .to_rgba8()
}

fn render_svg(bytes: &[u8], width: u32, height: u32) -> Result<RgbaImage> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .context("parsing background SVG")?;
    let size = tree.size();
    let (sw, sh) = (size.width().max(1.0), size.height().max(1.0));
    // Cover-scale: uniform scale that fills the target box, then center.
    let scale = (width as f32 / sw).max(height as f32 / sh);
    let (scaled_w, scaled_h) = (sw * scale, sh * scale);
    let tx = (width as f32 - scaled_w) / 2.0;
    let ty = (height as f32 - scaled_h) / 2.0;

    let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1))
        .context("allocating SVG raster target")?;
    let transform = tiny_skia::Transform::from_translate(tx, ty).pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    RgbaImage::from_raw(width.max(1), height.max(1), pixmap.data().to_vec())
        .context("building RGBA image from rasterized SVG")
}
