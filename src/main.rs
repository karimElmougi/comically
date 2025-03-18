mod comic_archive;
mod epub_builder;
mod image_processor;
mod mobi_converter;

use clap::Parser;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use std::{
    env,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};
use tracing::{debug, info, instrument, span, Level};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
// use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(
    name = "comically",
    about = "A simple converter for comic book files to Kindle MOBI format",
    version
)]
struct Cli {
    /// the input files to process. can be a directory or a file
    #[arg(required = true)]
    input: Vec<PathBuf>,

    /// whether to read the comic from right to left
    #[arg(long, short, default_value_t = true)]
    manga_mode: bool,

    /// the jpg compression quality of the images, between 0 and 100
    #[arg(long, short, default_value_t = 75)]
    quality: u8,

    /// the prefix to add to the title of the comic + the output file
    #[arg(long, short)]
    prefix: Option<String>,

    /// the number of threads to use for processing
    /// defaults to the number of logical CPUs
    #[arg(short)]
    threads: Option<usize>,

    /// auto-crop the dead space on the left and right of the pages
    #[arg(long, default_value_t = true)]
    auto_crop: bool,
}

fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber
    let indicatif_layer =
        tracing_indicatif::IndicatifLayer::new().with_max_progress_bars(u64::MAX, None);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(indicatif_layer.get_stderr_writer()))
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(indicatif_layer)
        .init();

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

    if let Some(threads) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .unwrap();
    }

    let files: Vec<PathBuf> = find_files(&cli)?;

    info!("Processing {} comic files", files.len());
    let results = process_files(files, &cli)?;

    let _ = results;

    Ok(())
}

#[instrument(skip(cli))]
fn find_files(cli: &Cli) -> anyhow::Result<Vec<PathBuf>> {
    fn valid_file(path: &std::path::Path) -> bool {
        path.extension()
            .map_or(false, |ext| ext == "cbz" || ext == "zip")
    }

    let mut files = Vec::new();
    for path in &cli.input {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).into_iter().flatten() {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if valid_file(&path) {
                        files.push(path);
                    }
                }
            }
        } else {
            if valid_file(&path) {
                files.push(path.clone());
            }
        }
    }

    files.sort_by_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });

    Ok(files)
}

#[instrument(skip(files, cli), fields(num_files = files.len()))]
fn process_files(files: Vec<PathBuf>, cli: &Cli) -> anyhow::Result<Vec<Comic>> {
    let results = files
        .into_par_iter()
        .map(|file| {
            let mut comic = create_comic(
                file.clone(),
                cli.manga_mode,
                cli.quality,
                cli.prefix.as_deref(),
                cli.auto_crop,
            )?;

            let span = comic.span.clone();
            let _guard = span.enter();

            debug!("Extracting CBZ archive");
            comic_archive::extract_cbz(&mut comic)?;

            debug!("Processing images");
            image_processor::process_images(&mut comic)?;

            debug!("Building EPUB");
            epub_builder::build_epub(&comic)?;

            debug!("Creating MOBI for {}", file.display());
            let spawned = mobi_converter::create_mobi(&comic)?;

            anyhow::Ok((comic, spawned))
        })
        .collect::<Vec<_>>();

    let results = results
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_par_iter()
        .map(|(comic, spawned)| {
            let span = comic.span.clone();
            let _guard = span.enter();

            let span = span!(Level::INFO, "wait_for_mobi", file = %comic.input.display());
            let _guard = span.enter();

            let result = spawned.wait();

            result?;

            Ok(comic)
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    Ok(results)
}

fn create_comic(
    file: PathBuf,
    manga_mode: bool,
    quality: u8,
    title_prefix: Option<&str>,
    auto_crop: bool,
) -> anyhow::Result<Comic> {
    let quality = quality.clamp(0, 100);

    let title_prefix = title_prefix
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let title = {
        let file_stem = file.file_stem().unwrap().to_string_lossy();
        match &title_prefix {
            Some(prefix) => format!("{} {}", prefix, file_stem),
            _ => file_stem.to_string(),
        }
    };

    let temp_dir = tempfile::tempdir()?;

    let comic = Comic {
        directory: temp_dir.into_path(),
        input_page_names: Vec::new(),
        processed_files: Vec::new(),

        span: span!(Level::INFO, "process_comic", file = %title),
        title,
        prefix: title_prefix,
        input: file,
        device_dimensions: (1236, 1648),
        right_to_left: manga_mode,
        compression_quality: quality,
        auto_crop,
    };

    Ok(comic)
}

pub struct Comic {
    directory: PathBuf,
    input_page_names: Vec<String>,
    processed_files: Vec<ProcessedImage>,
    span: span::Span,

    // config
    prefix: Option<String>,
    title: String,
    input: PathBuf,
    device_dimensions: (u32, u32),
    right_to_left: bool,
    // number between 0 and 100
    compression_quality: u8,
    auto_crop: bool,
}

impl Drop for Comic {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
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
        if let Some(prefix) = &self.prefix {
            path.set_file_name(format!(
                "{}_{}",
                prefix,
                path.file_stem().unwrap().to_string_lossy()
            ));
        }
        path.set_extension("mobi");
        path
    }
}
