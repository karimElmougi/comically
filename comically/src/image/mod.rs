//! Image processing pipeline for manga/comic optimization

mod decode;
mod encode;
mod transform;

// Re-export public API
pub use encode::{compress_to_jpeg, compress_to_png, compress_to_webp, PngCompression};

use anyhow::Result;
use arrayvec::ArrayVec;
use imageproc::image::DynamicImage;
use rayon::iter::ParallelIterator;
use rayon::slice::ParallelSlice;

use crate::archive::ArchiveFile;
use crate::comic::{ComicConfig, ProcessedImage};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ImageFormat {
    Jpeg { quality: u8 },
    Png { compression: PngCompression },
    WebP { quality: u8 },
}

impl ImageFormat {
    pub fn cycle(&self) -> Self {
        match self {
            ImageFormat::Jpeg { .. } => ImageFormat::Png {
                compression: PngCompression::Default,
            },
            ImageFormat::Png { .. } => ImageFormat::WebP { quality: 85 },
            ImageFormat::WebP { .. } => ImageFormat::Jpeg { quality: 85 },
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg { .. } => "jpg",
            ImageFormat::Png { .. } => "png",
            ImageFormat::WebP { .. } => "webp",
        }
    }

    pub fn adjust_quality(&mut self, increase: bool, fine: bool) {
        let step = if fine { 1 } else { 5 };
        match self {
            ImageFormat::Jpeg { quality } | ImageFormat::WebP { quality } => {
                if increase {
                    *quality = (*quality + step).min(100);
                } else {
                    *quality = quality.saturating_sub(step);
                }
            }
            ImageFormat::Png { compression } => {
                *compression = if increase {
                    match compression {
                        PngCompression::Fast => PngCompression::Default,
                        PngCompression::Default => PngCompression::Best,
                        PngCompression::Best => PngCompression::Best,
                    }
                } else {
                    match compression {
                        PngCompression::Fast => PngCompression::Fast,
                        PngCompression::Default => PngCompression::Fast,
                        PngCompression::Best => PngCompression::Default,
                    }
                };
            }
        }
    }
}

/// Stack-allocated container for 1-3 images (no heap allocation)
pub struct Split<T>(ArrayVec<T, 3>);

impl<T> Split<T> {
    #[inline(always)]
    pub fn one(t: T) -> Self {
        let mut vec = ArrayVec::new();
        vec.push(t);
        Split(vec)
    }

    #[inline(always)]
    pub fn two(t1: T, t2: T) -> Self {
        let mut vec = ArrayVec::new();
        vec.push(t1);
        vec.push(t2);
        Split(vec)
    }

    #[inline(always)]
    pub fn three(t1: T, t2: T, t3: T) -> Self {
        Split(ArrayVec::from([t1, t2, t3]))
    }

    #[inline(always)]
    pub fn map<U, F: FnMut(T) -> U>(self, f: F) -> Split<U> {
        Split(self.0.into_iter().map(f).collect())
    }
}

impl<T> IntoIterator for Split<T> {
    type Item = T;
    type IntoIter = <ArrayVec<T, 3> as IntoIterator>::IntoIter;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

pub fn process_archive_images(
    archive: impl Iterator<Item = anyhow::Result<ArchiveFile>> + Send,
    config: &ComicConfig,
) -> Result<Vec<ProcessedImage>> {
    log::info!("Processing archive images");

    // 1. Collect archive files (serial, fast - no parallelism overhead)
    let files: Vec<ArchiveFile> = archive
        .filter_map(|result| {
            result
                .map_err(|e| log::warn!("Failed to load archive file: {}", e))
                .ok()
        })
        .collect();

    log::debug!("Loaded {} files from archive", files.len());

    // Calculate chunk size to minimize rayon overhead
    // Use larger chunks to reduce coordination overhead
    let num_threads = rayon::current_num_threads();
    let chunk_size = (files.len() / num_threads).max(1);

    log::debug!(
        "Processing {} files with {} threads, chunk size: {}",
        files.len(),
        num_threads,
        chunk_size
    );

    // 2. Single parallel stage: decode + process + encode
    // This eliminates intermediate Vec allocation and keeps data hot in cache
    let mut images: Vec<ProcessedImage> = files
        .par_chunks(chunk_size)
        .flat_map_iter(|chunk| {
            chunk.iter().flat_map(|archive_file| {
                let mut encoded_images = ArrayVec::<ProcessedImage, 3>::new();

                // Decode image
                let img = match decode::decode(&archive_file.data) {
                    Ok(img) => img,
                    Err(e) => {
                        log::warn!(
                            "Failed to decode {}: {}",
                            archive_file.file_name.display(),
                            e
                        );
                        return encoded_images;
                    }
                };

                // Process image (transform, crop, resize, split)
                let processed_images = process(img, config);

                // Encode immediately while data is hot in cache
                for (i, img) in processed_images.into_iter().enumerate() {
                    match encode::encode_image_part(archive_file, &img, i, config.image_format) {
                        Ok(processed) => encoded_images.push(processed),
                        Err(e) => {
                            log::warn!(
                                "Failed to encode {}: {}",
                                archive_file.file_name.display(),
                                e
                            );
                        }
                    }
                }
                encoded_images
            })
        })
        .collect();

    // 4. Serial sort + dedup (fast, no benefit from parallelism)
    images.sort_unstable_by(|a, b| a.file_name.cmp(&b.file_name));
    images.dedup_by(|a, b| a.file_name == b.file_name);

    Ok(images)
}

/// Process a single image file with Kindle-optimized transformations
pub fn process(img: DynamicImage, config: &ComicConfig) -> Split<DynamicImage> {
    let img = transform::transform(img.into_luma8(), config.brightness, config.gamma);

    let gray_images = if config.auto_crop {
        if let Some(cropped) = transform::auto_crop(&img) {
            transform::process_image_view(&cropped.to_image(), config)
        } else {
            transform::process_image_view(&img, config)
        }
    } else {
        transform::process_image_view(&img, config)
    };

    // Convert GrayImage to DynamicImage
    gray_images.map(DynamicImage::ImageLuma8)
}
