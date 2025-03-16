use anyhow::{Context, Result};
use log::info;
use std::fs::{self, create_dir_all, File};
use std::io::{Write, BufWriter};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::{ZipWriter, CompressionMethod, write::FileOptions};

/// Builds an EPUB file from the processed images
pub fn build_epub(images_dir: PathBuf) -> Result<PathBuf> {
    info!("Building EPUB from {}", images_dir.display());
    
    // Create EPUB working directory
    let parent = images_dir.parent().unwrap_or(&images_dir);
    let epub_dir = parent.join("EPUB");
    create_dir_all(&epub_dir)?;
    
    // Create EPUB structure
    let oebps_dir = epub_dir.join("OEBPS");
    create_dir_all(&oebps_dir)?;
    
    let meta_inf_dir = epub_dir.join("META-INF");
    create_dir_all(&meta_inf_dir)?;
    
    // Create mimetype file
    create_mimetype_file(&epub_dir)?;
    
    // Create container.xml
    create_container_xml(&meta_inf_dir)?;
    
    // Copy images to OEBPS/Images
    let images_output_dir = oebps_dir.join("Images");
    create_dir_all(&images_output_dir)?;
    
    let mut image_paths = Vec::new();
    for entry in WalkDir::new(&images_dir).sort_by_file_name().into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        
        let path = entry.path();
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if ["jpg", "jpeg", "png"].contains(&extension.to_lowercase().as_str()) {
            let filename = path.file_name().unwrap();
            let dest_path = images_output_dir.join(filename);
            fs::copy(path, &dest_path)?;
            image_paths.push(dest_path);
        }
    }
    
    // Generate HTML for each image
    let html_dir = oebps_dir.clone();
    let html_files = create_html_files(&html_dir, &image_paths)?;
    
    // Create toc.ncx
    create_toc_ncx(&oebps_dir, &html_files)?;
    
    // Create content.opf
    create_content_opf(&oebps_dir, &html_files, &image_paths)?;
    
    // Package as EPUB
    let epub_path = parent.join("book.epub");
    create_epub_file(&epub_dir, &epub_path)?;
    
    info!("EPUB created at {}", epub_path.display());
    
    Ok(epub_path)
}

/// Creates the mimetype file (must be first in the EPUB and not compressed)
fn create_mimetype_file(epub_dir: &Path) -> Result<()> {
    let mimetype_path = epub_dir.join("mimetype");
    let mut file = File::create(&mimetype_path)?;
    file.write_all(b"application/epub+zip")?;
    Ok(())
}

/// Creates the META-INF/container.xml file
fn create_container_xml(meta_inf_dir: &Path) -> Result<()> {
    let container_path = meta_inf_dir.join("container.xml");
    let mut file = File::create(&container_path)?;
    file.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#)?;
    Ok(())
}

/// Creates HTML files for each image
fn create_html_files(oebps_dir: &Path, image_paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut html_files = Vec::new();
    
    for (i, image_path) in image_paths.iter().enumerate() {
        let filename = format!("page{:03}.html", i + 1);
        let html_path = oebps_dir.join(&filename);
        
        let image_filename = image_path.file_name().unwrap().to_string_lossy();
        let image_rel_path = format!("Images/{}", image_filename);
        
        let html_content = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head>
  <title>Page {}</title>
  <meta name="viewport" content="width=device-width, height=device-height, initial-scale=1.0, maximum-scale=1.0, user-scalable=no"/>
  <style>
    body {{ margin: 0; padding: 0; text-align: center; }}
    div.image {{ height: 100vh; width: 100vw; margin: 0; padding: 0; text-align: center; }}
    img {{ max-width: 100%; max-height: 100%; object-fit: contain; }}
  </style>
</head>
<body>
  <div class="image">
    <img src="{}" alt="Page {}"/>
  </div>
</body>
</html>"#, i + 1, image_rel_path, i + 1);
        
        let mut file = File::create(&html_path)?;
        file.write_all(html_content.as_bytes())?;
        
        html_files.push(html_path);
    }
    
    Ok(html_files)
}

/// Creates the toc.ncx file (navigation)
fn create_toc_ncx(oebps_dir: &Path, html_files: &[PathBuf]) -> Result<()> {
    let toc_path = oebps_dir.join("toc.ncx");
    let uuid = Uuid::new_v4().to_string();
    
    let mut nav_points = String::new();
    for (i, html_file) in html_files.iter().enumerate() {
        let filename = html_file.file_name().unwrap().to_string_lossy();
        nav_points.push_str(&format!(r#"    <navPoint id="navpoint-{}" playOrder="{}">
      <navLabel><text>Page {}</text></navLabel>
      <content src="{}"/>
    </navPoint>
"#, i + 1, i + 1, i + 1, filename));
    }
    
    let toc_content = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="{}"/>
    <meta name="dtb:depth" content="1"/>
    <meta name="dtb:totalPageCount" content="0"/>
    <meta name="dtb:maxPageNumber" content="0"/>
  </head>
  <docTitle><text>Comic Book</text></docTitle>
  <navMap>
{}  </navMap>
</ncx>"#, uuid, nav_points);
    
    let mut file = File::create(&toc_path)?;
    file.write_all(toc_content.as_bytes())?;
    
    Ok(())
}

/// Creates the content.opf file (package document)
fn create_content_opf(oebps_dir: &Path, html_files: &[PathBuf], image_paths: &[PathBuf]) -> Result<()> {
    let opf_path = oebps_dir.join("content.opf");
    let uuid = Uuid::new_v4().to_string();
    
    // Build manifest items
    let mut manifest = String::new();
    
    // Add NCX
    manifest.push_str(r#"    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#);
    manifest.push_str("\n");
    
    // Add HTML files
    for (i, html_file) in html_files.iter().enumerate() {
        let filename = html_file.file_name().unwrap().to_string_lossy();
        manifest.push_str(&format!(r#"    <item id="page{}" href="{}" media-type="application/xhtml+xml"/>"#, 
            i + 1, filename));
        manifest.push_str("\n");
    }
    
    // Add images
    for (i, image_path) in image_paths.iter().enumerate() {
        let filename = image_path.file_name().unwrap().to_string_lossy();
        let extension = image_path.extension().unwrap().to_string_lossy().to_lowercase();
        
        let media_type = if extension == "png" {
            "image/png"
        } else {
            "image/jpeg"
        };
        
        manifest.push_str(&format!(r#"    <item id="image{}" href="Images/{}" media-type="{}"/>"#,
            i + 1, filename, media_type));
        manifest.push_str("\n");
    }
    
    // Build spine items
    let mut spine = String::new();
    for i in 0..html_files.len() {
        spine.push_str(&format!(r#"    <itemref idref="page{}"/>"#, i + 1));
        spine.push_str("\n");
    }
    
    // Create the OPF content
    let opf_content = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="BookID">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Comic Book</dc:title>
    <dc:language>en</dc:language>
    <dc:identifier id="BookID">urn:uuid:{}</dc:identifier>
    <dc:creator>Comically</dc:creator>
    <dc:publisher>Comically</dc:publisher>
    <dc:date>{}</dc:date>
    <meta property="dcterms:modified">{}</meta>
  </metadata>
  <manifest>
{}  </manifest>
  <spine toc="ncx">
{}  </spine>
</package>"#, 
        uuid, 
        chrono::Local::now().format("%Y-%m-%d"),
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%SZ"),
        manifest,
        spine
    );
    
    let mut file = File::create(&opf_path)?;
    file.write_all(opf_content.as_bytes())?;
    
    Ok(())
}

/// Creates the EPUB file by zipping the directory structure
fn create_epub_file(epub_dir: &Path, output_path: &Path) -> Result<()> {
    let file = File::create(output_path)?;
    let writer = BufWriter::new(file);
    let mut zip = ZipWriter::new(writer);
    
    // Options for no compression
    let options_stored = FileOptions::default()
        .compression_method(CompressionMethod::Stored);
    
    // Options with compression
    let options_deflated = FileOptions::default()
        .compression_method(CompressionMethod::Deflated);
    
    // Add mimetype first (must not be compressed)
    let mimetype_path = epub_dir.join("mimetype");
    zip.start_file("mimetype", options_stored)?;
    let mimetype_content = fs::read(&mimetype_path)?;
    zip.write_all(&mimetype_content)?;
    
    // Add the rest of the files
    for entry in WalkDir::new(epub_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        
        // Skip the mimetype file (already added) and the output file
        if path == mimetype_path || path == output_path {
            continue;
        }
        
        if path.is_file() {
            let rel_path = path.strip_prefix(epub_dir)?;
            let rel_path_str = rel_path.to_str().unwrap();
            
            zip.start_file(rel_path_str, options_deflated)?;
            let content = fs::read(path)?;
            zip.write_all(&content)?;
        }
    }
    
    zip.finish()?;
    Ok(())
}