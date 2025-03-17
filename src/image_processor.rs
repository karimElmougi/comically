use anyhow::{Context, Result};
use image::imageops::colorops::contrast_in_place;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Pixel, RgbImage};
use log::{info, warn};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::path::Path;
use walkdir::WalkDir;

use crate::Comic;

// Default Kindle dimensions (Paperwhite Signature Edition)
const TARGET_WIDTH: u32 = 1236;
const TARGET_HEIGHT: u32 = 1648;

/// Process all images in the source directory
pub fn process_images(comic: &Comic) -> Result<()> {
    info!("Processing images in {}", comic.directory.display());

    // Create a processed directory
    let images_dir = comic.images_dir();
    let processed_dir = comic.processed_dir();
    create_dir_all(&processed_dir).context("Failed to create processed directory")?;

    // Process each image file
    let image_files: Vec<_> = WalkDir::new(&images_dir)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|entry| entry.path().to_path_buf())
        .collect();

    let processed_count = image_files
        .into_iter()
        .enumerate()
        .par_bridge()
        .map(|(idx, path)| {
            let filename = format!("page{:03}.jpg", idx + 1);
            let output_path = processed_dir.join(filename);
            match process_image(&path, &output_path) {
                Ok(_) => {
                    if idx == 6 {
                        // copy input file to working directory
                        let copy_path = format!("TEST_INPUT_{}.jpg", idx + 1);
                        std::fs::copy(&path, &copy_path).expect("Failed to copy image");

                        // copy output file to working directory
                        let copy_path = format!("TEST_OUTPUT_{}.jpg", idx + 1);
                        std::fs::copy(&output_path, &copy_path).expect("Failed to copy image");
                    }
                    1
                }
                Err(e) => {
                    warn!("Failed to process {}: {}", path.display(), e);
                    0
                }
            }
        })
        .reduce(|| 0, |acc, res| acc + res);

    info!("Processed {} images", processed_count);

    if processed_count == 0 {
        anyhow::bail!("No images were processed");
    }

    Ok(())
}

/// Process a single image file with Kindle-optimized transformations
fn process_image(input_path: &Path, output_path: &Path) -> Result<()> {
    // Load the image
    let img = image::open(input_path)
        .context(format!("Failed to open image: {}", input_path.display()))?;

    let img = crop_margins(img, 1.0, 0.5);

    let mut img = img.to_rgb8();

    // Apply auto contrast (simple version)
    auto_contrast(&mut img);

    let img = resize_image(DynamicImage::ImageRgb8(img))?;

    let img = quantize(img);

    let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(output_path)?);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output_buffer, 75);
    encoder.encode_image(&img).context(format!(
        "Failed to save processed image: {}",
        output_path.display()
    ))?;

    Ok(())
}

fn auto_contrast(img: &mut RgbImage) {
    let gamma = 1.5;

    for pixel in img.pixels_mut() {
        for c in pixel.channels_mut() {
            let normalized = *c as f32 / 255.0;
            let corrected = normalized.powf(gamma);
            *c = (corrected * 255.0).round() as u8;
        }
    }

    contrast_in_place(img, 0.4);
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

// Define the Kindle palette as a constant (without alpha channel)
#[rustfmt::skip]
const KINDLE_PALETTE: [u8; 16] = [
    0x00, // Black
    0x11, 
    0x22, 
    0x33, 
    0x44, 
    0x55, 
    0x66, 
    0x77, 
    0x88, 
    0x99, 
    0xaa, 
    0xbb, 
    0xcc, 
    0xdd, 
    0xee, 
    0xff, // White
];

/// Quantize image using the Kindle palette
fn quantize(img: DynamicImage) -> DynamicImage {
    let img = img.grayscale();
    let img = img.as_luma8().unwrap();
    let (width, height) = img.dimensions();

    let mut result = image::GrayImage::new(width, height);

    // Apply quantization to each pixel
    for y in 0..height {
        for x in 0..width {
            let pixel = img.get_pixel(x, y);
            let gray_value = pixel[0];

            let closest_color = find_closest_color(gray_value, &KINDLE_PALETTE);

            // Set the pixel in the result image
            result.put_pixel(x, y, image::Luma([closest_color]));
        }
    }

    DynamicImage::from(result)
}

fn find_closest_color(value: u8, palette: &[u8]) -> u8 {
    let mut closest = palette[0];
    let mut min_diff = 255;

    for &color in palette {
        let diff = if value > color {
            value - color
        } else {
            color - value
        };

        if diff < min_diff {
            min_diff = diff;
            closest = color;
        }
    }

    closest
}

/// Crop margins from an image based on background color detection
fn crop_margins(img: DynamicImage, power: f32, minimum_area_ratio: f32) -> DynamicImage {
    // Convert to grayscale for processing
    let mut grayscale = img.grayscale();
    contrast_in_place(&mut grayscale, 50.0);

    // Get dimensions
    let (width, height) = grayscale.dimensions();
    let original_area = width * height;

    // Calculate threshold based on power (0-3 range)
    let threshold = threshold_from_power(power);

    // Find bounding box
    if let Some(bbox) = get_bbox(&grayscale, threshold) {
        let (x1, y1, x2, y2) = bbox;
        let crop_area = (x2 - x1) * (y2 - y1);

        // Only crop if the resulting area is at least the minimum percentage of original
        if (crop_area as f32 / original_area as f32) >= minimum_area_ratio {
            log::info!("Cropping image by {}%", crop_area / original_area);
            return img.crop_imm(x1, y1, x2 - x1, y2 - y1);
        }
    }

    // Return original if no suitable crop found
    img
}

/// Calculate threshold value from power parameter
fn threshold_from_power(power: f32) -> u8 {
    // Convert power (0-3) to threshold (0-255)
    // Higher power means more aggressive cropping (lower threshold)
    let normalized_power = power.max(0.0).min(3.0) / 3.0;
    (255.0 * (1.0 - normalized_power.powf(1.5))).round() as u8
}

/// Find the bounding box of non-background content
fn get_bbox(img: &DynamicImage, threshold: u8) -> Option<(u32, u32, u32, u32)> {
    let gray_img = img.as_luma8().expect("luma 8");
    let (width, height) = gray_img.dimensions();

    // Initialize bounds to image edges
    let mut left = width;
    let mut top = height;
    let mut right = 0;
    let mut bottom = 0;

    // Scan the image to find content bounds
    for y in 0..height {
        for x in 0..width {
            let pixel = gray_img.get_pixel(x, y);
            // If pixel is darker than threshold (content)
            if pixel[0] <= threshold {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }

    // If we found any content
    if left < right && top < bottom {
        Some((left, top, right, bottom))
    } else {
        None
    }
}
