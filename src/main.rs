mod comic_archive;
mod epub_builder;
mod image_processor;
mod mobi_converter;

use clap::Parser;
use ratatui::{
    backend::Backend,
    crossterm::{event, ExecutableCommand},
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Gauge, Paragraph},
    Frame, Terminal, Viewport,
};
use rayon::iter::{IntoParallelIterator, ParallelBridge, ParallelIterator};
use std::{
    collections::HashMap,
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

    // Initialize ratatui terminal
    let mut terminal = ratatui::init_with_options(ratatui::TerminalOptions {
        viewport: Viewport::Fullscreen,
    });
    std::io::stderr().execute(ratatui::crossterm::terminal::EnterAlternateScreen)?;

    // Create channels for event communication
    let (tx, rx) = mpsc::channel();

    // Set up input handling
    let input_tx = tx.clone();
    thread::spawn(move || input_handling(input_tx));

    // Process files in a separate thread
    let process_tx = tx.clone();
    thread::spawn(move || {
        process_files(files, &cli, process_tx);
    });

    // Run UI loop
    let result = run(&mut terminal, rx);

    // Restore terminal
    ratatui::restore();

    result
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

fn process_files(files: Vec<PathBuf>, cli: &Cli, tx: mpsc::Sender<Event>) {
    let comics: Vec<_> = files
        .into_iter()
        .enumerate()
        .map(|(id, file)| {
            let title = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            tx.send(Event::RegisterComic {
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
                tx.clone(),
                title.clone(),
            ) {
                Ok(comic) => Some(comic),
                Err(e) => {
                    tx.send(Event::ComicUpdate {
                        id,
                        status: ComicStatus::Failed {
                            duration: Duration::from_secs(0),
                            error: e,
                        },
                    })
                    .unwrap();
                    None
                }
            }
        })
        .filter_map(|c| c)
        .collect();

    let spawns: Vec<_> = comics
        .into_iter()
        .par_bridge()
        .flat_map(|mut comic| {
            // Extract CBZ
            comic.update_status("extracting archive", 0.0);
            match comic_archive::extract_cbz(&mut comic) {
                Ok(_) => {}
                Err(e) => {
                    comic.failed(e);
                    return None;
                }
            }

            // Process images
            comic.update_status(
                &format!("processing {} images", comic.input_page_names.len()),
                25.0,
            );
            match image_processor::process_images(&mut comic) {
                Ok(_) => {}
                Err(e) => {
                    comic.failed(e);
                    return None;
                }
            }

            // Build EPUB
            comic.update_status("building epub", 50.0);
            match epub_builder::build_epub(&comic) {
                Ok(_) => {}
                Err(e) => {
                    comic.failed(e);
                    return None;
                }
            }
            comic.update_status("building epub", 75.0);

            // Build MOBI
            comic.update_status("building mobi", 75.0);
            let spawned = match mobi_converter::create_mobi(&comic) {
                Ok(spawned) => spawned,
                Err(e) => {
                    comic.failed(e);
                    return None;
                }
            };

            Some((comic, spawned))
        })
        .collect();

    spawns.into_par_iter().for_each(|(comic, spawned)| {
        let result = spawned.wait();
        match result {
            Ok(_) => comic.success(),
            Err(e) => comic.failed(e),
        }
    });

    tx.send(Event::ProcessingComplete).unwrap();
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
        start: Instant::now(),
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
    start: Instant,

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
        let elapsed = self.start.elapsed();
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Success { duration: elapsed },
        });
    }

    fn failed(&self, error: anyhow::Error) {
        let elapsed = self.start.elapsed();
        let _ = self.tx.send(Event::ComicUpdate {
            id: self.id,
            status: ComicStatus::Failed {
                duration: elapsed,
                error,
            },
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
    Processing {
        stage: String,
        progress: f64,
    },
    Success {
        duration: Duration,
    },
    Failed {
        duration: Duration,
        error: anyhow::Error,
    },
}

struct AppState {
    start: Instant,
    comic_order: Vec<usize>,
    comic_states: HashMap<usize, ComicState>,
    processing_complete: Option<Duration>,
}

#[derive(Debug)]
struct ComicState {
    title: String,
    status: ComicStatus,
}

fn run(terminal: &mut Terminal<impl Backend>, rx: mpsc::Receiver<Event>) -> anyhow::Result<()> {
    let mut app_state = AppState {
        start: Instant::now(),
        comic_order: Vec::new(),
        comic_states: HashMap::new(),
        processing_complete: None,
    };

    let mut redraw = true;

    loop {
        if redraw {
            terminal.draw(|frame| draw(frame, &app_state))?;
        }
        redraw = true;

        match rx.recv()? {
            Event::Input(event) => {
                if event.code == event::KeyCode::Char('q') {
                    break;
                }
            }
            Event::Resize => {
                terminal.autoresize()?;
            }
            Event::Tick => {}
            Event::RegisterComic { id, file_name } => {
                let _ = app_state.comic_states.insert(
                    id,
                    ComicState {
                        title: file_name,
                        status: ComicStatus::Waiting,
                    },
                );
                app_state.comic_order.push(id);

                tracing::info!("Registered comic: {:#?}", app_state.comic_states);
            }
            Event::ComicUpdate { id, status } => {
                if let Some(state) = app_state.comic_states.get_mut(&id) {
                    state.status = status;
                } else {
                    panic!("Comic state not found for id: {}", id);
                }
            }
            Event::ProcessingComplete => {
                app_state.processing_complete = Some(app_state.start.elapsed());
                terminal.draw(|frame| draw(frame, &app_state))?;
                redraw = false;
            }
        };
    }
    Ok(())
}

fn draw(frame: &mut Frame, state: &AppState) {
    let area = frame.area();

    let block = Block::new().title(Line::from("comically").centered());
    frame.render_widget(block, area);

    let vertical = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).margin(1);

    let [progress_area, main] = vertical.areas(area);

    // Total progress
    let total = state.comic_order.len();
    let completed = state
        .comic_states
        .values()
        .filter(|state| match state.status {
            ComicStatus::Success { .. } => true,
            ComicStatus::Failed { .. } => true,
            _ => false,
        })
        .count();

    let successful = state
        .comic_states
        .values()
        .filter(|state| matches!(state.status, ComicStatus::Success { .. }))
        .count();

    let progress_ratio = if total > 0 {
        completed as f64 / total as f64
    } else {
        0.0
    };

    let progress = {
        let elapsed = state
            .processing_complete
            .unwrap_or_else(|| state.start.elapsed());
        Gauge::default()
            .gauge_style(Style::default().fg(Color::Blue))
            .label(format!(
                "{}/{} ({:.1}s)",
                successful,
                total,
                elapsed.as_secs_f64()
            ))
            .ratio(progress_ratio)
    };

    frame.render_widget(progress, progress_area);

    // Only continue if we have comics to display
    if state.comic_order.is_empty() {
        return;
    }

    let comic_count = state.comic_order.len();
    let constraints = vec![Constraint::Length(1); comic_count];

    let comic_layout = Layout::vertical(constraints).split(main);

    // Render each comic with its gauge in the order of registration
    for (i, &id) in state.comic_order.iter().enumerate() {
        let state = &state.comic_states[&id];
        let comic_area = comic_layout[i];

        let horizontal_layout =
            Layout::horizontal([Constraint::Percentage(15), Constraint::Percentage(85)])
                .split(comic_area);

        // Render the title with cleaner status indicators
        let title_style = match &state.status {
            ComicStatus::Waiting => Style::default().fg(Color::Gray),
            ComicStatus::Processing { .. } => Style::default().fg(Color::Yellow),
            ComicStatus::Success { .. } => Style::default().fg(Color::Green),
            ComicStatus::Failed { .. } => Style::default().fg(Color::Red),
        };

        let title_paragraph = Paragraph::new(state.title.clone())
            .style(title_style)
            .block(Block::default().padding(ratatui::widgets::Padding::horizontal(1)));

        frame.render_widget(title_paragraph, horizontal_layout[0]);

        match &state.status {
            ComicStatus::Waiting => {
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(Color::Gray))
                    .ratio(0.0)
                    .label("Waiting");

                frame.render_widget(gauge, horizontal_layout[1]);
            }
            ComicStatus::Processing { stage, progress } => {
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(Color::Yellow))
                    .ratio(*progress / 100.0)
                    .label(format!("{} - {:.1}%", stage, progress));

                frame.render_widget(gauge, horizontal_layout[1]);
            }
            ComicStatus::Success { duration } => {
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(Color::Green))
                    .ratio(1.0)
                    .label(format!("success ({:.1}s)", duration.as_secs_f64()));

                frame.render_widget(gauge, horizontal_layout[1]);
            }
            ComicStatus::Failed { error, .. } => {
                let error = error.to_string();

                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(Color::Red))
                    .ratio(1.0)
                    .label(error);

                frame.render_widget(gauge, horizontal_layout[1]);
            }
        }
    }
}
