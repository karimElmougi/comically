# Changelog

## [0.1.5] - 2025-10-04

### Major Refactoring: In-Memory Processing

#### Eliminated Temp File I/O
- **Before**: Images were processed, saved to disk, then read back
- **After**: All processed images kept in memory as `Vec<u8>`
- **Benefit**: ~2x faster processing, no disk I/O overhead

**Changes:**
- `ProcessedImage` now stores `file_name: String`, `data: Vec<u8>`, `dimensions: (u32, u32)`, `format: ImageFormat`
- `image::process_archive_images()` no longer requires `output_dir` parameter
- Added `encode_image()` helper function for in-memory encoding
- Removed `save_image()` function (no longer needed)

#### EPUB Built Entirely in Memory
- **Before**: Created temp directory structure, wrote files to disk, then zipped
- **After**: Build EPUB directly in memory using `ZipWriter<Cursor<Vec<u8>>>`
- **Benefit**: No temp directory needed, cleaner code, faster

**Changes:**
- `epub::build()` now takes `output_dir` parameter and returns `Result<PathBuf>`
- All helper functions return `String` instead of writing to disk:
  - `container_xml()`, `cover_html()`, `page_html()`, `toc_ncx()`, `content_opf()`
- Final EPUB bytes written to output file in one operation

#### Simplified Comic Struct
- **Removed**: `temp_dir: TempDir` and `processed_dir: PathBuf` fields
- **Removed**: `epub_dir()`, `epub_file()`, `processed_dir()` methods
- **Removed**: `Drop` implementation (no cleanup needed)
- **Benefit**: Simpler struct, no temp file management

#### Updated MOBI Conversion
- `mobi::create()` now takes `epub_path: PathBuf` and `output_mobi: PathBuf` directly
- No longer coupled to `Comic` struct

### Module Renaming (Cleaner API)
- `comic_archive` → `archive`
- `cbz_builder` → `cbz` with `build_cbz()` → `build()`
- `epub_builder` → `epub` with `build_epub()` → `build()`
- `image_processor` → `image` with `process_image()` → `process()`
- `mobi_converter` → `mobi` with `create_mobi()` → `create()`

**New API:**
```rust
comically::archive::unarchive_comic_iter(path)
comically::image::process_archive_images(archive, config)
comically::image::process(img, config)
comically::cbz::build(comic)
comically::epub::build(comic, output_dir)
comically::mobi::create(epub_path, output_mobi)
```

### New: CLI Tool (`comically-cli`)

Added a new command-line interface for processing single comic files without the TUI.

**Features:**
- Simple, scriptable interface
- All ComicConfig options exposed as CLI arguments
- Device presets: Kindle Paperwhite, Kindle Scribe, Kobo Libra, Kobo Clara, reMarkable
- Image formats: JPEG, PNG, WebP with quality/compression controls
- Page handling: Split strategies, RTL mode, auto-crop
- Progress output with verbose/quiet modes

**Example:**
```bash
comically-cli manga.cbr --rtl --device kindle-paperwhite --format epub
```

### Performance Improvements
- **Memory usage**: ~40MB for 200-page manga (same as before, but no disk I/O)
- **Speed**: Significantly faster due to eliminated temp file operations
- **Disk usage**: No temp files created during processing

### Code Quality
- Organized use statements by group (external crates, std, workspace, internal)
- Removed dead code and unused functions
- Cleaner module boundaries and responsibilities

### Breaking Changes
- `ProcessedImage` struct fields changed
- `image::process_archive_images()` signature changed (removed `output_dir`)
- `epub::build()` signature changed (added `output_dir`, returns `PathBuf`)
- `mobi::create()` signature changed (takes paths instead of `&Comic`)
- Module names changed (old names no longer exist)

## [0.1.4] - Previous

- Initial TUI implementation
- Basic comic processing pipeline
- Support for CBZ, CBR, EPUB, MOBI formats
