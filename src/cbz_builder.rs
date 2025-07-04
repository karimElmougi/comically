use anyhow::{Context, Result};
use std::fs::File;
use zip::write::FileOptions;
use zip::ZipWriter;

use crate::comic::Comic;

pub fn build_cbz(comic: &Comic) -> Result<()> {
    let cbz_path = comic.temp_dir.path().join("book.cbz");
    let file = File::create(&cbz_path)?;
    let mut zip = ZipWriter::new(file);
    
    let options = FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    
    // Add images in order
    for (index, image) in comic.processed_files.iter().enumerate() {
        let file_name = format!("{:04}.jpg", index + 1);
        
        zip.start_file(&file_name, options)?;
        let image_data = std::fs::read(&image.path)
            .with_context(|| format!("Failed to read image: {:?}", image.path))?;
        std::io::Write::write_all(&mut zip, &image_data)?;
    }
    
    // TODO: Add ComicInfo.xml if we have metadata
    
    zip.finish()?;
    
    // Move CBZ to final destination
    let output_path = comic.output_path();
    std::fs::rename(&cbz_path, &output_path)
        .with_context(|| format!("Failed to move CBZ to output: {:?}", output_path))?;
    
    log::info!("Created CBZ: {:?}", output_path);
    
    Ok(())
}