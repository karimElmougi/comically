use anyhow::Result;
use std::fs::{self, create_dir_all, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::{write::FileOptions, CompressionMethod, ZipWriter};

use crate::{Comic, ProcessedImage};

/// Builds an EPUB file from the processed images
pub fn build_epub(comic: &Comic) -> Result<()> {
    // Create EPUB working directory
    let epub_dir = comic.epub_dir();

    create_dir_all(&epub_dir)?;

    // Create EPUB structure
    let oebps_dir = epub_dir.join("OEBPS");
    create_dir_all(&oebps_dir)?;

    let meta_inf_dir = epub_dir.join("META-INF");
    create_dir_all(&meta_inf_dir)?;

    create_mimetype_file(&epub_dir)?;
    create_container_xml(&meta_inf_dir)?;

    let cover_html_path = create_cover_page(&oebps_dir, &comic.processed_files)?;

    let mut image_map: Vec<(ProcessedImage, String)> = Vec::new();
    for (i, image) in comic.processed_files.iter().enumerate() {
        let filename = format!("image{:03}.jpg", i + 1);
        image_map.push((image.clone(), format!("Images/{}", filename)));
    }

    // Generate HTML for each image
    let html_dir = oebps_dir.clone();
    let html_files = create_html_files(&html_dir, &image_map)?;

    // Create toc.ncx
    create_toc_ncx(&comic, &oebps_dir, &cover_html_path, &html_files)?;

    // Create content.opf
    create_content_opf(
        &comic,
        &oebps_dir,
        &cover_html_path,
        &html_files,
        &image_map,
    )?;

    // Package as EPUB
    let epub_path = comic.epub_file();
    create_epub_file(&epub_dir, &epub_path, &image_map)?;

    Ok(())
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
    file.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
    )?;
    Ok(())
}

/// Creates a cover page using the first image
fn create_cover_page(oebps_dir: &Path, images: &[ProcessedImage]) -> Result<PathBuf> {
    // If no images, return early
    if images.is_empty() {
        return Err(anyhow::anyhow!("No images found to create cover page"));
    }

    // Use first image as cover
    let cover_img_path = &images[0].path;

    // Create cover HTML
    let cover_html = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head>
  <title>Cover</title>
  <meta name="viewport" content="width=device-width, height=device-height, initial-scale=1.0, maximum-scale=1.0, user-scalable=no"/>
</head>
<body>
  <div class="cover">
    <img src="{}" alt="Cover"/>
  </div>
</body>
</html>"#,
        cover_img_path.display()
    );

    let cover_html_path = oebps_dir.join("cover.html");
    let mut file = File::create(&cover_html_path)?;
    file.write_all(cover_html.as_bytes())?;

    Ok(cover_html_path)
}

/// Creates HTML files for each image
fn create_html_files(
    oebps_dir: &Path,
    images: &[(ProcessedImage, String)],
) -> Result<Vec<PathBuf>> {
    let mut html_files = Vec::new();

    for (i, (image, rel_path)) in images.iter().enumerate() {
        let filename = format!("page{:03}.html", i + 1);
        let html_path = oebps_dir.join(&filename);

        let html_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head>
  <title>Page {}</title>
  <meta name="viewport" content="width={width}, height={height}, initial-scale=1.0, maximum-scale=1.0, user-scalable=no"/>
</head>
<body>
  <div class="image">
    <img src="{}"/>
  </div>
</body>
</html>"#,
            i + 1,
            rel_path,
            width = image.dimensions.0,
            height = image.dimensions.1,
        );

        let mut file = File::create(&html_path)?;
        file.write_all(html_content.as_bytes())?;

        html_files.push(html_path);
    }

    Ok(html_files)
}

/// Creates the toc.ncx file (navigation)
fn create_toc_ncx(
    c: &Comic,
    oebps_dir: &Path,
    cover_html_path: &Path,
    html_files: &[PathBuf],
) -> Result<()> {
    let toc_path = oebps_dir.join("toc.ncx");
    let uuid = Uuid::new_v4().to_string();

    let mut nav_points = String::new();

    // Add cover to nav points
    let cover_filename = cover_html_path.file_name().unwrap().to_string_lossy();
    nav_points.push_str(&format!(
        r#"    <navPoint id="navpoint-cover" playOrder="1">
      <navLabel><text>Cover</text></navLabel>
      <content src="{}"/>
    </navPoint>
"#,
        cover_filename
    ));

    // Add content pages to nav points
    for (i, html_file) in html_files.iter().enumerate() {
        let filename = html_file.file_name().unwrap().to_string_lossy();
        nav_points.push_str(&format!(
            r#"    <navPoint id="navpoint-{}" playOrder="{}">
      <navLabel><text>Page {}</text></navLabel>
      <content src="{}"/>
    </navPoint>
"#,
            i + 2, // +2 because cover is 1
            i + 2,
            i + 1,
            filename
        ));
    }

    let toc_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="{uuid}"/>
    <meta name="dtb:depth" content="1"/>
    <meta name="dtb:totalPageCount" content="0"/>
    <meta name="dtb:maxPageNumber" content="0"/>
  </head>
  <docTitle><text>{title}</text></docTitle>
  <navMap>
{nav_points}  </navMap>
</ncx>"#,
        title = &c.title
    );

    let mut file = File::create(&toc_path)?;
    file.write_all(toc_content.as_bytes())?;

    Ok(())
}

/// Creates the content.opf file (package document)
fn create_content_opf(
    c: &Comic,
    oebps_dir: &Path,
    cover_html_path: &Path,
    html_files: &[PathBuf],
    images: &[(ProcessedImage, String)],
) -> Result<()> {
    let opf_path = oebps_dir.join("content.opf");
    let uuid = Uuid::new_v4().to_string();

    // Build manifest items
    let mut manifest = String::new();

    // Add NCX
    manifest
        .push_str(r#"    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#);
    manifest.push_str("\n");

    // Add cover HTML
    let cover_filename = cover_html_path.file_name().unwrap().to_string_lossy();
    manifest.push_str(&format!(
        r#"    <item id="cover-html" href="{}" media-type="application/xhtml+xml"/>"#,
        cover_filename
    ));
    manifest.push_str("\n");

    // Add content HTML files
    for (i, html_file) in html_files.iter().enumerate() {
        let filename = html_file.file_name().unwrap().to_string_lossy();
        manifest.push_str(&format!(
            r#"    <item id="page{}" href="{}" media-type="application/xhtml+xml"/>"#,
            i + 1,
            filename
        ));
        manifest.push_str("\n");
    }

    // Add images
    for (i, (image, rel_path)) in images.iter().enumerate() {
        let extension = image
            .path
            .extension()
            .unwrap()
            .to_string_lossy()
            .to_lowercase();

        let media_type = if extension == "png" {
            "image/png"
        } else {
            "image/jpeg"
        };

        // Special handling for the first image (cover)
        let href = rel_path;
        if i == 0 {
            manifest.push_str(&format!(
                r#"    <item id="cover-image" href="{href}" media-type="{media_type}" properties="cover-image"/>"#,
            ));
        } else {
            manifest.push_str(&format!(
                r#"    <item id="image{i}" href="{href}" media-type="{media_type}"/>"#,
            ));
        }
        manifest.push_str("\n");
    }

    // Build spine items with page spread properties
    let mut spine = String::new();

    let progression_direction = if c.config.right_to_left { "rtl" } else { "ltr" };
    // Add cover as first item in spine (typically center spread)
    spine.push_str(&format!(
        r#"    <itemref idref="cover-html" properties="page-spread-center"/>"#
    ));
    spine.push_str("\n");

    // Add content pages with alternating spreads
    let mut right_to_left = c.config.right_to_left;

    // Start from 1 because cover is already added
    for i in 1..html_files.len() {
        let spread_property = match right_to_left {
            true => "page-spread-right",
            false => "page-spread-left",
        };

        spine.push_str(&format!(
            r#"    <itemref idref="page{}" properties="{}"/>"#,
            i + 1,
            spread_property
        ));
        spine.push_str("\n");

        // Alternate page sides
        right_to_left = !right_to_left;
    }

    // Create the OPF content with page-progression-direction
    let opf_content = format!(
        r###"<?xml version="1.0" encoding="UTF-8"?>
        <package version="3.0" unique-identifier="BookID" xmlns="http://www.idpf.org/2007/opf">
          <metadata xmlns:opf="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:title>{title}</dc:title>
            <dc:language>en-US</dc:language>
            <dc:identifier id="BookID">urn:uuid:{uuid}</dc:identifier>
            <dc:creator>comically</dc:creator>
            <meta name="cover" content="cover-image"/>
            <meta name="fixed-layout" content="true"/>
            <meta name="original-resolution" content="{width}x{height}"/>
            <meta name="book-type" content="comic"/>
            <meta name="primary-writing-mode" content="{writing_mode}"/>
            <meta name="zero-gutter" content="true"/>
            <meta name="zero-margin" content="true"/>
            <meta name="ke-border-color" content="#FFFFFF"/>
            <meta name="ke-border-width" content="0"/>
            <meta name="orientation-lock" content="none"/>
            <meta name="region-mag" content="true"/>
            <meta property="rendition:spread">landscape</meta>
            <meta property="rendition:layout">pre-paginated</meta>
          </metadata>
          <manifest>{manifest}</manifest>
          <spine toc="ncx" page-progression-direction="{progression_direction}">{spine}</spine>
        </package>"###,
        title = &c.title,
        width = c.config.device_dimensions.0,
        height = c.config.device_dimensions.1,
        writing_mode = if c.config.right_to_left {
            "horizontal-rl"
        } else {
            "horizontal-lr"
        },
    );

    let mut file = File::create(&opf_path)?;
    file.write_all(opf_content.as_bytes())?;

    Ok(())
}

/// Creates the EPUB file by zipping the directory structure
fn create_epub_file(
    epub_dir: &Path,
    output_path: &Path,
    image_map: &[(ProcessedImage, String)],
) -> Result<()> {
    let file = File::create(output_path)?;
    let writer = BufWriter::new(file);
    let mut zip = ZipWriter::new(writer);

    let options_stored = FileOptions::default().compression_method(CompressionMethod::Stored);
    let options_deflated = FileOptions::default().compression_method(CompressionMethod::Deflated);

    // Add mimetype first (must not be compressed)
    let mimetype_path = epub_dir.join("mimetype");
    zip.start_file("mimetype", options_stored)?;
    let mimetype_content = fs::read(&mimetype_path)?;
    zip.write_all(&mimetype_content)?;

    // add the rest of the files
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
            let content = fs::File::open(path)?;
            let mut content = std::io::BufReader::new(content);
            std::io::copy(&mut content, &mut zip)?;
        }
    }

    // include all images
    for (image, rel_path) in image_map {
        let path = &image.path;
        let rel_path = format!("OEBPS/{}", rel_path);

        zip.start_file(rel_path, options_stored)?;
        let content = fs::File::open(path)?;
        let mut content = std::io::BufReader::new(content);
        std::io::copy(&mut content, &mut zip)?;
    }

    zip.finish()?;
    Ok(())
}
