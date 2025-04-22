use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, GrayImage, PixelWithColorType};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::path::{Path, PathBuf};

use crate::comic_archive::ArchiveFile;
use crate::{ComicConfig, ProcessedImage};

pub fn process_archive_images(
    archive: impl Iterator<Item = anyhow::Result<ArchiveFile>> + Send,
    config: ComicConfig,
    output_dir: &Path,
) -> Result<Vec<ProcessedImage>> {
    let (saved_tx, saved_rx) = std::sync::mpsc::channel();
    let (save_req_tx, save_req_rx) = std::sync::mpsc::channel::<(GrayImage, PathBuf, u8)>();

    std::thread::spawn(move || {
        while let Ok((img, path, quality)) = save_req_rx.recv() {
            match save_image(&img, &path, quality) {
                Ok(_) => {
                    saved_tx
                        .send(ProcessedImage {
                            path,
                            dimensions: img.dimensions(),
                        })
                        .unwrap();
                }
                Err(e) => {
                    log::warn!("Failed to save {}: {}", path.display(), e);
                }
            }
        }
    });

    archive
        .par_bridge()
        .filter_map(|load| {
            if let Err(e) = &load {
                log::warn!("Failed to load image: {}", e);
            }
            load.ok()
        })
        .filter_map(|archive_file| {
            let Ok(img) = image::load_from_memory(&archive_file.data) else {
                log::warn!("Failed to load image: {}", archive_file.file_name.display());
                return None;
            };
            let processed = process_image(img, &config);

            Some((archive_file, processed))
        })
        .for_each(|(archive_file, images)| {
            images.into_iter().enumerate().for_each(|(i, img)| {
                let path = output_dir.join(format!(
                    "{}_{}_{}.jpg",
                    archive_file.parent().display(),
                    archive_file.file_stem().to_string_lossy(),
                    i + 1
                ));
                save_req_tx
                    .send((img, path, config.compression_quality))
                    .unwrap();
            })
        });

    drop(save_req_tx);

    let mut images = saved_rx.iter().map(|p| p.clone()).collect::<Vec<_>>();

    images.sort_by(|a, b| a.path.as_os_str().cmp(&b.path.as_os_str()));
    images.dedup_by_key(|i| i.path.as_os_str().to_owned());

    Ok(images)
}

/// Process a single image file with Kindle-optimized transformations
pub fn process_image(img: DynamicImage, config: &ComicConfig) -> Vec<GrayImage> {
    let mut img = img.into_luma8();
    auto_contrast(&mut img, config.brightness, config.contrast);

    if config.auto_crop {
        if let Some(cropped) = auto_crop(&img) {
            process_image_view(&*cropped, config)
        } else {
            process_image_view(&img, config)
        }
    } else {
        process_image_view(&img, config)
    }
}

fn process_image_view<I>(img: &I, c: &ComicConfig) -> Vec<GrayImage>
where
    I: GenericImageView<Pixel = image::Luma<u8>> + Send + Sync,
{
    let (width, height) = img.dimensions();
    let processed_images = if c.split_double_page && width > height {
        let (left, right) = split_double_pages(img);

        let (left_resized, right_resized) = rayon::join(
            || resize_image(&*left, c.device_dimensions),
            || resize_image(&*right, c.device_dimensions),
        );

        // Determine order based on right_to_left setting
        let (first, second) = if c.right_to_left {
            (right_resized, left_resized)
        } else {
            (left_resized, right_resized)
        };

        vec![first, second]
    } else {
        let resized = resize_image(img, c.device_dimensions);
        vec![resized]
    };

    processed_images
}

fn auto_contrast(img: &mut GrayImage, brightness: Option<i32>, contrast: Option<f32>) {
    if let Some(brightness) = brightness {
        image::imageops::colorops::brighten_in_place(img, brightness);
    }
    if let Some(contrast) = contrast {
        image::imageops::colorops::contrast_in_place(img, contrast);
    }
}

fn split_double_pages<I: GenericImageView>(img: &I) -> (image::SubImage<&I>, image::SubImage<&I>) {
    let (width, height) = img.dimensions();

    let left = image::imageops::crop_imm(img, 0, 0, width / 2, height);
    let right = image::imageops::crop_imm(img, width / 2, 0, width / 2, height);

    (left, right)
}

// Pixel values above this are considered "white"
const WHITE_THRESHOLD: u8 = 230;
// Minimum width to consider cropping
const MIN_MARGIN_WIDTH: u32 = 10;
// Extra margin to keep, avoiding cutting content
const SAFETY_MARGIN: u32 = 2;

/// Auto-crop white margins from all sides of the image
fn auto_crop<'a>(img: &'a GrayImage) -> Option<image::SubImage<&'a GrayImage>> {
    let (width, height) = img.dimensions();

    // Left margin: scan from left to right
    let mut left_margin = 0;
    'left: for x in 0..width {
        for y in 0..height {
            if img.get_pixel(x, y)[0] < WHITE_THRESHOLD && is_not_noise(img, x, y) {
                left_margin = x;
                break 'left;
            }
        }
    }

    // Right margin: scan from right to left
    let mut right_margin = width - 1;
    'right: for x in (0..width).rev() {
        for y in 0..height {
            if img.get_pixel(x, y)[0] < WHITE_THRESHOLD && is_not_noise(img, x, y) {
                right_margin = x;
                break 'right;
            }
        }
    }

    // Top margin: scan from top to bottom
    let mut top_margin = 0;
    'top: for y in 0..height {
        for x in 0..width {
            if img.get_pixel(x, y)[0] < WHITE_THRESHOLD && is_not_noise(img, x, y) {
                top_margin = y;
                break 'top;
            }
        }
    }

    // Bottom margin: scan from bottom to top
    let mut bottom_margin = height - 1;
    'bottom: for y in (0..height).rev() {
        for x in 0..width {
            if img.get_pixel(x, y)[0] < WHITE_THRESHOLD && is_not_noise(img, x, y) {
                bottom_margin = y;
                break 'bottom;
            }
        }
    }

    // If we didn't find any content, return None
    if left_margin >= right_margin || top_margin >= bottom_margin {
        return None;
    }

    // Apply safety margin
    left_margin = left_margin.saturating_sub(SAFETY_MARGIN);
    right_margin = (right_margin + SAFETY_MARGIN).min(width - 1);
    top_margin = top_margin.saturating_sub(SAFETY_MARGIN);
    bottom_margin = (bottom_margin + SAFETY_MARGIN).min(height - 1);

    let crop_width = right_margin.saturating_sub(left_margin).saturating_add(1);
    let crop_height = bottom_margin.saturating_sub(top_margin).saturating_add(1);

    let left_margin_size = left_margin;
    let right_margin_size = width.saturating_sub(right_margin).saturating_sub(1);
    let top_margin_size = top_margin;
    let bottom_margin_size = height.saturating_sub(bottom_margin).saturating_sub(1);

    // Only crop if at least one margin is wide enough AND we have positive crop dimensions
    let should_crop_horizontal = (left_margin_size >= MIN_MARGIN_WIDTH
        || right_margin_size >= MIN_MARGIN_WIDTH)
        && crop_width > 0
        && crop_width < width;
    let should_crop_vertical = (top_margin_size >= MIN_MARGIN_WIDTH
        || bottom_margin_size >= MIN_MARGIN_WIDTH)
        && crop_height > 0
        && crop_height < height;

    if should_crop_horizontal || should_crop_vertical {
        return Some(image::imageops::crop_imm(
            img,
            left_margin,
            top_margin,
            crop_width,
            crop_height,
        ));
    }

    None
}

/// Check if a pixel is likely to be content rather than noise
#[inline]
fn is_not_noise(img: &GrayImage, x: u32, y: u32) -> bool {
    // how many neighbors need to be dark to consider the pixel content
    const REQUIRED_NEIGHBORS: i32 = 3;
    // Look a bit farther for connected pixels
    const DISTANCE: i32 = 4;

    let (width, height) = img.dimensions();
    let mut dark_neighbors = 0;

    for dy in -DISTANCE..=DISTANCE {
        for dx in -DISTANCE..=DISTANCE {
            // skip center
            if dx == 0 && dy == 0 {
                continue;
            }

            let nx = x as i32 + dx;
            let ny = y as i32 + dy;

            // Make sure coordinates are valid
            if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
                if img.get_pixel(nx as u32, ny as u32)[0] < WHITE_THRESHOLD {
                    dark_neighbors += 1;

                    // Early return if we have enough neighbors
                    if dark_neighbors >= REQUIRED_NEIGHBORS {
                        return true;
                    }
                }
            }
        }
    }

    dark_neighbors >= REQUIRED_NEIGHBORS
}

fn resize_image<I>(
    img: &I,
    device_dimensions: (u32, u32),
) -> image::ImageBuffer<I::Pixel, Vec<<I::Pixel as image::Pixel>::Subpixel>>
where
    I: GenericImageView,
    <I as GenericImageView>::Pixel: 'static,
{
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

    let width_ratio = target_width as f32 / width as f32;
    let height_ratio = target_height as f32 / height as f32;
    let ratio = width_ratio.min(height_ratio);

    let new_width = (width as f32 * ratio) as u32;
    let new_height = (height as f32 * ratio) as u32;

    image::imageops::resize(img, new_width, new_height, filter)
}

fn save_image<I>(img: &I, path: &Path, quality: u8) -> Result<()>
where
    I: GenericImageView,
    <I as GenericImageView>::Pixel: PixelWithColorType + 'static,
{
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create directories for path: {}",
                parent.display()
            )
        })?;
    }

    let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output_buffer, quality);

    encoder
        .encode_image(img)
        .with_context(|| format!("Failed to save processed image: {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    #[test]
    fn test_basic_cropping() {
        // Create a 100x50 image with 25px left margin and 35px right margin
        let test_img = create_test_image(100, 50, 25, 35, &[]);

        let result = auto_crop(&test_img);
        assert!(result.is_some(), "Cropping should have succeeded");

        let cropped = result.unwrap();
        let (cropped_width, cropped_height) = cropped.dimensions();

        assert_eq!(cropped_height, 50);
        assert_eq!(
            cropped_width,
            100 - 60 + 2 * SAFETY_MARGIN,
            "cropped width should be 60"
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
            &[
                // left
                (5, 1),
                (5, 2),
                (5, 3),
                // right
                (95, 1),
                (95, 2),
                (95, 3),
            ],
        );

        let result = auto_crop(&test_img);

        let cropped = result.expect("Cropping should have succeeded");
        let (cropped_width, _) = cropped.dimensions();

        assert_eq!(cropped_width, 100 - 40 + 2 * SAFETY_MARGIN,);
    }

    #[test]
    fn test_no_margins() {
        let test_img = create_test_image(100, 50, 0, 0, &[]);

        let cropped_option = auto_crop(&test_img);

        assert!(
            cropped_option.is_none(),
            "Expected None for image without margins"
        );
    }

    #[test]
    fn test_large_margin() {
        let test_img = create_test_image(100, 50, 30, 5, &[]);

        let result = auto_crop(&test_img);
        assert!(
            result.is_some(),
            "Expected cropping to succeed with one large margin"
        );

        let (width, height) = result.unwrap().dimensions();
        assert_eq!(width, 100 - 35 + 2 * SAFETY_MARGIN);
        assert_eq!(height, 50);
    }

    #[test]
    fn test_complex_image() {
        // The shape has a 40px margin on both sides
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
        let result = auto_crop(&img);

        let cropped = result.expect("Cropping should have succeeded");
        let (cropped_width, _) = cropped.dimensions();

        assert_eq!(
            cropped_width,
            200 - 80 + 2 * SAFETY_MARGIN,
            "invalid cropped width"
        );
    }

    #[test]
    fn test_vertical_cropping() {
        // Create a 100x100 image with 20px top margin and 30px bottom margin
        let test_img = create_test_image_with_vertical(100, 100, 0, 0, 20, 30, &[]);

        let top = find_topmost_content(&test_img);
        let bottom = find_bottommost_content(&test_img);

        assert_eq!(top, 20, "Top content should start at y=20");
        assert_eq!(bottom, 69, "Bottom content should end at y=69");

        let result = auto_crop(&test_img);
        assert!(result.is_some(), "Cropping should have succeeded");

        let cropped = result.unwrap();
        let (_, cropped_height) = cropped.dimensions();

        assert_eq!(
            cropped_height,
            100 - 50 + 2 * SAFETY_MARGIN,
            "Cropped height {} is outside expected range [45-55]",
            cropped_height
        );
    }

    #[test]
    fn test_all_margins_cropping() {
        // Create a 200x200 image with margins on all sides
        let test_img = create_test_image_with_vertical(200, 200, 30, 40, 25, 35, &[]);

        let result = auto_crop(&test_img);
        assert!(result.is_some(), "Cropping should have succeeded");

        let cropped = result.unwrap();
        let (cropped_width, cropped_height) = cropped.dimensions();

        assert_eq!(
            cropped_width,
            200 - 70 + 2 * SAFETY_MARGIN,
            "Cropped width {} is outside expected range [125-135]",
            cropped_width
        );
        assert_eq!(
            cropped_height,
            200 - 60 + 2 * SAFETY_MARGIN,
            "Cropped height {} is outside expected range [135-145]",
            cropped_height
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
            img.put_pixel(x, y, Luma([0]));
        }

        img
    }

    // Helper function for creating test images with top and bottom margins
    fn create_test_image_with_vertical(
        width: u32,
        height: u32,
        left_margin: u32,
        right_margin: u32,
        top_margin: u32,
        bottom_margin: u32,
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
        let right_boundary = width.saturating_sub(right_margin);
        let bottom_boundary = height.saturating_sub(bottom_margin);

        for y in top_margin..bottom_boundary {
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

    fn find_topmost_content(img: &GrayImage) -> u32 {
        let (width, height) = img.dimensions();
        for y in 0..height {
            for x in 0..width {
                if img.get_pixel(x, y)[0] < 230 {
                    return y;
                }
            }
        }
        0
    }

    fn find_bottommost_content(img: &GrayImage) -> u32 {
        let (width, height) = img.dimensions();
        for y in (0..height).rev() {
            for x in 0..width {
                if img.get_pixel(x, y)[0] < 230 {
                    return y;
                }
            }
        }
        height - 1
    }
}
