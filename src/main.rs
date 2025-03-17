use clap::Parser;
use std::{env, path::PathBuf};

mod comic_archive;
mod epub_builder;
mod image_processor;
mod mobi_converter;

#[derive(Parser, Debug)]
#[command(
    name = "comically",
    about = "A simple converter for comic book files to Kindle MOBI format",
    version
)]
struct Cli {
    #[arg(required = true)]
    input: PathBuf,

    #[arg(short, default_value_t = true)]
    manga_mode: bool,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    if cfg!(target_os = "macos") {
        let additional_paths = [
            "/Applications/Kindle Comic Creator/Kindle Comic Creator.app/Contents/MacOS",
            "/Applications/Kindle Previewer 3.app/Contents/lib/fc/bin/",
        ];

        let current_path = env::var("PATH").unwrap_or_default();
        let new_path = additional_paths
            .iter()
            .fold(current_path, |acc, &path| format!("{}:{}", acc, path));

        env::set_var("PATH", new_path);
    }

    let cli = Cli::parse();

    log::info!("Converting {}", cli.input.display());

    time_it("Convert to MOBI", || {
        convert_to_mobi(cli.input, cli.manga_mode)
    })?;

    Ok(())
}

fn convert_to_mobi(file: PathBuf, manga_mode: bool) -> anyhow::Result<()> {
    let title = file.file_stem().unwrap().to_string_lossy().to_string();

    let temp_dir = tempfile::tempdir()?;

    let mut comic = Comic {
        title,
        input: file,
        directory: temp_dir.path().to_path_buf(),
        input_page_names: Vec::new(),
        processed_files: Vec::new(),
        // kindle paperwhite signature edition
        device_dimensions: (1236, 1648),
        right_to_left: manga_mode,
    };

    time_it("Extract CBZ", || comic_archive::extract_cbz(&mut comic))?;

    // Process images
    time_it("Process Images", || {
        image_processor::process_images(&mut comic)
    })?;

    // Create EPUB
    time_it("Create EPUB", || epub_builder::build_epub(&comic))?;

    // Convert to MOBI
    time_it("Create MOBI", || mobi_converter::create_mobi(&comic))?;

    Ok(())
}

pub struct Comic {
    title: String,
    input: PathBuf,
    directory: PathBuf,
    input_page_names: Vec<String>,
    processed_files: Vec<ProcessedImage>,
    device_dimensions: (u32, u32),
    right_to_left: bool,
}

#[derive(Debug, Clone)]
pub struct ProcessedImage {
    path: PathBuf,
    dimensions: (u32, u32),
}

impl Comic {
    // where decompressed images are stored
    pub fn images_dir(&self) -> PathBuf {
        self.directory.join("Images")
    }

    // where processed images are stored
    pub fn processed_dir(&self) -> PathBuf {
        self.directory.join("Processed")
    }

    pub fn epub_dir(&self) -> PathBuf {
        self.directory.join("EPUB")
    }

    pub fn epub_file(&self) -> PathBuf {
        self.epub_dir().join("book.epub")
    }

    pub fn output_mobi(&self) -> PathBuf {
        let mut path = self.input.clone();
        path.set_extension("mobi");
        path
    }
}

fn time_it<F, T>(label: &str, func: F) -> T
where
    F: FnOnce() -> T,
{
    let start = std::time::Instant::now();
    let result = func();
    let duration = start.elapsed();
    log::debug!("{}: {}ms", label, duration.as_millis());
    result
}
