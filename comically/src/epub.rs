use uuid::Uuid;
use zip::{
    write::{SimpleFileOptions, ZipWriter},
    CompressionMethod,
};

use std::io::{Cursor, Write};

use crate::comic::{Comic, ComicConfig, ProcessedImage};
use crate::image::ImageFormat;

/// Build EPUB and return the bytes
pub fn build(comic: &Comic, config: &ComicConfig, images: &[ProcessedImage]) -> Vec<u8> {
    let mut buffer = Vec::new();
    build_into(comic, config, images, &mut buffer);
    buffer
}

/// Build EPUB into the provided buffer, reusing existing allocation
pub fn build_into(
    comic: &Comic,
    config: &ComicConfig,
    images: &[ProcessedImage],
    buffer: &mut Vec<u8>,
) {
    log::debug!("Building EPUB into buffer: {:?}", comic);

    buffer.clear();
    let cursor = Cursor::new(buffer);
    let mut zip = ZipWriter::new(cursor);

    let options_stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let options_deflated =
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // 1. Add mimetype (must be first and uncompressed)
    zip.start_file("mimetype", options_stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    // 2. Add META-INF/container.xml
    zip.start_file("META-INF/container.xml", options_deflated)
        .unwrap();
    zip.write_all(container_xml().as_bytes()).unwrap();

    // 3. Prepare image map
    let mut image_map: Vec<(&ProcessedImage, String)> = Vec::new();
    for (i, image) in images.iter().enumerate() {
        image_map.push((image, format!("Images/image{:03}.jpg", i + 1)));
    }

    // 4. Add cover.html
    zip.start_file("OEBPS/cover.html", options_deflated)
        .unwrap();
    let cover_image_path = &image_map[0].1;
    zip.write_all(cover_html(cover_image_path).as_bytes())
        .unwrap();

    // 5. Add HTML pages for each image
    for (i, (img, img_path)) in image_map.iter().enumerate() {
        let html_path = format!("OEBPS/page{:03}.html", i + 1);
        zip.start_file(&html_path, options_deflated).unwrap();
        zip.write_all(page_html(img_path, i + 1, img.dimensions).as_bytes())
            .unwrap();
    }

    // 6. Add toc.ncx
    zip.start_file("OEBPS/toc.ncx", options_deflated).unwrap();
    zip.write_all(toc_ncx(comic, image_map.len()).as_bytes())
        .unwrap();

    // 7. Add content.opf
    zip.start_file("OEBPS/content.opf", options_deflated)
        .unwrap();
    zip.write_all(content_opf(comic, config, &image_map).as_bytes())
        .unwrap();

    // 8. Add all images
    for (image, rel_path) in &image_map {
        let path = format!("OEBPS/{}", rel_path);
        zip.start_file(&path, options_stored).unwrap();
        zip.write_all(&image.data).unwrap();
    }

    // Finish zip and get bytes
    zip.finish().unwrap();
}

fn container_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#
}

fn cover_html(cover_img_path: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head>
  <title>Cover</title>
  <meta name="viewport" content="width=device-width, height=device-height, initial-scale=1.0, maximum-scale=1.0, user-scalable=no"/>
</head>
<body style="background-color:#000000;">
  <div class="cover">
    <img src="{cover_img_path}" alt="Cover"/>
  </div>
</body>
</html>"#,
    )
}

fn page_html(img_path: &str, page_num: usize, dimensions: (u32, u32)) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head>
  <title>Page {page_num}</title>
  <meta name="viewport" content="width={}, height={}, initial-scale=1.0, maximum-scale=1.0, user-scalable=no"/>
</head>
<body>
  <div class="image">
    <img src="{img_path}"/>
  </div>
</body>
</html>"#,
        dimensions.0, dimensions.1
    )
}

fn toc_ncx(comic: &Comic, num_pages: usize) -> String {
    let uuid = Uuid::new_v4().to_string();
    let mut nav_points = String::new();

    // Add cover to nav points
    nav_points.push_str(
        r#"    <navPoint id="navpoint-cover" playOrder="1">
      <navLabel><text>Cover</text></navLabel>
      <content src="cover.html"/>
    </navPoint>
"#,
    );

    // Add content pages to nav points
    for i in 1..=num_pages {
        nav_points.push_str(&format!(
            r#"    <navPoint id="navpoint-{i}" playOrder="{}">
      <navLabel><text>Page {i}</text></navLabel>
      <content src="page{i:03}.html"/>
    </navPoint>
"#,
            i + 1, // +1 because cover is 1
        ));
    }

    format!(
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
        title = &comic.title
    )
}

fn content_opf(
    comic: &Comic,
    config: &ComicConfig,
    image_map: &[(&ProcessedImage, String)],
) -> String {
    let uuid = Uuid::new_v4().to_string();

    // Build manifest items
    let mut manifest = String::new();

    // Add NCX
    manifest
        .push_str(r#"    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#);
    manifest.push('\n');

    // Add cover HTML
    manifest.push_str(
        r#"    <item id="cover-html" href="cover.html" media-type="application/xhtml+xml"/>"#,
    );
    manifest.push('\n');

    // Add content HTML files
    for i in 0..image_map.len() {
        manifest.push_str(&format!(
            r#"    <item id="page{}" href="page{:03}.html" media-type="application/xhtml+xml"/>"#,
            i + 1,
            i + 1
        ));
        manifest.push('\n');
    }

    // Add images
    for (i, (image, rel_path)) in image_map.iter().enumerate() {
        let media_type = match image.format {
            ImageFormat::Jpeg { .. } => "image/jpeg",
            ImageFormat::Png { .. } => "image/png",
            ImageFormat::WebP { .. } => "image/webp",
        };

        // Special handling for the first image (cover)
        if i == 0 {
            manifest.push_str(&format!(
                r#"    <item id="cover-image" href="{rel_path}" media-type="{media_type}" properties="cover-image"/>"#,
            ));
        } else {
            manifest.push_str(&format!(
                r#"    <item id="image{i}" href="{rel_path}" media-type="{media_type}"/>"#,
            ));
        }
        manifest.push('\n');
    }

    // Build spine items with page spread properties
    let mut spine = String::new();
    let progression_direction = if config.right_to_left { "rtl" } else { "ltr" };

    // Add cover as first item in spine (typically center spread)
    spine.push_str(r#"    <itemref idref="cover-html" properties="page-spread-center"/>"#);
    spine.push('\n');

    let mut right_to_left = config.right_to_left;
    for i in 1..image_map.len() {
        let spread_property = if right_to_left {
            "page-spread-right"
        } else {
            "page-spread-left"
        };

        spine.push_str(&format!(
            r#"    <itemref idref="page{}" properties="{}"/>"#,
            i + 1,
            spread_property
        ));
        spine.push('\n');

        // Alternate page sides
        right_to_left = !right_to_left;
    }

    let (width, height) = config.device_dimensions();

    // Create the OPF content with page-progression-direction
    format!(
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
            <meta name="ke-border-color" content="#000000"/>
            <meta name="ke-border-width" content="0"/>
            <meta name="orientation-lock" content="none"/>
            <meta name="region-mag" content="true"/>
            <meta property="rendition:spread">landscape</meta>
            <meta property="rendition:layout">pre-paginated</meta>
          </metadata>
          <manifest>{manifest}</manifest>
          <spine toc="ncx" page-progression-direction="{progression_direction}">{spine}</spine>
        </package>"###,
        title = &comic.title,
        writing_mode = if config.right_to_left {
            "horizontal-rl"
        } else {
            "horizontal-lr"
        },
    )
}
