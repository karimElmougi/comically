mod comic_archive;
mod epub_builder;
mod image_processor;
mod mobi_converter;

use clap::Parser;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::iter::{IntoParallelIterator, ParallelBridge, ParallelIterator};
use std::{env, path::PathBuf, time::Instant};

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

    // Overall progress bar - improved styling
    let overall_bar = multi_progress.add(ProgressBar::new((files.len() * NUM_STAGES + 1) as u64));
    let overall_style =
        ProgressStyle::with_template("[{elapsed_precise}] [{bar:40.cyan/blue}] {msg} ({percent}%)")
            .unwrap()
            .progress_chars("█▇▆▅▄▃▂▁ ");
    overall_bar.set_style(overall_style);
    overall_bar.set_message("processing comic files");
    overall_bar.enable_steady_tick(std::time::Duration::from_millis(100));

    let results = process_files(files, &cli, multi_progress.clone(), overall_bar.clone());

    let num_success = results.iter().filter(|result| result.is_ok()).count();

    overall_bar.finish_with_message(format!(
        "All files processed ({}/{})",
        num_success,
        results.len()
    ));

    Ok(())
}

const NUM_STAGES: usize = 5;

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
) -> Vec<anyhow::Result<Comic>> {
    let max_prefix_len = files
        .iter()
        .map(|file| {
            file.file_stem()
                .map_or(0, |stem| stem.to_string_lossy().len())
        })
        .max()
        .unwrap_or(0);

    let style = ProgressStyle::with_template("{prefix} {bar:30.cyan/blue} {msg}")
        .unwrap()
        .progress_chars("##-");

    let comics = files
        .into_iter()
        .map(|file| {
            let bar = multi_progress.add(ProgressBar::new(5));
            bar.set_style(style.clone());

            let file_name = file.file_stem().unwrap_or_default().to_string_lossy();
            bar.set_message(format!("{}", file_name));

            // Pad the prefix to ensure alignment
            let padded_prefix = format!("{:width$}", file_name, width = max_prefix_len);
            bar.set_prefix(padded_prefix);

            let c = create_comic(
                file.clone(),
                cli.manga_mode,
                cli.quality,
                cli.prefix.as_deref(),
                cli.auto_crop,
                bar,
            )?;

            c.set_stage("waiting");

            Ok(c)
        })
        .collect::<Vec<anyhow::Result<Comic>>>();

    let results = comics
        // not using .into_par_iter() to maintain some semblance of ordering
        .into_iter()
        .par_bridge()
        .map(|comic| {
            let mut comic = comic?;

            comic.set_stage("extracting");
            comic_archive::extract_cbz(&mut comic)?;
            overall_bar.inc(1);
            comic.bar.inc(1);

            comic.set_stage("processing");
            image_processor::process_images(&mut comic)?;
            overall_bar.inc(1);
            comic.bar.inc(1);

            comic.set_stage("building epub");
            epub_builder::build_epub(&comic)?;
            overall_bar.inc(1);
            comic.bar.inc(1);

            comic.set_stage("building mobi");
            let spawned = mobi_converter::create_mobi(&comic)?;
            overall_bar.inc(1);
            comic.bar.inc(1);

            anyhow::Ok((comic, spawned))
        })
        .collect::<Vec<_>>();

    let results = results
        .into_par_iter()
        .map(|result| {
            let (comic, spawned) = result?;

            let result = spawned.wait();
            comic.bar.inc(1);
            comic.finish(result.is_ok());

            result?;
            overall_bar.inc(1);

            Ok(comic)
        })
        .collect::<Vec<_>>();

    results
}

fn create_comic(
    file: PathBuf,
    manga_mode: bool,
    quality: u8,
    title_prefix: Option<&str>,
    auto_crop: bool,
    progress_bar: ProgressBar,
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

        start: Instant::now(),
        bar: progress_bar,

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

    start: Instant,
    bar: ProgressBar,

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

    fn set_stage(&self, stage: &str) {
        self.bar.set_message(format!("{}", stage));
    }

    fn finish(&self, success: bool) {
        let elapsed = self.start.elapsed();

        self.bar.finish_with_message(format!(
            "{} in {}",
            success.then(|| "✓").unwrap_or("✗"),
            indicatif::HumanDuration(elapsed)
        ));
    }
}
