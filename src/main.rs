mod comic_archive;
mod epub_builder;
mod image_processor;
mod mobi_converter;
mod tui;

use clap::Parser;
use ratatui::{
    crossterm::{event, ExecutableCommand},
    Viewport,
};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::{
    env,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

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
    let log_path = "comically.log";
    let log_file = std::fs::File::create(log_path)?;
    std::env::set_var(
        "RUST_LOG",
        std::env::var("RUST_LOG")
            .or_else(|_| std::env::var("LOG_ENV".to_string()))
            .unwrap_or_else(|_| format!("{}=info", env!("CARGO_CRATE_NAME"))),
    );

    let file_subscriber = tracing_subscriber::fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_writer(log_file)
        .with_target(false)
        .with_ansi(false)
        .with_filter(tracing_subscriber::filter::EnvFilter::from_default_env());

    tracing_subscriber::registry()
        .with(file_subscriber)
        .with(tracing_error::ErrorLayer::default())
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

    let mut terminal = ratatui::init_with_options(ratatui::TerminalOptions {
        viewport: Viewport::Fullscreen,
    });
    std::io::stderr().execute(ratatui::crossterm::terminal::EnterAlternateScreen)?;

    let (event_tx, event_rx) = mpsc::channel();
    let (kindlegen_tx, kindlegen_rx) = mpsc::channel();

    thread::spawn({
        let event_tx = event_tx.clone();
        move || input_handling(event_tx)
    });

    thread::spawn({
        let event_tx = event_tx.clone();
        let kindlegen_tx = kindlegen_tx.clone();
        move || {
            process_files(files, &cli, event_tx, kindlegen_tx);
        }
    });

    // polling thread
    thread::spawn(move || poll_kindlegen(kindlegen_rx));

    let result = tui::run(&mut terminal, event_rx);

    // Restore terminal
    ratatui::restore();

    result
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
    event_tx: mpsc::Sender<Event>,
    kindlegen_tx: mpsc::Sender<KindleGenStatus>,
) {
    let comics: Vec<_> = files
        .into_iter()
        .enumerate()
        .map(|(id, file)| {
            let title = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            event_tx
                .send(Event::RegisterComic {
                    id,
                    file_name: title.clone(),
                })
                .unwrap();

            // Create comic
            match create_comic(
                file.clone(),
                cli.manga_mode,
                cli.quality,
                cli.prefix.as_deref(),
                cli.auto_crop,
                id,
                event_tx.clone(),
                title.clone(),
            ) {
                Ok(comic) => Some(comic),
                Err(e) => {
                    event_tx
                        .send(Event::ComicUpdate {
                            id,
                            status: ComicStatus::Failed { error: e },
                        })
                        .unwrap();
                    None
                }
            }
        })
        .filter_map(|c| c)
        .collect();

    comics
        .into_iter()
        .par_bridge()
        .flat_map(|mut comic| {
            comic.update_status("extracting archive", 0.0);
            let extract_start = Instant::now();
            comic.with_try(|comic| {
                comic_archive::extract_cbz(comic)?;
                comic.record_extract_time(extract_start.elapsed());
                Ok(())
            })?;

            comic.update_status(
                &format!("processing {} images", comic.input_page_names.len()),
                25.0,
            );
            comic.with_try(|comic| {
                let start = Instant::now();
                image_processor::process_images(comic)?;
                comic.record_process_time(start.elapsed());
                Ok(())
            })?;

            comic.update_status("building epub", 50.0);
            comic.with_try(|comic| {
                let start = Instant::now();
                epub_builder::build_epub(comic)?;
                comic.record_epub_time(start.elapsed());
                Ok(())
            })?;

            comic.update_status("building mobi", 75.0);
            let (spawned, mobi_start) = comic.with_try(|comic| {
                let start = Instant::now();
                let spawned = mobi_converter::create_mobi(comic)?;
                comic.record_mobi_time(start.elapsed());
                Ok((spawned, start))
            })?;

            // instead of blocking the rayon thread, we send this to a polling thread so that it can be polled for completion.
            kindlegen_tx
                .send(KindleGenStatus {
                    comic,
                    spawned,
                    start_time: mobi_start,
                })
                .unwrap();

            Some(())
        })
        .collect::<Vec<_>>();

    event_tx.send(Event::ProcessingComplete).unwrap();
}

fn create_comic(
    file: PathBuf,
    manga_mode: bool,
    quality: u8,
    title_prefix: Option<&str>,
    auto_crop: bool,
    id: usize,
    tx: mpsc::Sender<Event>,
    title: String,
) -> anyhow::Result<Comic> {
    let quality = quality.clamp(0, 100);

    let title_prefix = title_prefix
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let full_title = match &title_prefix {
        Some(prefix) => format!("{} {}", prefix, title),
        _ => title,
    };

    let temp_dir = tempfile::tempdir()?;

    let comic = Comic {
        id,
        tx,
        directory: temp_dir.into_path(),
        input_page_names: Vec::new(),
        processed_files: Vec::new(),
        stage_timing: StageTiming::new(),

        title: full_title,
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
    id: usize,
    tx: mpsc::Sender<Event>,
    directory: PathBuf,
    input_page_names: Vec<String>,
    processed_files: Vec<ProcessedImage>,
    stage_timing: StageTiming,

    // Config
    title: String,
    prefix: Option<String>,
    input: PathBuf,
    device_dimensions: (u32, u32),
    right_to_left: bool,
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
    pub fn with_try<F, T>(&mut self, f: F) -> Option<T>
    where
        F: FnOnce(&mut Comic) -> anyhow::Result<T>,
    {
        let result = f(self);
        match result {
            Ok(t) => Some(t),
            Err(e) => {
                self.failed(e);
                None
            }
        }
    }

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

    // New status update methods
    fn update_status(&self, stage: &str, progress: f64) {
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Processing {
                stage: stage.to_string(),
                progress,
            },
        });
    }

    fn success(&self) {
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Success {
                stage_timing: self.stage_timing.clone(),
            },
        });
    }

    fn failed(&self, error: anyhow::Error) {
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Failed { error },
        });
    }

    fn record_extract_time(&mut self, duration: Duration) {
        self.stage_timing.extract = duration;
    }

    fn record_process_time(&mut self, duration: Duration) {
        self.stage_timing.process = duration;
    }

    fn record_epub_time(&mut self, duration: Duration) {
        self.stage_timing.epub = duration;
    }

    fn record_mobi_time(&mut self, duration: Duration) {
        self.stage_timing.mobi = duration;
    }
}

struct KindleGenStatus {
    comic: Comic,
    spawned: mobi_converter::SpawnedKindleGen,
    start_time: Instant,
}

fn input_handling(tx: mpsc::Sender<Event>) {
    let tick_rate = Duration::from_millis(200);
    let mut last_tick = Instant::now();

    loop {
        // poll for tick rate duration, if no events, send tick event.
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).unwrap() {
            match event::read().unwrap() {
                event::Event::Key(key) => {
                    if tx.send(Event::Input(key)).is_err() {
                        break;
                    }
                }
                event::Event::Resize(_, _) => {
                    if tx.send(Event::Resize).is_err() {
                        break;
                    }
                }
                _ => {}
            };
        }
        if last_tick.elapsed() >= tick_rate {
            if tx.send(Event::Tick).is_err() {
                break;
            }
            last_tick = Instant::now();
        }
    }
}

fn poll_kindlegen(tx: mpsc::Receiver<KindleGenStatus>) {
    let mut pending = Vec::<Option<KindleGenStatus>>::new();
    loop {
        while let Ok(status) = tx.try_recv() {
            pending.push(Some(status));
        }

        for s in pending.iter_mut() {
            let is_done = match s {
                Some(status) => {
                    matches!(status.spawned.try_wait(), Ok(Some(_)))
                }
                _ => false,
            };

            if is_done {
                if let Some(mut status) = s.take() {
                    let _ = status.comic.with_try(|comic| {
                        status.spawned.wait()?;
                        comic.record_mobi_time(status.start_time.elapsed());
                        comic.success();
                        Ok(())
                    });
                }
            }
        }

        pending.retain(|s| s.is_some());

        thread::sleep(Duration::from_millis(100));
    }
}

enum Event {
    Input(event::KeyEvent),
    Tick,
    Resize,
    RegisterComic { id: usize, file_name: String },
    ComicUpdate { id: usize, status: ComicStatus },
    ProcessingComplete,
}

#[derive(Debug)]
enum ComicStatus {
    Waiting,
    Processing { stage: String, progress: f64 },
    Success { stage_timing: StageTiming },
    Failed { error: anyhow::Error },
}

#[derive(Debug, Clone)]
pub struct StageTiming {
    extract: Duration,
    process: Duration,
    epub: Duration,
    mobi: Duration,
}

impl StageTiming {
    fn new() -> Self {
        Self {
            extract: Duration::default(),
            process: Duration::default(),
            epub: Duration::default(),
            mobi: Duration::default(),
        }
    }

    fn total(&self) -> Duration {
        self.extract + self.process + self.epub + self.mobi
    }
}
