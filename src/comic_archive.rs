use anyhow::{Context, Result};
use std::fs::{create_dir_all, File};
use std::io;
use std::path::{Path, PathBuf};
use unrar::Archive;
use zip::ZipArchive;

use crate::Comic;

/// Extracts a comic archive to the target directory
/// supports cbz, zip, cbr, rar
pub fn unarchive_comic(comic: &mut Comic) -> Result<()> {
    log::debug!("Extracting comic: {}", comic.input.display());

    let images_dir = comic.images_dir();
    create_dir_all(&images_dir).context("Failed to create images directory")?;

    let ext = comic
        .input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "cbz" | "zip" => extract_zip(comic, &images_dir)?,
        "cbr" | "rar" => extract_rar(comic, &images_dir)?,
        _ => anyhow::bail!("Unsupported archive format: {}", ext),
    }

    if comic.input_page_names.is_empty() {
        anyhow::bail!("No images found in the archive");
    }

    log::debug!("Found {} images", comic.input_page_names.len());

    comic.input_page_names.sort();
    comic.input_page_names.dedup();

    Ok(())
}

fn extract_zip(comic: &mut Comic, images_dir: &Path) -> Result<()> {
    log::info!("extracting cbz/zip file");
    let file = File::open(&comic.input).context("Failed to open zip file")?;
    let mut reader = std::io::BufReader::new(file);
    let mut archive =
        ZipArchive::new(&mut reader).context("Failed to parse file as zip archive")?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => {
                continue;
            }
        };

        if file.is_dir() {
            continue;
        }

        let Some(file_name) = get_file_name(&outpath) else {
            continue;
        };

        let target_path = images_dir.join(&file_name);

        let outfile = File::create(&target_path)
            .context(format!("Failed to create file: {}", target_path.display()))?;
        let mut outfile = std::io::BufWriter::new(outfile);

        io::copy(&mut file, &mut outfile)
            .context(format!("Failed to extract file: {}", outpath.display()))?;

        comic.input_page_names.push(file_name);
    }

    Ok(())
}

fn extract_rar(comic: &mut Comic, images_dir: &Path) -> Result<()> {
    log::info!("processing as rar/cbr file");

    let input_path = comic.input.to_str().unwrap_or_default();
    let mut archive = Archive::new(input_path)
        .open_for_processing()
        .context("Failed to open RAR file")?;

    while let Some(header) = archive.read_header()? {
        let file_path = PathBuf::from(&header.entry().filename);

        if header.entry().is_directory() {
            archive = header.skip()?;
            continue;
        }

        let Some(file_name) = get_file_name(&file_path) else {
            archive = header.skip()?;
            continue;
        };

        let target_path = images_dir.join(&file_name);

        // Extract the file and get the archive back for the next iteration
        match header.extract_to(target_path.as_path()) {
            Ok(next_archive) => {
                archive = next_archive;
            }
            Err(err) => {
                anyhow::bail!("Failed to extract {}: {}", file_name, err);
            }
        }

        comic.input_page_names.push(file_name);
    }

    Ok(())
}

fn get_file_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|f| f.to_string_lossy())
        .filter(|f| !should_skip_file(&f))
        .filter(|_| has_image_extension(path))
        .map(|f| f.to_string())
}

fn should_skip_file(file_name: &str) -> bool {
    file_name.starts_with(".")
        || file_name.contains("__MACOSX")
        || file_name.contains("thumbs.db")
        || file_name.contains(".DS_Store")
}

fn has_image_extension(path: &Path) -> bool {
    static VALID_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        for valid_ext in VALID_EXTENSIONS {
            if valid_ext == &ext_str {
                return true;
            }
        }
    }
    false
}
