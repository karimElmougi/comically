use anyhow::{Context, Result};
use image::imageops::colorops::contrast_in_place;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, GrayImage};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::path::Path;

use crate::{Comic, ProcessedImage};

/// Process all images in the source directory
pub fn process_images(comic: &mut Comic) -> Result<()> {
    log::debug!("Processing images in {}", comic.directory.display());

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
                    log::warn!("Failed to process {}: {}", input_path.display(), e);
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    log::debug!("Processed {} images", processed.len());

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

    // Apply auto-cropping
    let img = if let Some(cropped) = auto_crop_sides(&img) {
        cropped
    } else {
        DynamicImage::from(img)
    };

    let img = resize_image(img, device_dimensions)?;

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

/// Auto-crop white margins from left and right sides of the image
fn auto_crop_sides(img: &GrayImage) -> Option<DynamicImage> {
    const WHITE_THRESHOLD: u8 = 230; // Pixel values above this are considered "white"
    const MIN_MARGIN_WIDTH: u32 = 10; // Minimum width to consider cropping

    let (width, height) = img.dimensions();

    // Find middle y coordinate
    let mid_y = height / 2;

    // Scan from left edge to find first non-white pixel
    let mut left_margin = 0;
    for x in 0..width {
        if img.get_pixel(x, mid_y)[0] < WHITE_THRESHOLD {
            left_margin = x;
            break;
        }
    }

    // Scan from right edge to find first non-white pixel
    let mut right_margin = width;
    for x in (0..width).rev() {
        if img.get_pixel(x, mid_y)[0] < WHITE_THRESHOLD {
            right_margin = x + 1;
            break;
        }
    }

    // Verify these columns are empty (all white) throughout their height
    let left_margin = verify_vertical_margin(img, left_margin, WHITE_THRESHOLD).unwrap_or(0);
    let right_margin = verify_vertical_margin(img, right_margin.saturating_sub(1), WHITE_THRESHOLD)
        .map(|x| x + 1)
        .unwrap_or(width);

    // Only crop if we found valid margins with sufficient width
    if left_margin > MIN_MARGIN_WIDTH || width - right_margin > MIN_MARGIN_WIDTH {
        let crop_width = right_margin - left_margin;
        if crop_width > 0 && crop_width < width {
            return Some(DynamicImage::from(
                image::imageops::crop_imm(img, left_margin, 0, crop_width, height).to_image(),
            ));
        }
    }

    None
}

/// Verify if a column can be considered a valid margin by checking if it's all white
/// Returns the adjusted margin position
fn verify_vertical_margin(img: &GrayImage, initial_x: u32, white_threshold: u8) -> Option<u32> {
    let (width, height) = img.dimensions();
    if initial_x >= width {
        return None;
    }

    // For left margin: find rightmost column that's all white
    // For right margin: find leftmost column that's all white
    let is_left_side = initial_x < width / 2;

    let mut margin_x = if is_left_side { 0 } else { width - 1 };

    let range = if is_left_side {
        0..initial_x.saturating_add(1)
    } else {
        initial_x..width
    };

    for x in range {
        let mut is_white_column = true;

        // Check the entire column
        for y in 0..height {
            if img.get_pixel(x, y)[0] < white_threshold {
                is_white_column = false;
                break;
            }
        }

        if is_white_column {
            margin_x = x;
            // For left margin, continue to find rightmost white column
            if !is_left_side {
                break;
            }
        } else if is_left_side {
            // Found non-white column on left side, stop
            break;
        }
    }

    Some(margin_x)
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
