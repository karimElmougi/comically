use anyhow::{Context, Result};
use image::imageops::colorops::contrast_in_place;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, GrayImage};
use log::{info, warn};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::path::Path;

use crate::Comic;

// Default Kindle dimensions (Paperwhite Signature Edition)
const TARGET_WIDTH: u32 = 1236;
const TARGET_HEIGHT: u32 = 1648;

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
            match process_image(&input_path, &output_path) {
                Ok(_) => Some(output_path),
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

    processed.sort();

    comic.processed_files = processed;

    Ok(())
}

/// Process a single image file with Kindle-optimized transformations
fn process_image(input_path: &Path, output_path: &Path) -> Result<()> {
    let img = image::open(input_path)
        .context(format!("Failed to open image: {}", input_path.display()))?;

    let mut img = img.into_luma8();

    auto_contrast(&mut img);

    let img = resize_image(DynamicImage::from(img))?;

    let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(output_path)?);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output_buffer, 75);
    encoder.encode_image(&img).context(format!(
        "Failed to save processed image: {}",
        output_path.display()
    ))?;

    Ok(())
}

fn auto_contrast(img: &mut GrayImage) {
    contrast_in_place(img, 20.0);
}

fn resize_image(img: DynamicImage) -> Result<DynamicImage> {
    let (width, height) = img.dimensions();

    // Choose resize method based on whether we're upscaling or downscaling
    let filter = if width <= TARGET_WIDTH && height <= TARGET_HEIGHT {
        // For upscaling, Bicubic gives smoother results for manga
        FilterType::CatmullRom
    } else {
        // For downscaling, Lanczos3 preserves more detail
        FilterType::Lanczos3
    };

    // Calculate aspect ratios
    let ratio_device = TARGET_HEIGHT as f32 / TARGET_WIDTH as f32;
    let ratio_image = height as f32 / width as f32;

    // Determine resize strategy based on aspect ratios
    let processed = if (ratio_image - ratio_device).abs() < 0.015 {
        // Similar aspect ratios - use fit to fill the screen
        img.resize_exact(TARGET_WIDTH, TARGET_HEIGHT, filter)
    } else {
        // Different aspect ratios - maintain aspect ratio
        let width_ratio = TARGET_WIDTH as f32 / width as f32;
        let height_ratio = TARGET_HEIGHT as f32 / height as f32;
        let ratio = width_ratio.min(height_ratio);

        let new_width = (width as f32 * ratio) as u32;
        let new_height = (height as f32 * ratio) as u32;

        img.resize(new_width, new_height, filter)
    };

    Ok(processed)
}
