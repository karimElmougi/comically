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
    /// the input files to process. can be a directory or a file.
    /// supports .cbz, .zip, .cbr, .rar files
    #[arg(required = true)]
    input: Vec<PathBuf>,

    /// the prefix to add to the title of the comics + the output file
    #[arg(long, short)]
    prefix: Option<String>,

    /// whether to read the comic from right to left
    #[arg(long, short, default_value = "true", default_missing_value="true", num_args=0..=1)]
    manga: Option<bool>,

    /// the jpg compression quality of the images, between 0 and 100
    #[arg(long, short, default_value_t = 75)]
    quality: u8,

    /// brighten the images
    /// positive values will brighten the images, negative values will darken them
    #[arg(long, short, allow_hyphen_values = true, default_missing_value = "-10")]
    brightness: Option<i32>,

    /// the contrast of the images
    /// positive values will increase the contrast, negative values will decrease it
    #[arg(long, short, allow_hyphen_values = true, default_missing_value = "1.0")]
    contrast: Option<f32>,

    /// the number of threads to use for processing.
    /// defaults to the number of logical CPUs
    #[arg(short)]
    threads: Option<usize>,

    /// crop the dead space on each page
    #[arg(short, long, default_value = "true", default_missing_value = "true")]
    crop: Option<bool>,

    /// split double pages into two separate pages
    #[arg(short, long, default_value = "true", default_missing_value = "true")]
    split: Option<bool>,
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

    let (event_tx, event_rx) = mpsc::channel();
    let (kindlegen_tx, kindlegen_rx) = mpsc::channel();

    thread::spawn({
        let event_tx = event_tx.clone();
        move || input_handling(event_tx)
    });

    thread::spawn({
        let event_tx = event_tx.clone();
        move || {
            process_files(files, &cli, event_tx, kindlegen_tx);
        }
    });

    // polling thread
    thread::spawn({
        let event_tx = event_tx.clone();
        move || {
            poll_kindlegen(kindlegen_rx);
            // once the worker thread has completed, poll kindlegen will stop looping
            // then send the ProcessingComplete event
            event_tx.send(Event::ProcessingComplete).unwrap();
        }
    });

    let mut terminal = ratatui::init_with_options(ratatui::TerminalOptions {
        viewport: Viewport::Fullscreen,
    });
    std::io::stderr().execute(ratatui::crossterm::terminal::EnterAlternateScreen)?;

    let result = tui::run(&mut terminal, event_rx);

    ratatui::restore();

    result
}

fn find_files(cli: &Cli) -> anyhow::Result<Vec<PathBuf>> {
    fn valid_file(path: &std::path::Path) -> bool {
        path.extension().map_or(false, |ext| {
            ext == "cbz" || ext == "zip" || ext == "cbr" || ext == "rar"
        })
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
    kindlegen_tx: mpsc::Sender<Comic>,
) {
    let config = ComicConfig {
        device_dimensions: (1236, 1648),
        // device_dimensions: (600, 800),
        right_to_left: cli.manga.unwrap_or(true),
        split_double_page: cli.split.unwrap_or(true),
        compression_quality: cli.quality,
        auto_crop: cli.crop.unwrap_or(true),
        brightness: cli.brightness,
        contrast: cli.contrast,
    };

    log::info!("processing with config: {:?}", config);
    log::info!("processing {} files", files.len());

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
                id,
                file.clone(),
                cli.prefix.as_deref(),
                title,
                config,
                event_tx.clone(),
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
        .filter_map(|mut comic| {
            let images = comic.with_try(|comic| {
                let start = comic.update_status(ComicStage::Process, 50.0);
                let files = comic_archive::unarchive_comic_iter(&comic.input)?;
                let images =
                    image_processor::process_archive_images(files, config, comic.processed_dir())?;
                comic.stage_completed(ComicStage::Process, start.elapsed());
                Ok(images)
            })?;

            log::info!("Processed {} images for {}", images.len(), comic.title);

            comic.processed_files = images;

            comic.with_try(|comic| {
                let start = comic.update_status(ComicStage::Epub, 50.0);
                epub_builder::build_epub(comic)?;
                comic.stage_completed(ComicStage::Epub, start.elapsed());
                Ok(())
            })?;
            Some(comic)
        })
        .for_each(|comic| {
            kindlegen_tx.send(comic).unwrap();
        });
}

fn create_comic(
    id: usize,
    file: PathBuf,
    title_prefix: Option<&str>,
    title: String,
    mut config: ComicConfig,
    tx: mpsc::Sender<Event>,
) -> anyhow::Result<Comic> {
    config.compression_quality = config.compression_quality.clamp(0, 100);

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
        processed_dir: temp_dir.path().join("Processed"),
        temp_dir,

        processed_files: Vec::new(),

        title: full_title,
        prefix: title_prefix,
        input: file,
        config,
    };

    std::fs::create_dir_all(comic.processed_dir())?;

    Ok(comic)
}

pub struct Comic {
    id: usize,
    tx: mpsc::Sender<Event>,
    temp_dir: tempfile::TempDir,
    processed_dir: PathBuf,
    processed_files: Vec<ProcessedImage>,

    title: String,
    prefix: Option<String>,
    input: PathBuf,

    config: ComicConfig,
}

#[derive(Debug, Clone, Copy)]
struct ComicConfig {
    device_dimensions: (u32, u32),
    right_to_left: bool,
    split_double_page: bool,
    auto_crop: bool,
    compression_quality: u8,
    brightness: Option<i32>,
    contrast: Option<f32>,
}

impl Drop for Comic {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp_dir);
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
                log::error!("Error in comic: {} {e}", self.title);
                self.failed(e);
                None
            }
        }
    }

    // where processed images are stored
    pub fn processed_dir(&self) -> &std::path::Path {
        &self.processed_dir
    }

    pub fn epub_dir(&self) -> PathBuf {
        self.temp_dir.path().join("EPUB")
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

    fn update_status(&self, stage: ComicStage, progress: f64) -> Instant {
        let start = Instant::now();
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Processing {
                stage,
                progress,
                start,
            },
        });
        start
    }

    fn stage_completed(&self, stage: ComicStage, duration: Duration) {
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::StageCompleted { stage, duration },
        });
    }

    fn success(&self) {
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Success,
        });
    }

    fn failed(&self, error: anyhow::Error) {
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Failed { error },
        });
    }
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

fn poll_kindlegen(tx: mpsc::Receiver<Comic>) {
    struct KindleGenStatus {
        comic: Comic,
        spawned: mobi_converter::SpawnedKindleGen,
        start: Instant,
    }

    let mut pending = Vec::<Option<KindleGenStatus>>::new();

    'outer: loop {
        // get new comics from the channel
        loop {
            let result = tx.try_recv();

            match result {
                Ok(mut comic) => {
                    let result = comic.with_try(|comic| {
                        let start = comic.update_status(ComicStage::Mobi, 75.0);
                        let spawned = mobi_converter::create_mobi(comic)?;
                        Ok((spawned, start))
                    });
                    if let Some((spawned, start)) = result {
                        pending.push(Some(KindleGenStatus {
                            comic,
                            spawned,
                            start,
                        }));
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if pending.is_empty() {
                        break 'outer;
                    } else {
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    break;
                }
            }
        }

        // check for completed processes
        for s in pending.iter_mut() {
            let is_done = match s {
                Some(status) => match status.spawned.try_wait() {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(e) => {
                        log::error!("error waiting for kindlegen: {}", e);
                        true
                    }
                },
                _ => false,
            };

            if is_done {
                if let Some(mut status) = s.take() {
                    let _ = status.comic.with_try(|comic| {
                        status.spawned.wait()?;
                        comic.stage_completed(ComicStage::Mobi, status.start.elapsed());
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
    // initial state
    Waiting,

    // currently processing a specific stage
    Processing {
        stage: ComicStage,
        progress: f64,
        start: Instant,
    },

    // stage completed
    StageCompleted {
        stage: ComicStage,
        duration: Duration,
    },

    // final states
    Success,
    Failed {
        error: anyhow::Error,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum ComicStage {
    Extract,
    Process,
    Epub,
    Mobi,
}

impl std::fmt::Display for ComicStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComicStage::Extract => write!(f, "extract"),
            ComicStage::Process => write!(f, "process"),
            ComicStage::Epub => write!(f, "epub"),
            ComicStage::Mobi => write!(f, "mobi"),
        }
    }
}
