use anyhow::Result;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use std::fs::File;

use crate::comic::Comic;

pub fn build(comic: &Comic) -> Result<()> {
    log::info!("Building CBZ: {:?}", comic);

    let output_path = comic.output_path();
    let file = File::create(&output_path)?;
    let mut zip = ZipWriter::new(file);

    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // Add images in order
    for image in comic.processed_files.iter() {
        zip.start_file(&image.file_name, options)?;
        std::io::Write::write_all(&mut zip, &image.data)?;
    }

    // TODO: Add ComicInfo.xml if we have metadata

    zip.finish()?;

    log::info!("Created CBZ: {:?}", output_path);

    Ok(())
}
