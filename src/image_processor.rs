use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, GrayImage, ImageBuffer, Pixel, Rgb, RgbImage};
use log::{info, warn};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// Default Kindle dimensions (Paperwhite Signature Edition)
const TARGET_WIDTH: u32 = 1236;
const TARGET_HEIGHT: u32 = 1648;

// Kindle Paperwhite grayscale palette (16 levels)
const KINDLE_PALETTE_16: [u8; 48] = [
    0x00, 0x00, 0x00, // Black
    0x11, 0x11, 0x11,
    0x22, 0x22, 0x22,
    0x33, 0x33, 0x33,
    0x44, 0x44, 0x44,
    0x55, 0x55, 0x55,
    0x66, 0x66, 0x66,
    0x77, 0x77, 0x77,
    0x88, 0x88, 0x88,
    0x99, 0x99, 0x99,
    0xaa, 0xaa, 0xaa,
    0xbb, 0xbb, 0xbb,
    0xcc, 0xcc, 0xcc,
    0xdd, 0xdd, 0xdd,
    0xee, 0xee, 0xee,
    0xff, 0xff, 0xff, // White
];

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

    // Convert to RGB and apply processing pipeline
    let mut img = img.to_rgb8();
    
    // Step 1: Apply auto contrast to improve visibility
    auto_contrast(&mut img);
    
    // Step 2: Resize image appropriately for the device
    let mut processed = resize_image_kcc_style(DynamicImage::ImageRgb8(img))?;
    
    // Step 3: Apply gamma correction for better visibility on e-ink
    processed = apply_gamma(processed, 1.2); // Gamma 1.2 based on KCC defaults
    
    // Step 4: Quantize and dither to Kindle's 16 grayscale levels
    processed = quantize_to_kindle_palette(processed);

    // Save with high quality settings
    let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(output_path)?);
    processed
        .write_to(&mut output_buffer, image::ImageFormat::Jpeg)
        .context(format!(
            "Failed to save processed image: {}",
            output_path.display()
        ))?;

    Ok(())
}

/// Auto-contrast function similar to KCC's approach
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

/// Resize image using KCC's strategy
fn resize_image_kcc_style(img: DynamicImage) -> Result<DynamicImage> {
    let (width, height) = img.dimensions();

    // Choose resize method based on whether we're upscaling or downscaling
    let filter = if width <= TARGET_WIDTH && height <= TARGET_HEIGHT {
        // For upscaling, Bicubic gives smoother results for manga
        image::imageops::FilterType::CatmullRom
    } else {
        // For downscaling, Lanczos3 preserves more detail
        image::imageops::FilterType::Lanczos3
    };

    // Calculate aspect ratios
    let ratio_device = TARGET_HEIGHT as f32 / TARGET_WIDTH as f32;
    let ratio_image = height as f32 / width as f32;

    // Determine resize strategy based on aspect ratios
    let processed = if (ratio_image - ratio_device).abs() < 0.015 {
        // Similar aspect ratios - use fit to fill the screen
        let resized = image::imageops::resize(&img, TARGET_WIDTH, TARGET_HEIGHT, filter);
        DynamicImage::from(resized)
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

/// Apply gamma correction to improve visibility on e-ink displays
fn apply_gamma(img: DynamicImage, gamma: f32) -> DynamicImage {
    let rgb = img.to_rgb8();
    let (width, height) = rgb.dimensions();
    
    // Create lookup table for gamma correction
    let mut gamma_lut = [0u8; 256];
    for i in 0..256 {
        let normalized = i as f32 / 255.0;
        let corrected = normalized.powf(1.0 / gamma);
        gamma_lut[i] = (corrected * 255.0).round() as u8;
    }
    
    let mut result = RgbImage::new(width, height);
    
    for y in 0..height {
        for x in 0..width {
            let pixel = rgb.get_pixel(x, y);
            let new_pixel = Rgb([
                gamma_lut[pixel.0[0] as usize],
                gamma_lut[pixel.0[1] as usize],
                gamma_lut[pixel.0[2] as usize],
            ]);
            result.put_pixel(x, y, new_pixel);
        }
    }
    
    DynamicImage::ImageRgb8(result)
}

/// Quantize image to the Kindle's 16-level grayscale palette using Floyd-Steinberg dithering
fn quantize_to_kindle_palette(img: DynamicImage) -> DynamicImage {
    // Convert to grayscale first
    let grayscale = img.grayscale();
    let rgb = grayscale.to_rgb8();
    let (width, height) = rgb.dimensions();
    
    // Create result image
    let mut result = RgbImage::new(width, height);
    
    // Create a mutable copy for dithering
    let mut dither_img = rgb.clone();
    
    // Extract just the grayscale levels from the palette (R=G=B)
    let mut kindle_gray_palette = [0u8; 16];
    for i in 0..16 {
        kindle_gray_palette[i] = KINDLE_PALETTE_16[i * 3]; // Take just the R value
    }
    
    // Apply Floyd-Steinberg dithering with Kindle palette
    for y in 0..height {
        for x in 0..width {
            // Get current pixel value (just read R since R=G=B in grayscale)
            let old_pixel = dither_img.get_pixel(x, y).0[0];
            
            // Find closest palette color
            let closest_color_idx = find_closest_color(old_pixel, &kindle_gray_palette);
            let new_pixel_value = kindle_gray_palette[closest_color_idx];
            
            // Update the result image (set R=G=B to the same palette value)
            let new_pixel = Rgb([new_pixel_value, new_pixel_value, new_pixel_value]);
            result.put_pixel(x, y, new_pixel);
            
            // Calculate quantization error
            let quant_error = old_pixel as i16 - new_pixel_value as i16;
            
            // Distribute the error to neighboring pixels (Floyd-Steinberg)
            if x < width - 1 {
                let pixel = dither_img.get_pixel_mut(x + 1, y);
                let value = pixel.0[0] as i16 + (quant_error * 7 / 16);
                pixel.0[0] = value.clamp(0, 255) as u8;
                pixel.0[1] = pixel.0[0]; // Keep grayscale consistent
                pixel.0[2] = pixel.0[0];
            }
            
            if y < height - 1 {
                if x > 0 {
                    let pixel = dither_img.get_pixel_mut(x - 1, y + 1);
                    let value = pixel.0[0] as i16 + (quant_error * 3 / 16);
                    pixel.0[0] = value.clamp(0, 255) as u8;
                    pixel.0[1] = pixel.0[0];
                    pixel.0[2] = pixel.0[0];
                }
                
                let pixel = dither_img.get_pixel_mut(x, y + 1);
                let value = pixel.0[0] as i16 + (quant_error * 5 / 16);
                pixel.0[0] = value.clamp(0, 255) as u8;
                pixel.0[1] = pixel.0[0];
                pixel.0[2] = pixel.0[0];
                
                if x < width - 1 {
                    let pixel = dither_img.get_pixel_mut(x + 1, y + 1);
                    let value = pixel.0[0] as i16 + (quant_error * 1 / 16);
                    pixel.0[0] = value.clamp(0, 255) as u8;
                    pixel.0[1] = pixel.0[0];
                    pixel.0[2] = pixel.0[0];
                }
            }
        }
    }
    
    DynamicImage::ImageRgb8(result)
}

/// Find the closest color in the palette to the given pixel value
fn find_closest_color(pixel_value: u8, palette: &[u8]) -> usize {
    let mut closest_idx = 0;
    let mut closest_diff = 255;
    
    for (i, &palette_value) in palette.iter().enumerate() {
        let diff = (pixel_value as i16 - palette_value as i16).abs() as u8;
        if diff < closest_diff {
            closest_diff = diff;
            closest_idx = i;
        }
    }
    
    closest_idx
}