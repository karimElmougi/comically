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

    // Configure a MultiProgress that properly refreshes
    let multi_progress = MultiProgress::new();
    multi_progress.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(20));
    multi_progress.set_move_cursor(true);

    // Overall progress bar - simpler styling
    let overall_bar = multi_progress.add(ProgressBar::new(files.len() as u64));
    let overall_style = ProgressStyle::with_template(
        "✨ [Total: {elapsed_precise}] {wide_bar:.yellow/red} {percent}% ({pos}/{len}) {msg}",
    )
    .unwrap()
    .progress_chars("█▓▒░ ");
    overall_bar.set_style(overall_style);
    overall_bar.set_message("converting files");

    let results = process_files(files, &cli, multi_progress.clone(), overall_bar.clone())?;

    let _ = results;

    overall_bar.finish_with_message("All files processed");

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

    files.sort_by_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });

    Ok(files)
}

fn process_files(
    files: Vec<PathBuf>,
    cli: &Cli,
    multi_progress: MultiProgress,
    overall_bar: ProgressBar,
) -> anyhow::Result<Vec<Comic>> {
    let style = ProgressStyle::with_template(
        "[{elapsed_precise}] {bar:30.cyan/blue} {pos:>4}/{len:4} {msg}",
    )
    .unwrap()
    .progress_chars("##-");

    let file_bars: Vec<_> = files
        .iter()
        .map(|file| {
            let bar = multi_progress.add(ProgressBar::new(5));
            bar.set_style(style.clone());
            bar.set_message(format!(
                "{}",
                file.file_stem().unwrap_or_default().to_string_lossy()
            ));
            (file.clone(), bar)
        })
        .collect();

    // Spawn a thread that will tick all active bars every second
    let ticking = Arc::new(AtomicBool::new(true));
    let ticker_thread = std::thread::spawn({
        let overall_bar = overall_bar.clone();
        let ticking = ticking.clone();
        let file_bars = file_bars.clone();
        move || {
            while ticking.load(std::sync::atomic::Ordering::Acquire) {
                // // Tick all active bars
                for (_, bar) in file_bars.iter() {
                    bar.tick();
                }

                overall_bar.tick();

                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
        }
    });

    let results = file_bars
        .into_par_iter()
        .map(|(file, file_bar)| {
            let comic = process_to_epub(
                file.clone(),
                cli.manga_mode,
                cli.quality,
                cli.prefix.as_deref(),
                cli.auto_crop,
                &file_bar,
            )?;

            let spawned = mobi_converter::create_mobi(&comic)?;
            file_bar.inc(1);
            file_bar.set_message(format!("{} converting to MOBI", comic.title));

            anyhow::Ok((comic, spawned, file_bar))
        })
        .collect::<Vec<_>>();

    let results = results
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_par_iter()
        .map(|(comic, spawned, file_bar)| {
            let result = spawned.wait();
            file_bar.finish_with_message(format!(
                "{} {}",
                comic.title,
                result.is_ok().then_some("✓").unwrap_or("✗")
            ));

            result?;

            // Update overall progress
            overall_bar.inc(1);

            Ok(comic)
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    overall_bar.finish_with_message("All files processed");

    ticking.store(false, std::sync::atomic::Ordering::Release);
    ticker_thread.join().unwrap();

    Ok(results)
}

fn process_to_epub(
    file: PathBuf,
    manga_mode: bool,
    quality: u8,
    title_prefix: Option<&str>,
    auto_crop: bool,
    progress_bar: &ProgressBar,
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

    progress_bar.set_message(format!("{} extracting...", comic.title));
    comic_archive::extract_cbz(&mut comic)?;
    progress_bar.inc(1);

    progress_bar.set_message(format!("{} processing images", comic.title));
    image_processor::process_images(&mut comic)?;
    progress_bar.inc(1);

    progress_bar.set_message(format!("{} building EPUB", comic.title));
    epub_builder::build_epub(&comic)?;
    progress_bar.inc(1);

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
    log::info!("{}: {}ms", label, duration.as_millis());
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
