use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use std::io::Cursor;

use crate::comic::{Comic, ProcessedImage};

/// Build CBZ and return the bytes
pub fn build(comic: &Comic, images: &[ProcessedImage]) -> Vec<u8> {
    let mut buffer = Vec::new();
    build_into(comic, images, &mut buffer);
    buffer
}

/// Build CBZ into the provided buffer, reusing existing allocation
pub fn build_into(comic: &Comic, images: &[ProcessedImage], buffer: &mut Vec<u8>) {
    log::debug!("Building CBZ into buffer: {:?}", comic);

    buffer.clear();
    let cursor = Cursor::new(buffer);
    let mut zip = ZipWriter::new(cursor);

    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // Add images in order
    for image in images.iter() {
        zip.start_file(&image.file_name, options).unwrap();
        std::io::Write::write_all(&mut zip, &image.data).unwrap();
    }

    // TODO: Add ComicInfo.xml if we have metadata

    zip.finish().unwrap();
}
