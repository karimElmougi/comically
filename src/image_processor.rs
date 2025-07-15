use anyhow::{Context, Result};
use imageproc::image::{
    imageops::{self, FilterType},
    load_from_memory, DynamicImage, GenericImageView, GrayImage, ImageBuffer, ImageEncoder, Luma,
    Pixel, PixelWithColorType, SubImage,
};
use imageproc::stats::histogram;
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::path::Path;
use std::sync::mpsc;

use crate::comic::{ComicConfig, ImageFormat, PngCompression, ProcessedImage, SplitStrategy};
use crate::comic_archive::ArchiveFile;
use crate::Event;

pub fn process_archive_images(
    archive: impl Iterator<Item = anyhow::Result<ArchiveFile>> + Send,
    config: ComicConfig,
    output_dir: &Path,
    comic_id: usize,
    event_tx: &mpsc::Sender<Event>,
) -> Result<Vec<ProcessedImage>> {
    log::info!("Processing archive images");

    let mut images = archive
        .par_bridge()
        .filter_map(|load| {
            if let Err(e) = &load {
                log::warn!("Failed to load image: {}", e);
            }
            load.ok()
        })
        .filter_map(|archive_file| {
            let Ok(img) = load_from_memory(&archive_file.data) else {
                log::warn!("Failed to load image: {}", archive_file.file_name.display());
                return None;
            };

            Some((archive_file, process_image(img, &config)))
        })
        .flat_map(|(archive_file, images)| {
            let result = images
                .into_iter()
                .enumerate()
                .filter_map(|(i, img)| {
                    let extension = config.image_format.extension();
                    let path = output_dir.join(format!(
                        "{}_{}_{}.{}",
                        archive_file.parent().display(),
                        archive_file.file_stem().display(),
                        i + 1,
                        extension
                    ));
                    match save_image(&img, &path, &config.image_format) {
                        Ok(_) => {
                            log::trace!("Saved image: {}", path.display());
                            Some(ProcessedImage {
                                path,
                                dimensions: img.dimensions(),
                            })
                        }
                        Err(e) => {
                            log::warn!("Failed to save {}: {}", path.display(), e);
                            None
                        }
                    }
                })
                .collect::<Vec<_>>();

            // Send progress update for each successfully processed image
            if !result.is_empty() {
                use crate::comic::{ComicStatus, ProgressEvent};
                let _ = event_tx.send(Event::Progress(ProgressEvent::ComicUpdate {
                    id: comic_id,
                    status: ComicStatus::ImageProcessed,
                }));
            }

            result
        })
        .collect::<Vec<_>>();

    images.sort_by(|a, b| a.path.as_os_str().cmp(b.path.as_os_str()));
    images.dedup_by_key(|i| i.path.as_os_str().to_owned());

    Ok(images)
}

/// Process a single image file with Kindle-optimized transformations
pub fn process_image(img: DynamicImage, config: &ComicConfig) -> Vec<GrayImage> {
    let img = transform(img.into_luma8(), config.brightness, config.gamma);

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
    I: GenericImageView<Pixel = Luma<u8>> + Send + Sync,
{
    let target = c.device_dimensions();
    let (width, height) = img.dimensions();
    let is_double_page = width > height;

    let margin = c.margin_color.map(|color| Luma([color]));

    match c.split {
        SplitStrategy::None => {
            // Just resize, no splitting or rotation
            vec![resize_image(img, target, margin)]
        }
        SplitStrategy::Split => {
            if is_double_page {
                // Split double pages
                let (left, right) = split_double_pages(img);

                let (left_resized, right_resized) = rayon::join(
                    || resize_image(&*left, target, margin),
                    || resize_image(&*right, target, margin),
                );

                // Determine order based on right_to_left setting
                let (first, second) = if c.right_to_left {
                    (right_resized, left_resized)
                } else {
                    (left_resized, right_resized)
                };

                vec![first, second]
            } else {
                vec![resize_image(img, target, margin)]
            }
        }
        SplitStrategy::Rotate => {
            if is_double_page {
                let rotated = rotate_image_90(img, c.right_to_left);
                vec![resize_image(&rotated, target, margin)]
            } else {
                vec![resize_image(img, target, margin)]
            }
        }
        SplitStrategy::RotateAndSplit => {
            if is_double_page {
                let (left, right) = split_double_pages(img);

                let mut rotated_resized = None;
                let mut left_resized = None;
                let mut right_resized = None;

                rayon::scope(|s| {
                    s.spawn(|_| {
                        let rotated = rotate_image_90(img, c.right_to_left);
                        rotated_resized = Some(resize_image(&rotated, target, margin));
                    });
                    s.spawn(|_| {
                        left_resized = Some(resize_image(&*left, target, margin));
                    });
                    s.spawn(|_| {
                        right_resized = Some(resize_image(&*right, target, margin));
                    });
                });

                let rotated_resized = rotated_resized.unwrap();
                let left_resized = left_resized.unwrap();
                let right_resized = right_resized.unwrap();

                let (first, second) = if c.right_to_left {
                    (right_resized, left_resized)
                } else {
                    (left_resized, right_resized)
                };

                vec![rotated_resized, first, second]
            } else {
                vec![resize_image(img, target, margin)]
            }
        }
    }
}

/// gamma - 0.1 to 3.0, where 1.0 = no change, <1 = brighter, >1 = more contrast
fn transform(mut img: GrayImage, brightness: i32, gamma: f32) -> GrayImage {
    let gamma = gamma.clamp(0.1, 3.0);
    // only apply gamma if it's not 1.0
    if (gamma - 1.0).abs() > 0.01 {
        imageproc::map::map_colors_mut(&mut img, |pixel| {
            let normalized = pixel[0] as f32 / 255.0;
            let corrected = normalized.powf(gamma);
            let new_value = (corrected * 255.0).round().clamp(0.0, 255.0) as u8;
            Luma([new_value])
        });
    }

    // Apply autocontrast - find actual min/max and stretch to 0-255
    let hist = histogram(&img);

    let channel_hist = &hist.channels[0];

    let min = channel_hist
        .iter()
        .position(|&count| count > 0)
        .unwrap_or(0) as u8;
    let max = channel_hist
        .iter()
        .rposition(|&count| count > 0)
        .unwrap_or(255) as u8;

    // Only stretch if there's a range to work with
    if max > min {
        img = imageproc::contrast::stretch_contrast(&img, min, max, 0, 255);
    }

    // Only apply manual adjustments if explicitly set
    if brightness != 0 {
        imageops::colorops::brighten_in_place(&mut img, brightness);
    }

    img
}

fn split_double_pages<I: GenericImageView>(img: &I) -> (SubImage<&I>, SubImage<&I>) {
    let (width, height) = img.dimensions();

    let left = imageops::crop_imm(img, 0, 0, width / 2, height);
    let right = imageops::crop_imm(img, width / 2, 0, width / 2, height);

    (left, right)
}

fn rotate_image_90<I>(img: &I, clockwise: bool) -> GrayImage
where
    I: GenericImageView<Pixel = Luma<u8>>,
{
    let (width, height) = img.dimensions();
    let mut rotated = GrayImage::new(height, width);

    for y in 0..height {
        for x in 0..width {
            let pixel = img.get_pixel(x, y);
            if clockwise {
                rotated.put_pixel(height - 1 - y, x, pixel);
            } else {
                rotated.put_pixel(y, width - 1 - x, pixel);
            }
        }
    }

    rotated
}

// Pixel values above this are considered "white"
const WHITE_THRESHOLD: u8 = 230;
// Minimum width to consider cropping
const MIN_MARGIN_WIDTH: u32 = 10;
// Extra margin to keep, avoiding cutting content
const SAFETY_MARGIN: u32 = 2;

/// Auto-crop white margins from all sides of the image
fn auto_crop(img: &GrayImage) -> Option<SubImage<&GrayImage>> {
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
        return Some(imageops::crop_imm(
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
            if nx >= 0
                && ny >= 0
                && nx < width as i32
                && ny < height as i32
                && img.get_pixel(nx as u32, ny as u32)[0] < WHITE_THRESHOLD
            {
                dark_neighbors += 1;

                // Early return if we have enough neighbors
                if dark_neighbors >= REQUIRED_NEIGHBORS {
                    return true;
                }
            }
        }
    }

    dark_neighbors >= REQUIRED_NEIGHBORS
}

fn resize_image<I>(
    img: &I,
    device_dimensions: (u32, u32),
    margin_color: Option<I::Pixel>,
) -> ImageBuffer<I::Pixel, Vec<<I::Pixel as Pixel>::Subpixel>>
where
    I: GenericImageView,
    <I as GenericImageView>::Pixel: 'static,
{
    let (target_width, target_height) = device_dimensions;
    let (width, height) = img.dimensions();

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

    let resized = imageops::resize(img, new_width, new_height, filter);

    if new_width == target_width && new_height == target_height {
        return resized;
    }

    let margin_color = match margin_color {
        Some(color) => color,
        None => return resized,
    };

    let mut img = ImageBuffer::from_pixel(target_width, target_height, margin_color);

    // Calculate centering offsets
    let x_offset = (target_width - new_width) / 2;
    let y_offset = (target_height - new_height) / 2;

    // Copy the resized image to the center of the final image
    imageops::overlay(&mut img, &resized, x_offset.into(), y_offset.into());

    img
}

/// Compress an image to JPEG format with the specified quality
pub fn compress_to_jpeg<I, W>(img: &I, writer: &mut W, quality: u8) -> Result<()>
where
    I: GenericImageView,
    <I as GenericImageView>::Pixel: PixelWithColorType + 'static,
    W: std::io::Write,
{
    let mut encoder =
        imageproc::image::codecs::jpeg::JpegEncoder::new_with_quality(writer, quality);

    encoder
        .encode_image(img)
        .with_context(|| "Failed to compress image to JPEG")?;

    Ok(())
}

/// Compress an image to PNG format with the specified compression level
pub fn compress_to_png<I, W>(img: &I, writer: &mut W, compression: PngCompression) -> Result<()>
where
    I: GenericImageView,
    <I as GenericImageView>::Pixel: PixelWithColorType + 'static,
    W: std::io::Write,
{
    use imageproc::image::codecs::png::{CompressionType, PngEncoder};

    let compression_type = match compression {
        PngCompression::Fast => CompressionType::Fast,
        PngCompression::Default => CompressionType::Default,
        PngCompression::Best => CompressionType::Best,
    };

    let encoder = PngEncoder::new_with_quality(
        writer,
        compression_type,
        imageproc::image::codecs::png::FilterType::Adaptive,
    );

    encoder
        .write_image(
            img.as_bytes(),
            img.width(),
            img.height(),
            <I::Pixel as PixelWithColorType>::COLOR_TYPE,
        )
        .with_context(|| "Failed to compress image to PNG")?;

    Ok(())
}

/// Compress an image to WebP format with the specified quality
pub fn compress_to_webp<I>(img: &I, quality: u8) -> Result<Vec<u8>>
where
    I: GenericImageView<Pixel = Luma<u8>>,
{
    let (width, height) = img.dimensions();
    let mut raw_data = Vec::with_capacity((width * height) as usize);

    for y in 0..height {
        for x in 0..width {
            let pixel = img.get_pixel(x, y);
            raw_data.push(pixel[0]);
        }
    }

    let encoder = webp::Encoder::from_rgb(&raw_data, width, height);
    let webp_data = encoder.encode(quality as f32);

    Ok(webp_data.to_vec())
}

fn save_image<I>(img: &I, path: &Path, format: &ImageFormat) -> Result<()>
where
    I: GenericImageView<Pixel = Luma<u8>>,
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

    match format {
        ImageFormat::Jpeg { quality } => {
            let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(path)?);
            compress_to_jpeg(img, &mut output_buffer, *quality)
                .with_context(|| format!("Failed to save JPEG image: {}", path.display()))?;
        }
        ImageFormat::Png { compression } => {
            let mut output_buffer = std::io::BufWriter::new(std::fs::File::create(path)?);
            compress_to_png(img, &mut output_buffer, *compression)
                .with_context(|| format!("Failed to save PNG image: {}", path.display()))?;
        }
        ImageFormat::WebP { quality } => {
            let webp_data = compress_to_webp(img, *quality)
                .with_context(|| format!("Failed to encode WebP image: {}", path.display()))?;
            std::fs::write(path, webp_data)
                .with_context(|| format!("Failed to save WebP image: {}", path.display()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use imageproc::image::{GrayImage, Luma};

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
