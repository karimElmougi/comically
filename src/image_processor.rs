use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, ImageFormat};
use log::{info, warn};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// Default Kindle dimensions (Paperwhite)
const TARGET_WIDTH: u32 = 1072;
const TARGET_HEIGHT: u32 = 1448;

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

/// Process a single image file
fn process_image(input_path: &Path, output_path: &Path) -> Result<()> {
    // Load the image
    let img = image::open(input_path);

    if let Err(e) = &img {
        log::error!("Failed to open image: {e:?}");
    }

    let img = img.context(format!("Failed to open image: {}", input_path.display()))?;

    // Apply processing
    let processed = resize_image(img)?;

    // Save the processed image
    processed
        .save_with_format(output_path, ImageFormat::Jpeg)
        .context(format!(
            "Failed to save processed image: {}",
            output_path.display()
        ))?;

    Ok(())
}

/// Resize image to target dimensions while maintaining aspect ratio
fn resize_image(img: DynamicImage) -> Result<DynamicImage> {
    let (width, height) = img.dimensions();

    // Calculate scaling factors
    let width_ratio = TARGET_WIDTH as f32 / width as f32;
    let height_ratio = TARGET_HEIGHT as f32 / height as f32;

    // Choose the smaller ratio to maintain aspect ratio
    let ratio = width_ratio.min(height_ratio);

    // Calculate new dimensions
    let new_width = (width as f32 * ratio) as u32;
    let new_height = (height as f32 * ratio) as u32;

    // Resize using Lanczos3 filter (good quality)
    let resized = img.resize(new_width, new_height, image::imageops::FilterType::Lanczos3);

    Ok(resized)
}
