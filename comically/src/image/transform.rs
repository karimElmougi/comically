//! Image transformations: gamma, brightness, contrast, cropping, resizing

use fast_image_resize as fr;
use fr::images::CroppedImage as FrCroppedImage;
use fr::images::Image as FrImage;
use fr::images::ImageRef as FrImageRef;
use imageproc::image::{imageops, GrayImage, Luma};
use imageproc::stats::histogram;

use super::Split;
use crate::comic::{ComicConfig, SplitStrategy};

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

/// Trait for zero-copy image views compatible with fast_image_resize.
///
/// This trait abstracts over owned images ([`Image`]) and borrowed views
/// ([`CroppedImage`]) to enable efficient operations without copying pixel data.
/// All implementations integrate directly with `fast_image_resize` for
/// high-performance resizing.
pub trait Img {
    /// Returns the dimensions (width, height) of the image.
    fn dimensions(&self) -> (u32, u32);

    /// Gets the pixel value at the specified coordinates.
    ///
    /// # Panics
    /// May panic if coordinates are out of bounds.
    fn get_pixel(&self, x: u32, y: u32) -> u8;

    /// Creates a zero-copy cropped view of this image.
    fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> CroppedImage<'_>;

    /// Converts this image into a view compatible with fast_image_resize.
    fn as_fr_image(&self) -> impl fr::IntoImageView;
}

/// Owned grayscale image optimized for zero-copy operations.
///
/// This type integrates with `fast_image_resize` to avoid unnecessary copies
/// during cropping and resizing operations. Pixel data is stored in row-major
/// order as a flat `Vec<u8>`.
pub struct Image {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Image {
    #[inline(always)]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Apply gamma correction to an image
    ///
    /// gamma - 0.1 to 3.0, where 1.0 = no change, <1 = brighter, >1 = more contrast
    #[inline]
    pub fn gamma(mut self, gamma: f32) -> Image {
        let gamma = gamma.clamp(0.1, 3.0);
        // only apply gamma if it's not 1.0
        if (gamma - 1.0).abs() > 0.01 {
            for pixel in self.data.iter_mut() {
                *pixel = gamma_lut(gamma)[*pixel as usize];
            }
        }
        self
    }

    /// Adjust the brightness of an image
    ///
    /// brightness - -255 to 255, where 0 = no change, <0 = darker, >0 = brighter
    #[inline(always)]
    pub fn brightness(self, brightness: i32) -> Image {
        let mut img = GrayImage::from(self);
        imageops::colorops::brighten_in_place(&mut img, brightness);
        img.into()
    }

    /// Apply autocontrast to an image
    ///
    /// This function stretches the contrast of the image to the full range of 0-255
    pub fn autocontrast(self) -> Image {
        let mut img = GrayImage::from(self);
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
        img.into()
    }

    /// Auto-crop white margins from all sides of the image
    pub fn auto_crop(&self) -> CroppedImage<'_> {
        let (width, height) = self.dimensions();

        // Left margin: scan from left to right
        let mut left_margin = 0;
        'left: for x in 0..width {
            for y in 0..height {
                if self.get_pixel(x, y) < WHITE_THRESHOLD && is_not_noise(self, x, y) {
                    left_margin = x;
                    break 'left;
                }
            }
        }

        // Right margin: scan from right to left
        let mut right_margin = width - 1;
        'right: for x in (0..width).rev() {
            for y in 0..height {
                if self.get_pixel(x, y) < WHITE_THRESHOLD && is_not_noise(self, x, y) {
                    right_margin = x;
                    break 'right;
                }
            }
        }

        // Top margin: scan from top to bottom
        let mut top_margin = 0;
        'top: for y in 0..height {
            for x in 0..width {
                if self.get_pixel(x, y) < WHITE_THRESHOLD && is_not_noise(self, x, y) {
                    top_margin = y;
                    break 'top;
                }
            }
        }

        // Bottom margin: scan from bottom to top
        let mut bottom_margin = height - 1;
        'bottom: for y in (0..height).rev() {
            for x in 0..width {
                if self.get_pixel(x, y) < WHITE_THRESHOLD && is_not_noise(self, x, y) {
                    bottom_margin = y;
                    break 'bottom;
                }
            }
        }

        // If we didn't find any content, return None
        if left_margin >= right_margin || top_margin >= bottom_margin {
            return self.crop(0, 0, width, height);
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
            self.crop(left_margin, top_margin, crop_width, crop_height)
        } else {
            self.crop(0, 0, width, height)
        }
    }
}

impl Img for Image {
    #[inline(always)]
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    #[inline(always)]
    fn get_pixel(&self, x: u32, y: u32) -> u8 {
        self.data[y as usize * self.width as usize + x as usize]
    }

    #[inline(always)]
    fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> CroppedImage<'_> {
        let image = FrImageRef::new(
            self.width,
            self.height,
            self.data.as_slice(),
            fr::PixelType::U8,
        )
        .unwrap();
        CroppedImage {
            image,
            left: x,
            top: y,
            width,
            height,
        }
    }

    #[inline(always)]
    fn as_fr_image(&self) -> impl fr::IntoImageView {
        FrImageRef::new(
            self.width,
            self.height,
            self.data.as_slice(),
            fr::PixelType::U8,
        )
        .unwrap()
    }
}

impl From<GrayImage> for Image {
    #[inline(always)]
    fn from(img: GrayImage) -> Self {
        let (width, height) = img.dimensions();
        let data = img.into_raw();
        Image {
            width,
            height,
            data,
        }
    }
}

impl From<Image> for GrayImage {
    #[inline(always)]
    fn from(img: Image) -> Self {
        GrayImage::from_raw(img.width, img.height, img.data).unwrap()
    }
}

/// Zero-copy view into an image region.
///
/// Wraps `fast_image_resize::ImageRef` to provide efficient cropping
/// without allocating new buffers. The view is valid as long as the source
/// image remains alive (enforced by the lifetime `'a`).
pub struct CroppedImage<'a> {
    image: FrImageRef<'a>,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

impl Img for CroppedImage<'_> {
    #[inline(always)]
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    #[inline(always)]
    fn get_pixel(&self, x: u32, y: u32) -> u8 {
        assert!(x < self.width);
        assert!(y < self.height);
        let x = x + self.left;
        let y = y + self.top;
        self.image.buffer()[y as usize * self.image.width() as usize + x as usize]
    }

    #[inline(always)]
    fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> CroppedImage<'_> {
        let left = self.left + x;
        let top = self.top + y;
        CroppedImage {
            image: self.image,
            left,
            top,
            width,
            height,
        }
    }

    #[inline(always)]
    fn as_fr_image(&self) -> impl fr::IntoImageView {
        FrCroppedImage::new(&self.image, self.left, self.top, self.width, self.height).unwrap()
    }
}

/// Checks if a pixel is likely to be content rather than noise.
///
/// Examines neighboring pixels within a 4-pixel radius. A pixel is considered
/// content (not noise) if at least 3 neighbors are also dark (below threshold).
#[inline]
fn is_not_noise(img: &Image, x: u32, y: u32) -> bool {
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
                && img.get_pixel(nx as u32, ny as u32) < WHITE_THRESHOLD
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

/// Resizes image to fit device dimensions with optional margins.
///
/// Uses `fast_image_resize` with Lanczos3 for downscaling and CatmullRom for upscaling.
/// If the resized image doesn't exactly match the target dimensions and `margin_color`
/// is specified, adds centered margins of the specified color.
fn resize<I: Img>(img: I, device_dimensions: (u32, u32), margin_color: Option<u8>) -> Image {
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

    // Create destination buffer
    let mut dst_buffer = vec![0u8; (new_width * new_height) as usize];
    let mut dst_image =
        FrImage::from_slice_u8(new_width, new_height, &mut dst_buffer, fr::PixelType::U8).unwrap();

    // Perform resize
    let mut resizer = fr::Resizer::new();
    resizer
        .resize(
            &img.as_fr_image(),
            &mut dst_image,
            Some(&fr::ResizeOptions::new().resize_alg(algorithm)),
        )
        .unwrap();

    // Convert back to GrayImage
    let resized = GrayImage::from_raw(new_width, new_height, dst_buffer).unwrap();

    // If exact fit, return as-is
    if new_width == target_width && new_height == target_height {
        return resized.into();
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
    .into()
}

/// Process image with split and rotation strategies.
///
/// Applies the configured split strategy (none, split, rotate, or rotate+split)
/// and resizes the resulting images to fit the target device dimensions.
///
/// # Returns
/// A [`Split`] containing 1-3 processed images depending on the strategy.
pub fn split_rotate<I: Img + Send + Sync>(img: I, c: &ComicConfig) -> Split<Image> {
    let target = c.device_dimensions();
    let (width, height) = img.dimensions();
    let is_double_page = width > height;

    let margin = c.margin_color;

    match c.split {
        SplitStrategy::None => {
            // Just resize, no splitting or rotation
            Split::one(resize(img, target, margin))
        }
        SplitStrategy::Split => {
            if is_double_page {
                // Split double pages
                let (left, right) = split_double_pages(&img);

                let (left_resized, right_resized) = rayon::join(
                    || resize(left, target, margin),
                    || resize(right, target, margin),
                );

                // Determine order based on right_to_left setting
                let (first, second) = if c.right_to_left {
                    (right_resized, left_resized)
                } else {
                    (left_resized, right_resized)
                };

                Split::two(first, second)
            } else {
                Split::one(resize(img, target, margin))
            }
        }
        SplitStrategy::Rotate => {
            if is_double_page {
                let rotated = rotate_image_90(&img, c.right_to_left);
                Split::one(resize(rotated, target, margin))
            } else {
                Split::one(resize(img, target, margin))
            }
        }
        SplitStrategy::RotateAndSplit => {
            if is_double_page {
                let (left, right) = split_double_pages(&img);

                let mut rotated_resized = None;
                let mut left_resized = None;
                let mut right_resized = None;

                rayon::scope(|s| {
                    s.spawn(|_| {
                        let rotated = rotate_image_90(&img, c.right_to_left);
                        rotated_resized = Some(resize(rotated, target, margin));
                    });
                    s.spawn(|_| {
                        left_resized = Some(resize(left, target, margin));
                    });
                    s.spawn(|_| {
                        right_resized = Some(resize(right, target, margin));
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
                Split::one(resize(img, target, margin))
            }
        }
    }
}

/// Splits a double-page spread into left and right halves (zero-copy).
fn split_double_pages<I: Img>(img: &I) -> (CroppedImage<'_>, CroppedImage<'_>) {
    let (width, height) = img.dimensions();

    let left = img.crop(0, 0, width / 2, height);
    let right = img.crop(width / 2, 0, width / 2, height);

    (left, right)
}

/// Rotates an image 90 degrees clockwise or counter-clockwise.
///
/// Note: This operation requires copying pixels into a new buffer.
fn rotate_image_90<I: Img>(img: &I, clockwise: bool) -> Image {
    let (width, height) = img.dimensions();
    let mut rotated = GrayImage::new(height, width);

    for y in 0..height {
        for x in 0..width {
            let pixel = img.get_pixel(x, y);
            if clockwise {
                rotated.put_pixel(height - 1 - y, x, Luma([pixel]));
            } else {
                rotated.put_pixel(y, width - 1 - x, Luma([pixel]));
            }
        }
    }

    rotated.into()
}
