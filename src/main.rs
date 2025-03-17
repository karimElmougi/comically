use clap::Parser;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
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
    /// the input files to process. can be a directory or a file
    #[arg(required = true)]
    input: Vec<PathBuf>,

    /// whether to read the comic from right to left
    #[arg(long, short, default_value_t = true)]
    manga_mode: bool,

    /// the quality of the images, between 0 and 100
    #[arg(long, short, default_value_t = 50)]
    quality: u8,
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

    // Collect all files to process
    let files_to_process: Vec<PathBuf> = cli
        .input
        .iter()
        .flat_map(|path| {
            if path.is_dir() {
                std::fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        let path = entry.path();
                        let extension = path.extension().unwrap_or_default();
                        if extension == "cbz" || extension == "zip" {
                            Some(path)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![path.clone()]
            }
        })
        .collect();

    // PHASE 1: Process all files up to EPUB creation in parallel
    let (pending_conversions, phase1_duration) = timer_result(|| {
        files_to_process
            .into_par_iter()
            .map(|file| timer_result(|| process_to_epub(file, cli.manga_mode, cli.quality)))
            .collect::<Vec<_>>()
    });

    log::info!(
        "Phase 1 (CBZ extraction, image processing, EPUB creation) completed in {}",
        display_duration(phase1_duration)
    );

    // Filter out any failed conversions from phase 1
    let successful_comics: Vec<_> = pending_conversions
        .into_iter()
        .filter_map(|(result, duration)| match result {
            Ok(comic) => Some((comic, duration)),
            Err(e) => {
                log::error!("Failed in phase 1: {}", e);
                None
            }
        })
        .collect();

    // PHASE 2: Convert all EPUBs to MOBI in parallel
    let (mut results, phase2_duration) = timer_result(|| {
        successful_comics
            .into_par_iter()
            .map(|(comic, phase1_duration)| {
                let (mobi_result, mobi_duration) =
                    timer_result(|| mobi_converter::create_mobi(&comic));
                match mobi_result {
                    Ok(_) => (Ok(comic), phase1_duration + mobi_duration),
                    Err(e) => (
                        Err(anyhow::anyhow!("MOBI conversion failed: {}", e)),
                        phase1_duration + mobi_duration,
                    ),
                }
            })
            .collect::<Vec<_>>()
    });

    log::info!(
        "Phase 2 (MOBI conversion) completed in {}",
        display_duration(phase2_duration)
    );

    results.sort_by_key(|(result, _)| result.as_ref().map_or(String::new(), |c| c.title.clone()));

    let mut summary = String::new();
    for (result, duration) in &results {
        summary.push_str(&match result {
            Ok(comic) => format!(
                "✓ {} ({})\n",
                comic.input.display(),
                display_duration(*duration)
            ),
            Err(e) => format!("✗ {}\n", e),
        });
    }

    let successful_count = results.iter().filter(|(r, _)| r.is_ok()).count();
    let failed_count = results.len() - successful_count;
    summary.push_str(&format!(
        "\nProcessed {} file(s) ({} succeeded, {} failed) in {}",
        results.len(),
        successful_count,
        failed_count,
        display_duration(phase1_duration + phase2_duration)
    ));

    log::info!("\n{}", summary);

    Ok(())
}

fn process_to_epub(file: PathBuf, manga_mode: bool, quality: u8) -> anyhow::Result<Comic> {
    log::debug!("Processing {} to EPUB", file.display());
    let quality = quality.clamp(0, 100);
    let title = file.file_stem().unwrap().to_string_lossy().to_string();

    let temp_dir = tempfile::tempdir()?;

    let mut comic = Comic {
        title,
        input: file,
        directory: temp_dir.into_path(),
        input_page_names: Vec::new(),
        processed_files: Vec::new(),
        device_dimensions: (1236, 1648),
        right_to_left: manga_mode,
        compression_quality: quality,
    };

    timer_log("Extract CBZ", || comic_archive::extract_cbz(&mut comic))?;
    timer_log("Process Images", || {
        image_processor::process_images(&mut comic)
    })?;
    timer_log("Create EPUB", || epub_builder::build_epub(&comic))?;

    Ok(comic)
}

pub struct Comic {
    title: String,
    directory: PathBuf,
    input_page_names: Vec<String>,
    processed_files: Vec<ProcessedImage>,

    // config
    input: PathBuf,
    device_dimensions: (u32, u32),
    right_to_left: bool,
    // number between 0 and 100
    compression_quality: u8,
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
        path.set_extension("mobi");
        path
    }
}

fn timer_log<F, T>(label: &str, func: F) -> T
where
    F: FnOnce() -> T,
{
    let (result, duration) = timer_result(func);
    log::debug!("{}: {}ms", label, duration.as_millis());
    result
}

fn timer_result<F, T>(func: F) -> (T, std::time::Duration)
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
