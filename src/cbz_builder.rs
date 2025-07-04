use anyhow::{Context, Result};
use std::fs::File;
use zip::write::FileOptions;
use zip::ZipWriter;

use crate::comic::Comic;

pub fn build_cbz(comic: &Comic) -> Result<()> {
    let output_path = comic.output_path();
    let file = File::create(&output_path)?;
    let mut zip = ZipWriter::new(file);

    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // Add images in order
    for image in comic.processed_files.iter() {
        let file_name = image.path.file_name().unwrap().to_string_lossy();
        zip.start_file(file_name, options)?;
        let image_data = std::fs::read(&image.path)
            .with_context(|| format!("Failed to read image: {:?}", image.path))?;
        std::io::Write::write_all(&mut zip, &image_data)?;
    }

    // TODO: Add ComicInfo.xml if we have metadata

    zip.finish()?;

    log::info!("Created CBZ: {:?}", output_path);

    Ok(())
}
