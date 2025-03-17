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
                comic.auto_crop,
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
    auto_crop: bool,
) -> Result<DynamicImage> {
    let img = image::open(input_path)
        .context(format!("Failed to open image: {}", input_path.display()))?;

    let mut img = img.into_luma8();

    auto_contrast(&mut img);

    let img = auto_crop
        .then(|| auto_crop_sides(&img))
        .flatten()
        .unwrap_or(DynamicImage::from(img));

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
    const SAMPLE_ROWS: usize = 5; // Number of rows to sample
    const SAFETY_MARGIN: u32 = 2; // Extra margin to keep, avoiding cutting content

    let (width, height) = img.dimensions();

    // Sample multiple rows instead of just the middle
    let mut sample_ys = Vec::with_capacity(SAMPLE_ROWS);
    for i in 0..SAMPLE_ROWS {
        sample_ys.push((height * (i + 1) as u32) / (SAMPLE_ROWS as u32 + 1));
    }

    // Find the most conservative margins (closest to content)
    let mut left_margin = width;
    let mut right_margin = 0;

    for &y in &sample_ys {
        // Find leftmost non-white pixel for this row
        for x in 0..width {
            if img.get_pixel(x, y)[0] < WHITE_THRESHOLD {
                left_margin = left_margin.min(x);
                break;
            }
        }

        // Find rightmost non-white pixel for this row
        for x in (0..width).rev() {
            if img.get_pixel(x, y)[0] < WHITE_THRESHOLD {
                right_margin = right_margin.max(x);
                break;
            }
        }
    }

    // Guard against not finding any content
    if left_margin >= width || right_margin == 0 {
        return None;
    }

    // Apply safety margin
    left_margin = left_margin.saturating_sub(SAFETY_MARGIN);
    right_margin = (right_margin + SAFETY_MARGIN).min(width - 1);

    let crop_width = right_margin.saturating_sub(left_margin).saturating_add(1);
    let left_margin_size = left_margin;
    let right_margin_size = width.saturating_sub(right_margin).saturating_sub(1);

    // Only crop if margins are wide enough AND we have a positive crop width
    if (left_margin_size >= MIN_MARGIN_WIDTH || right_margin_size >= MIN_MARGIN_WIDTH)
        && crop_width > 0
        && crop_width < width
    {
        return Some(DynamicImage::from(
            image::imageops::crop_imm(img, left_margin, 0, crop_width, height).to_image(),
        ));
    }

    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    #[test]
    fn test_basic_cropping() {
        // Create a 100x50 image with 25px left margin and 35px right margin
        let test_img = create_test_image(100, 50, 25, 35, &[]);

        let left = find_leftmost_content(&test_img);
        let right = find_rightmost_content(&test_img);

        assert_eq!(left, 25, "Left content should start at x=25");
        assert_eq!(right, 64, "Right content should end at x=64");

        let result = auto_crop_sides(&test_img);
        assert!(result.is_some(), "Cropping should have succeeded");

        let cropped = result.unwrap();
        let (cropped_width, cropped_height) = cropped.dimensions();

        assert_eq!(cropped_height, 50);
        assert!(
            cropped_width >= 35 && cropped_width <= 45,
            "Cropped width {} is outside expected range [35-45]",
            cropped_width
        );
    }

    #[test]
    fn test_noise_resilience() {
        // Create an image with proper margins and noise in those margins
        let test_img = create_test_image(
            100,
            50,
            20,
            20,
            &[(5, 25), (96, 10)], // Noise in left and right margins
        );

        let left = find_leftmost_content(&test_img);
        let right = find_rightmost_content(&test_img);

        assert_eq!(left, 5, "Should detect noise at x=5");
        assert_eq!(right, 96, "Should detect noise at x=96");

        let result = auto_crop_sides(&test_img);
        assert!(
            result.is_some(),
            "Cropping should succeed with noise pixels"
        );

        let cropped = result.unwrap();
        let (cropped_width, _) = cropped.dimensions();

        assert!(
            cropped_width >= 75 && cropped_width <= 85,
            "Cropped width {} is outside expected range [75-85]",
            cropped_width
        );
    }

    #[test]
    fn test_no_margins() {
        let test_img = create_test_image(100, 50, 0, 0, &[]);

        let cropped_option = auto_crop_sides(&test_img);

        assert!(
            cropped_option.is_none(),
            "Expected None for image without margins"
        );
    }

    #[test]
    fn test_large_margin() {
        let test_img = create_test_image(100, 50, 30, 5, &[]);

        let left = find_leftmost_content(&test_img);
        let right = find_rightmost_content(&test_img);
        assert_eq!(left, 30);
        assert_eq!(right, 94);

        let result = auto_crop_sides(&test_img);
        assert!(
            result.is_some(),
            "Expected cropping to succeed with one large margin"
        );
    }

    #[test]
    fn test_complex_image() {
        let mut img = GrayImage::new(200, 100);

        for y in 0..100 {
            for x in 0..200 {
                img.put_pixel(x, y, Luma([255]));
            }
        }

        for y in 20..80 {
            for x in 40..160 {
                img.put_pixel(x, y, Luma([0]));
            }
        }

        // Apply cropping
        let result = auto_crop_sides(&img);
        assert!(result.is_some(), "Expected cropping to succeed");

        // The shape has a 40px margin on both sides
        let cropped = result.unwrap();
        let (cropped_width, _) = cropped.dimensions();

        assert!(
            cropped_width >= 115 && cropped_width <= 125,
            "Cropped width {} is outside expected range [115-125]",
            cropped_width
        );
    }

    /// Create a test image with known margins and content
    fn create_test_image(
        width: u32,
        height: u32,
        left_margin: u32,
        right_margin: u32,
        noise_positions: &[(u32, u32)],
    ) -> GrayImage {
        let mut img = GrayImage::new(width, height);

        // Fill with white
        for y in 0..height {
            for x in 0..width {
                img.put_pixel(x, y, Luma([255]));
            }
        }

        // Add content (black area) in the middle
        for y in 0..height {
            let right_boundary = width.saturating_sub(right_margin);
            for x in left_margin..right_boundary {
                img.put_pixel(x, y, Luma([0]));
            }
        }

        // Add noise pixels
        for &(x, y) in noise_positions {
            if x < width && y < height {
                img.put_pixel(x, y, Luma([0]));
            }
        }

        img
    }

    fn find_leftmost_content(img: &GrayImage) -> u32 {
        let (width, height) = img.dimensions();
        for x in 0..width {
            for y in 0..height {
                if img.get_pixel(x, y)[0] < 230 {
                    return x;
                }
            }
        }
        0
    }

    fn find_rightmost_content(img: &GrayImage) -> u32 {
        let (width, height) = img.dimensions();
        for x in (0..width).rev() {
            for y in 0..height {
                if img.get_pixel(x, y)[0] < 230 {
                    return x;
                }
            }
        }
        width - 1
    }
}
