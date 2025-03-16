use anyhow::{Context, Result};
use color_quant::NeuQuant;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
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

    // Convert to grayscale
    let img = img.grayscale();
    let mut img = img.to_rgb8();

    // Apply auto contrast (simple version)
    auto_contrast(&mut img);

    // Resize image for device dimensions
    let processed = resize_image_kcc_style(DynamicImage::ImageRgb8(img))?;

    // Apply quantization with color_quant (using the 16-color grayscale palette)
    let quantized = quantize_with_neuquant(processed);

    // Save with high quality settings
    let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(output_path)?);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output_buffer, 95);
    encoder.encode_image(&quantized).context(format!(
        "Failed to save processed image: {}",
        output_path.display()
    ))?;

    Ok(())
}

/// Simple auto-contrast function
fn auto_contrast(img: &mut RgbImage) {
    let mut min = 255;
    let mut max = 0;

    // Find min and max values
    for pixel in img.pixels() {
        let luminance =
            (pixel[0] as u32 * 299 + pixel[1] as u32 * 587 + pixel[2] as u32 * 114) / 1000;
        min = min.min(luminance as u8);
        max = max.max(luminance as u8);
    }

    // Apply contrast stretching if there's a meaningful range
    if max > min {
        for pixel in img.pixels_mut() {
            for c in 0..3 {
                pixel[c] =
                    (((pixel[c] as u32 - min as u32) * 255) / (max as u32 - min as u32)) as u8;
            }
        }
    }
}

/// KCC-style image resizing
fn resize_image_kcc_style(img: DynamicImage) -> Result<DynamicImage> {
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

/// Quantize image using NeuQuant algorithm from color_quant
fn quantize_with_neuquant(img: DynamicImage) -> DynamicImage {
    // Force convert to grayscale to ensure proper contrast
    let grayscale = img.grayscale();
    let rgb = grayscale.to_rgb8();
    let (width, height) = rgb.dimensions();

    // Flatten RGB pixels into a vec of bytes for NeuQuant
    let pixels: Vec<u8> = rgb.pixels().flat_map(|p| p.0.to_vec()).collect();

    // Setup Kindle grayscale palette
    // We specifically want the 16 Kindle grayscale levels (not automatic colors)
    // These values match the Kindle's e-ink display capabilities
    let kindle_palette = [
        0x00, 0x00, 0x00, // Black
        0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x33, 0x33, 0x33, 0x44, 0x44, 0x44, 0x55, 0x55, 0x55,
        0x66, 0x66, 0x66, 0x77, 0x77, 0x77, 0x88, 0x88, 0x88, 0x99, 0x99, 0x99, 0xAA, 0xAA, 0xAA,
        0xBB, 0xBB, 0xBB, 0xCC, 0xCC, 0xCC, 0xDD, 0xDD, 0xDD, 0xEE, 0xEE, 0xEE, 0xFF, 0xFF,
        0xFF, // White
    ];

    // Apply NeuQuant quantization with a low sample factor for higher quality
    // Sample factor: 1 is highest quality, 30 is fastest
    let nq = NeuQuant::new(1, 16, &pixels);

    // Create a new image with the quantized colors
    let mut result = RgbImage::new(width, height);

    // Apply quantization to each pixel
    let mut pixel_index = 0;
    for y in 0..height {
        for x in 0..width {
            let r = pixels[pixel_index];
            let g = pixels[pixel_index + 1];
            let b = pixels[pixel_index + 2];

            // Get the index of the closest color in the palette
            let color_idx = nq.index_of(&[r, g, b]);

            // Get the actual color from the palette
            let [mut r_val, mut g_val, mut b_val, a] = nq.lookup(color_idx).unwrap();

            // Enhance dark colors slightly for better text readability
            // This specifically helps with manga text bubbles

            // Darken dark areas slightly for better text contrast
            if r_val < 64 && g_val < 64 && b_val < 64 {
                r_val = (r_val as f32 * 0.7) as u8;
                g_val = (g_val as f32 * 0.7) as u8;
                b_val = (b_val as f32 * 0.7) as u8;
            }

            // Set the pixel in the result image
            result.put_pixel(x, y, Rgb([r_val, g_val, b_val]));

            pixel_index += 3;
        }
    }

    DynamicImage::ImageRgb8(result)
}
