use anyhow::{Context, Result};
use image::imageops::colorops::contrast_in_place;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, GrayImage};
use log::{info, warn};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::path::Path;

use crate::{Comic, ProcessedImage};

/// Process all images in the source directory
pub fn process_images(comic: &mut Comic) -> Result<()> {
    info!("Processing images in {}", comic.directory.display());

    // Create a processed directory
    let images_dir = comic.images_dir();
    let processed_dir = comic.processed_dir();
    create_dir_all(&processed_dir).context("Failed to create processed directory")?;

    let mut processed = comic
        .input_page_names
        .iter()
        .enumerate()
        .par_bridge()
        .filter_map(|(idx, file_name)| {
            let input_path = images_dir.join(file_name);
            let output_path = processed_dir.join(format!("page{:03}.jpg", idx + 1));
            match process_image(
                &input_path,
                &output_path,
                comic.device_dimensions,
                comic.compression_quality,
            ) {
                Ok(img) => Some((output_path, img.dimensions())),
                Err(e) => {
                    warn!("Failed to process {}: {}", input_path.display(), e);
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    info!("Processed {} images", processed.len());

    if processed.is_empty() {
        anyhow::bail!("No images were processed");
    }

    processed.sort_by_key(|(a, _)| a.to_string_lossy().into_owned());

    comic.processed_files = processed
        .into_iter()
        .map(|(path, dimensions)| ProcessedImage { path, dimensions })
        .collect::<Vec<_>>();

    Ok(())
}

/// Process a single image file with Kindle-optimized transformations
fn process_image(
    input_path: &Path,
    output_path: &Path,
    device_dimensions: (u32, u32),
    compression_quality: u8,
) -> Result<DynamicImage> {
    let img = image::open(input_path)
        .context(format!("Failed to open image: {}", input_path.display()))?;

    let mut img = img.into_luma8();

    auto_contrast(&mut img);

    let img = resize_image(DynamicImage::from(img), device_dimensions)?;

    let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(output_path)?);
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output_buffer, compression_quality);
    encoder.encode_image(&img).context(format!(
        "Failed to save processed image: {}",
        output_path.display()
    ))?;

    Ok(img)
}

fn auto_contrast(img: &mut GrayImage) {
    contrast_in_place(img, 20.0);
}

fn resize_image(img: DynamicImage, device_dimensions: (u32, u32)) -> Result<DynamicImage> {
    let (target_width, target_height) = device_dimensions;
    let (width, height) = img.dimensions();

    // Choose resize method based on whether we're upscaling or downscaling
    let filter = if width <= target_width && height <= target_height {
        // For upscaling, Bicubic gives smoother results for manga
        FilterType::CatmullRom
    } else {
        // For downscaling, Lanczos3 preserves more detail
        FilterType::Lanczos3
    };

    // Calculate aspect ratios
    let ratio_device = target_height as f32 / target_width as f32;
    let ratio_image = height as f32 / width as f32;

    // Determine resize strategy based on aspect ratios
    let processed = if (ratio_image - ratio_device).abs() < 0.015 {
        // Similar aspect ratios - use fit to fill the screen
        img.resize_exact(target_width, target_height, filter)
    } else {
        // Different aspect ratios - maintain aspect ratio
        let width_ratio = target_width as f32 / width as f32;
        let height_ratio = target_height as f32 / height as f32;
        let ratio = width_ratio.min(height_ratio);

        let new_width = (width as f32 * ratio) as u32;
        let new_height = (height as f32 * ratio) as u32;

        img.resize(new_width, new_height, filter)
    };

    Ok(processed)
}
