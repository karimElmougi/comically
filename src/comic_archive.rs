use anyhow::{Context, Result};
use log::info;
use std::fs::{create_dir_all, File};
use std::io;
use std::path::Path;
use zip::ZipArchive;

use crate::Comic;

/// Extracts a CBZ file to the target directory
pub fn extract_cbz(comic: &mut Comic) -> Result<()> {
    info!("Extracting CBZ: {}", comic.input.display());

    // Create the images directory
    let images_dir = comic.images_dir();
    create_dir_all(&images_dir).context("Failed to create images directory")?;

    // Open the zip file
    let file = File::open(&comic.input).context("Failed to open CBZ file")?;
    let mut archive = ZipArchive::new(file).context("Failed to parse CBZ file as ZIP archive")?;

    // Extract all image files
    let mut extracted_count = 0;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };

        // Skip directories and non-image files
        if file.is_dir() || !has_image_extension(&outpath) {
            continue;
        }

        // Skip system files
        let file_name = outpath.file_name().unwrap().to_string_lossy();
        if file_name.starts_with(".")
            || file_name.contains("__MACOSX")
            || file_name.contains("thumbs.db")
            || file_name.contains(".DS_Store")
        {
            continue;
        }

        let target_path = images_dir.join(&*file_name);
        comic.input_page_names.push(file_name.to_string());

        // Extract the file
        let mut outfile = File::create(&target_path)
            .context(format!("Failed to create file: {}", target_path.display()))?;
        io::copy(&mut file, &mut outfile)
            .context(format!("Failed to extract file: {}", outpath.display()))?;

        extracted_count += 1;
    }

    info!("Extracted {} images", extracted_count,);

    if extracted_count == 0 {
        anyhow::bail!("No images found in the CBZ file");
    }

    Ok(())
}

/// Helper function to check if a file has an image extension
fn has_image_extension(path: &Path) -> bool {
    static VALID_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".gif"];
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        for valid_ext in VALID_EXTENSIONS {
            if valid_ext.contains(&ext_str) {
                return true;
            }
        }
    }
    false
}

// Checks for metadata in the comic archive (ComicInfo.xml)
// pub fn extract_metadata(cbz_path: &Path) -> Result<Option<PathBuf>> {
//     let file = File::open(cbz_path)?;
//     let mut archive = ZipArchive::new(file)?;

//     // archive.by_name("ComicInfo.xml").or_else(|| archive.by_name("comicinfo.xml"))?;

//     // Look for ComicInfo.xml
//     for i in 0..archive.len() {
//         let mut file = archive.by_index(i)?;
//         let outpath = match file.enclosed_name() {
//             Some(path) => path.to_owned(),
//             None => continue,
//         };

//         if outpath
//             .file_name()
//             .unwrap()
//             .to_string_lossy()
//             .to_lowercase()
//             == "comicinfo.xml"
//         {
//             // Found the metadata file
//             let metadata_path = cbz_path.with_extension("xml");
//             let mut outfile = File::create(&metadata_path)?;
//             io::copy(&mut file, &mut outfile)?;
//             return Ok(Some(metadata_path));
//         }
//     }

//     Ok(None)
// }
