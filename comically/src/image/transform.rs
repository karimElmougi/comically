//! Image transformations: gamma, brightness, contrast, cropping, resizing

use fast_image_resize as fr;
use fr::images::Image as FrImage;
use imageproc::image::{imageops, GrayImage, Luma, SubImage};
use imageproc::stats::histogram;

use crate::comic::{ComicConfig, SplitStrategy};
use super::Split;

// Pixel values above this are considered "white"
const WHITE_THRESHOLD: u8 = 230;
// Minimum width to consider cropping
const MIN_MARGIN_WIDTH: u32 = 10;
// Extra margin to keep, avoiding cutting content
const SAFETY_MARGIN: u32 = 2;

/// Gamma correction lookup table
/// Only computed once per unique gamma value (256 iterations) to avoid slow float operations
static GAMMA_LUT: std::sync::OnceLock<[u8; 256]> = std::sync::OnceLock::new();

fn gamma_lut(gamma: f32) -> &'static [u8; 256] {
    GAMMA_LUT.get_or_init(|| {
        let mut lut = [0u8; 256];
        for (i, pixel) in lut.iter_mut().enumerate() {
            let normalized = i as f32 / 255.0;
            let corrected = normalized.powf(gamma);
            *pixel = (corrected * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        lut
    })
}

/// Apply gamma, brightness, and autocontrast transformations
/// 
/// gamma - 0.1 to 3.0, where 1.0 = no change, <1 = brighter, >1 = more contrast
pub(super) fn transform(mut img: GrayImage, brightness: i32, gamma: f32) -> GrayImage {
    let gamma = gamma.clamp(0.1, 3.0);
    // only apply gamma if it's not 1.0
    if (gamma - 1.0).abs() > 0.01 {
        imageproc::map::map_colors_mut(&mut img, |pixel| {
            Luma([gamma_lut(gamma)[pixel[0] as usize]])
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

/// Auto-crop white margins from all sides of the image
pub(super) fn auto_crop(img: &GrayImage) -> Option<SubImage<&GrayImage>> {
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

/// Resize image to fit device dimensions with optional margins
pub(super) fn resize_image(
    img: &GrayImage,
    device_dimensions: (u32, u32),
    margin_color: Option<u8>,
) -> GrayImage {
    let (target_width, target_height) = device_dimensions;
    let (width, height) = img.dimensions();

    // Calculate aspect-fit dimensions
    let width_ratio = target_width as f32 / width as f32;
    let height_ratio = target_height as f32 / height as f32;
    let ratio = width_ratio.min(height_ratio);

    let new_width = (width as f32 * ratio) as u32;
    let new_height = (height as f32 * ratio) as u32;

    // Choose algorithm based on scaling direction
    let algorithm = if ratio < 1.0 {
        // Downscaling: Lanczos3 preserves detail
        fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3)
    } else {
        // Upscaling: CatmullRom gives smoother results
        fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom)
    };

    // Create source image view (need to clone data since fast_image_resize requires mutable slice)
    let src_buffer = img.as_raw().clone();
    let src_image = FrImage::from_vec_u8(width, height, src_buffer, fr::PixelType::U8).unwrap();

    // Create destination buffer
    let mut dst_buffer = vec![0u8; (new_width * new_height) as usize];
    let mut dst_image =
        FrImage::from_slice_u8(new_width, new_height, &mut dst_buffer, fr::PixelType::U8).unwrap();

    // Perform resize
    let mut resizer = fr::Resizer::new();
    resizer
        .resize(
            &src_image,
            &mut dst_image,
            Some(&fr::ResizeOptions::new().resize_alg(algorithm)),
        )
        .unwrap();

    // Convert back to GrayImage
    let resized = GrayImage::from_raw(new_width, new_height, dst_buffer).unwrap();

    // If exact fit, return as-is
    if new_width == target_width && new_height == target_height {
        return resized;
    }

    // Add margins if requested
    match margin_color {
        Some(color) => {
            let mut result = GrayImage::from_pixel(target_width, target_height, Luma([color]));
            let x_offset = (target_width - new_width) / 2;
            let y_offset = (target_height - new_height) / 2;
            imageops::overlay(&mut result, &resized, x_offset.into(), y_offset.into());
            result
        }
        None => resized,
    }
}

/// Process image view with split strategy
pub(super) fn process_image_view(img: &GrayImage, c: &ComicConfig) -> Split<GrayImage> {
    let target = c.device_dimensions();
    let (width, height) = img.dimensions();
    let is_double_page = width > height;

    let margin = c.margin_color;

    match c.split {
        SplitStrategy::None => {
            // Just resize, no splitting or rotation
            Split::one(resize_image(img, target, margin))
        }
        SplitStrategy::Split => {
            if is_double_page {
                // Split double pages
                let (left, right) = split_double_pages(img);

                let (left_resized, right_resized) = rayon::join(
                    || resize_image(&left, target, margin),
                    || resize_image(&right, target, margin),
                );

                // Determine order based on right_to_left setting
                let (first, second) = if c.right_to_left {
                    (right_resized, left_resized)
                } else {
                    (left_resized, right_resized)
                };

                Split::two(first, second)
            } else {
                Split::one(resize_image(img, target, margin))
            }
        }
        SplitStrategy::Rotate => {
            if is_double_page {
                let rotated = rotate_image_90(img, c.right_to_left);
                Split::one(resize_image(&rotated, target, margin))
            } else {
                Split::one(resize_image(img, target, margin))
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
                        left_resized = Some(resize_image(&left, target, margin));
                    });
                    s.spawn(|_| {
                        right_resized = Some(resize_image(&right, target, margin));
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

                Split::three(rotated_resized, first, second)
            } else {
                Split::one(resize_image(img, target, margin))
            }
        }
    }
}

fn split_double_pages(img: &GrayImage) -> (GrayImage, GrayImage) {
    let (width, height) = img.dimensions();

    let left = imageops::crop_imm(img, 0, 0, width / 2, height).to_image();
    let right = imageops::crop_imm(img, width / 2, 0, width / 2, height).to_image();

    (left, right)
}

fn rotate_image_90(img: &GrayImage, clockwise: bool) -> GrayImage {
    let (width, height) = img.dimensions();
    let mut rotated = GrayImage::new(height, width);

    for y in 0..height {
        for x in 0..width {
            let pixel = img.get_pixel(x, y);
            if clockwise {
                rotated.put_pixel(height - 1 - y, x, *pixel);
            } else {
                rotated.put_pixel(y, width - 1 - x, *pixel);
            }
        }
    }

    rotated
}
