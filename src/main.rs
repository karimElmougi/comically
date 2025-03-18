mod comic_archive;
mod epub_builder;
mod image_processor;
mod mobi_converter;

use clap::Parser;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use std::{env, path::PathBuf, time::Duration};

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

    /// the quality of the images, between 0 and 100
    #[arg(long, short, default_value_t = 50)]
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

    if let Some(threads) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .unwrap();
    }

    let files: Vec<PathBuf> = find_files(&cli)?;

    log::info!("Processing {} files", files.len());

    let (results, duration) = timer(|| process_files(files, &cli));
    let mut results = results?;

    results.sort_by_key(|(c, _)| c.title.clone());

    let mut summary = String::new();
    for (comic, duration) in &results {
        summary.push_str(&format!(
            "✓ {} ({})\n",
            comic.input.display(),
            display_duration(*duration)
        ));
    }

    let successful_count = results.iter().count();
    let failed_count = results.len() - successful_count;
    summary.push_str(&format!(
        "\nProcessed {} file(s) ({} succeeded, {} failed) in {}",
        results.len(),
        successful_count,
        failed_count,
        display_duration(duration)
    ));

    log::info!("\n{}", summary);

    Ok(())
}

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

    Ok(files)
}

fn process_files(files: Vec<PathBuf>, cli: &Cli) -> anyhow::Result<Vec<(Comic, Duration)>> {
    let results = files
        .into_par_iter()
        .map(|file| {
            let (result, duration) = timer(|| {
                let comic = process_to_epub(
                    file,
                    cli.manga_mode,
                    cli.quality,
                    cli.prefix.as_deref(),
                    cli.auto_crop,
                )?;

                let spawned = mobi_converter::create_mobi(&comic)?;
                anyhow::Ok((comic, spawned))
            });

            let (comic, spawned) = result?;

            anyhow::Ok((comic, spawned, duration))
        })
        .collect::<Vec<_>>();

    let results = results
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_par_iter()
        .map(|(comic, spawned, processing)| {
            let (result, wait_duration) = timer(|| spawned.wait());
            result?;
            Ok((comic, processing + wait_duration))
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    Ok(results)
}

fn process_to_epub(
    file: PathBuf,
    manga_mode: bool,
    quality: u8,
    title_prefix: Option<&str>,
    auto_crop: bool,
) -> anyhow::Result<Comic> {
    log::debug!("Processing {} to EPUB", file.display());
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

    let mut comic = Comic {
        directory: temp_dir.into_path(),
        input_page_names: Vec::new(),
        processed_files: Vec::new(),

        title,
        prefix: title_prefix,
        input: file,
        device_dimensions: (1236, 1648),
        right_to_left: manga_mode,
        compression_quality: quality,
        auto_crop,
    };

    timer_log("Extract CBZ", || comic_archive::extract_cbz(&mut comic))?;
    timer_log("Process Images", || {
        image_processor::process_images(&mut comic)
    })?;
    timer_log("Create EPUB", || epub_builder::build_epub(&comic))?;

    Ok(comic)
}

pub struct Comic {
    directory: PathBuf,
    input_page_names: Vec<String>,
    processed_files: Vec<ProcessedImage>,

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

fn timer_log<F, T>(label: &str, func: F) -> T
where
    F: FnOnce() -> T,
{
    let (result, duration) = timer(func);
    log::debug!("{}: {}ms", label, duration.as_millis());
    result
}

fn timer<F, T>(func: F) -> (T, std::time::Duration)
where
    F: FnOnce() -> T,
{
    let start = std::time::Instant::now();
    let result = func();
    let duration = start.elapsed();
    (result, duration)
}

fn display_duration(duration: std::time::Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}
