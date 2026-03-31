use anyhow::{Context, Result};
use image::codecs::png::{CompressionType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use std::path::Path;

pub fn convert_image_to_png(image_path: &Path) -> Result<Vec<u8>> {
    let img = image::open(image_path).context("Failed to open input image")?;
    let rgba = img.into_rgba8();
    let mut buf = Vec::new();
    let encoder = PngEncoder::new_with_quality(
        &mut buf,
        CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    );
    encoder
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .context("Failed to encode PNG")?;
    Ok(buf)
}
