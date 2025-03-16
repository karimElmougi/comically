use anyhow::{Context, Result};
use color_quant::NeuQuant;
use image::imageops::colorops::contrast_in_place;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Pixel, RgbImage};
use log::{info, warn};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// Default Kindle dimensions (Paperwhite Signature Edition)
const TARGET_WIDTH: u32 = 1236;
const TARGET_HEIGHT: u32 = 1648;

/// Process all images in the source directory
pub fn process_images(src_dir: PathBuf) -> Result<PathBuf> {
    info!("Processing images in {}", src_dir.display());

    // Create a processed directory
    let parent = src_dir.parent().unwrap_or(&src_dir);
    let processed_dir = parent.join("Processed");
    create_dir_all(&processed_dir).context("Failed to create processed directory")?;

    // Process each image file
    let image_files: Vec<_> = WalkDir::new(&src_dir)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|entry| {
            let is_file = entry.file_type().is_file();
            if is_file {
                let path = entry.path();
                let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
                ["jpg", "jpeg", "png", "gif"].contains(&extension.to_lowercase().as_str())
            } else {
                false
            }
        })
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
                Ok(_) => 1,
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

    Ok(processed_dir)
}

/// Process a single image file with Kindle-optimized transformations
fn process_image(input_path: &Path, output_path: &Path) -> Result<()> {
    // Load the image
    let img = image::open(input_path)
        .context(format!("Failed to open image: {}", input_path.display()))?;

    let mut img = img.to_rgb8();

    // Apply auto contrast (simple version)
    auto_contrast(&mut img);

    let processed = resize_image(DynamicImage::ImageRgb8(img))?;

    let quantized = quantize(processed);

    // Save with high quality settings
    let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(output_path)?);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output_buffer, 95);
    encoder.encode_image(&quantized).context(format!(
        "Failed to save processed image: {}",
        output_path.display()
    ))?;

    Ok(())
}

fn auto_contrast(img: &mut RgbImage) {
    let gamma = 1.8;

    for pixel in img.pixels_mut() {
        for c in pixel.channels_mut() {
            let normalized = *c as f32 / 255.0;
            let corrected = normalized.powf(gamma);
            *c = (corrected * 255.0).round() as u8;
        }
    }

    contrast_in_place(img, 0.1);
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

// Define the Kindle palette as a constant
#[rustfmt::skip]
const KINDLE_PALETTE: [u8; 64] = [
    0x00, 0x00, 0x00, 0xff,  // Black with full opacity
    0x11, 0x11, 0x11, 0xff,
    0x22, 0x22, 0x22, 0xff,
    0x33, 0x33, 0x33, 0xff,
    0x44, 0x44, 0x44, 0xff,
    0x55, 0x55, 0x55, 0xff,
    0x66, 0x66, 0x66, 0xff,
    0x77, 0x77, 0x77, 0xff,
    0x88, 0x88, 0x88, 0xff,
    0x99, 0x99, 0x99, 0xff,
    0xaa, 0xaa, 0xaa, 0xff,
    0xbb, 0xbb, 0xbb, 0xff,
    0xcc, 0xcc, 0xcc, 0xff,
    0xdd, 0xdd, 0xdd, 0xff,
    0xee, 0xee, 0xee, 0xff,
    0xff, 0xff, 0xff, 0xff,  // White with full opacity
];

/// Quantize image using the Kindle palette
fn quantize(img: DynamicImage) -> DynamicImage {
    // Force convert to grayscale to ensure proper contrast
    let grayscale = img.grayscale();
    let rgb = grayscale.to_rgba8();
    let (width, height) = rgb.dimensions();

    // Apply NeuQuant quantization
    // TODO: USE LAZYLOCK
    let nq = NeuQuant::new(1, 16, &KINDLE_PALETTE);

    // Create a new image with the quantized colors
    let mut result = image::RgbaImage::new(width, height);

    // Apply quantization to each pixel
    for y in 0..height {
        for x in 0..width {
            let pixel = rgb.get_pixel(x, y);

            let mut color = [pixel[0], pixel[1], pixel[2], pixel[3]];
            nq.map_pixel(&mut color);

            // Set the pixel in the result image
            result.put_pixel(x, y, image::Rgba(color));
        }
    }

    DynamicImage::from(result)
}
