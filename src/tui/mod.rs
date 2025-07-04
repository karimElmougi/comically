pub mod button;
pub mod config;
pub mod error;
pub mod progress;
pub mod splash;
pub mod theme;
pub mod utils;

use anyhow::Context;
use ratatui::{
    backend::Backend,
    crossterm::event,
    style::{palette, Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Terminal,
};
use std::{
    fs::create_dir_all,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use crate::{
    comic::OutputFormat,
    pipeline::process_files,
    tui::{
        config::MangaFile,
        error::ErrorInfo,
        splash::{splash_title, SplashScreen},
    },
    Event,
};

pub use theme::{Theme, ThemeMode};

pub struct App {
    pub state: AppState,
    pub theme: Theme,
}

pub enum AppState {
    Config(config::ConfigState),
    Processing(progress::ProgressState),
}

pub fn run(
    input_dir: Option<PathBuf>,
    output_dir: Option<PathBuf>,

    terminal: &mut Terminal<impl Backend>,
    picker: ratatui_image::picker::Picker,
    theme: Theme,

    event_tx: mpsc::Sender<Event>,
    mut event_rx: mpsc::Receiver<Event>,
) {
    let input_dir =
        input_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let output_dir = output_dir.unwrap_or_else(|| input_dir.join("comically"));

    let files = match init(&input_dir, &output_dir) {
        Ok(files) => files,
        Err(e) => {
            let _ = run_fatal_error(terminal, &mut event_rx, &e, &theme);
            return;
        }
    };

    match show_splash_screen(terminal, &mut event_rx, theme) {
        Ok(true) => {}
        Ok(false) => {
            return;
        }
        Err(e) => {
            let e = ErrorInfo::unknown_error(e);
            let _ = run_fatal_error(terminal, &mut event_rx, &e, &theme);
            return;
        }
    }

    match run_main(
        files,
        output_dir,
        terminal,
        event_tx,
        &mut event_rx,
        picker,
        theme,
    ) {
        Ok(()) => {}
        Err(e) => {
            let _ = run_fatal_error(terminal, &mut event_rx, &e, &theme);
        }
    }
}

// if true continue, else exit
fn show_splash_screen(
    terminal: &mut Terminal<impl Backend>,
    event_rx: &mut mpsc::Receiver<Event>,
    theme: Theme,
) -> anyhow::Result<bool> {
    fn poll(
        event_rx: &mut mpsc::Receiver<Event>,
        terminal: &mut Terminal<impl Backend>,
    ) -> anyhow::Result<Option<bool>> {
        while let Ok(event) = event_rx.try_recv() {
            match event {
                Event::Key(key) => match key.code {
                    event::KeyCode::Char('q') | event::KeyCode::Esc => {
                        return Ok(Some(false));
                    }
                    event::KeyCode::Char(' ') => {
                        return Ok(Some(true));
                    }
                    _ => {}
                },
                Event::Resize(_) => terminal.autoresize()?,
                _ => {}
            }
        }
        Ok(None)
    }

    let mut splash = SplashScreen::new(10, theme.is_dark())?;

    while !splash.is_complete() {
        if let Some(should_continue) = poll(event_rx, terminal)? {
            return Ok(should_continue);
        }

        terminal.draw(|frame| {
            frame.render_widget(&splash, frame.area());
        })?;

        splash.advance();
        std::thread::sleep(Duration::from_millis(100));
    }

    terminal.draw(|frame| {
        frame.render_widget(&splash, frame.area());
        splash_title(frame, theme);
    })?;

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        if let Some(should_continue) = poll(event_rx, terminal)? {
            return Ok(should_continue);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    Ok(true)
}

fn run_fatal_error(
    terminal: &mut Terminal<impl Backend>,
    event_rx: &mut mpsc::Receiver<Event>,
    error_info: &ErrorInfo,
    theme: &Theme,
) -> anyhow::Result<()> {
    while let Ok(event) = event_rx.recv() {
        match event {
            Event::Key(key) => {
                if key.code == event::KeyCode::Char('q') || key.code == event::KeyCode::Esc {
                    return Ok(());
                }
            }
            Event::Resize(_) => {
                terminal.autoresize()?;
            }
            _ => {}
        }

        terminal.draw(|frame| {
            error::render_error_screen(theme, error_info, frame.area(), frame.buffer_mut());
        })?;
    }
    Ok(())
}

fn run_main(
    manga_files: Vec<MangaFile>,
    output_dir: PathBuf,
    terminal: &mut Terminal<impl Backend>,
    event_tx: mpsc::Sender<Event>,
    event_rx: &mut mpsc::Receiver<Event>,
    picker: ratatui_image::picker::Picker,
    theme: Theme,
) -> Result<(), ErrorInfo> {
    let state = config::ConfigState::new(event_tx.clone(), picker, manga_files, theme, output_dir);

    let mut app = App {
        state: AppState::Config(state),
        theme,
    };
    let mut pending_events = Vec::new();

    'outer: loop {
        // Collect all pending events
        while let Ok(event) = event_rx.try_recv() {
            pending_events.push(event);
        }

        let pending = !pending_events.is_empty();

        // Process events
        if !process_events(terminal, &mut app, &mut pending_events, &event_tx)? {
            break 'outer;
        }

        // Draw if there were pending events
        if pending {
            terminal
                .draw(|frame| {
                    let render_start = std::time::Instant::now();

                    match &mut app.state {
                        AppState::Config(config_state) => {
                            config::ConfigScreen::new(config_state)
                                .render(frame.area(), frame.buffer_mut());
                        }
                        AppState::Processing(processing_state) => {
                            progress::ProgressScreen::new(processing_state)
                                .render(frame.area(), frame.buffer_mut());
                        }
                    }

                    let render_time = render_start.elapsed();

                    if render_time > std::time::Duration::from_millis(50) {
                        log::warn!("Render closure took {:?}", render_time,);
                    }
                })
                .map_err(ErrorInfo::unknown_error)?;
        }

        // Wait for next event
        match event_rx.recv() {
            Ok(event) => pending_events.push(event),
            Err(_) => break 'outer,
        }
    }

    Ok(())
}

fn process_events(
    terminal: &mut Terminal<impl Backend>,
    app: &mut App,
    pending_events: &mut Vec<Event>,
    event_tx: &mpsc::Sender<Event>,
) -> Result<bool, ErrorInfo> {
    for event in pending_events.drain(..) {
        match event {
            Event::Mouse(mouse) => match &mut app.state {
                AppState::Config(c) => {
                    c.handle_mouse(mouse);
                }
                AppState::Processing(p) => {
                    p.handle_mouse(mouse);
                }
            },
            Event::Key(key) => {
                if key.code == event::KeyCode::Char('q') {
                    return Ok(false);
                }

                if key.code == event::KeyCode::Char('t') {
                    app.theme.toggle();
                    match &mut app.state {
                        AppState::Config(config_state) => {
                            config_state.theme = app.theme;
                        }
                        AppState::Processing(processing_state) => {
                            processing_state.theme = app.theme;
                        }
                    }
                    continue;
                }

                match &mut app.state {
                    AppState::Config(c) => c.handle_key(key),
                    AppState::Processing(p) => p.handle_key(key),
                }
            }
            Event::Resize(picker) => {
                terminal.autoresize().map_err(ErrorInfo::unknown_error)?;
                if let AppState::Config(c) = &mut app.state {
                    if let Some(picker) = picker {
                        c.update_picker(picker);
                    }
                }
            }
            Event::Tick => {}
            Event::Progress(event) => {
                if let AppState::Processing(processing_state) = &mut app.state {
                    processing_state.handle_event(event);
                }
            }
            Event::Config(event) => {
                if let AppState::Config(config_state) = &mut app.state {
                    config_state.handle_event(event);
                }
            }
            Event::StartProcessing {
                files,
                config,
                output_dir,
            } => {
                if config.output_format == OutputFormat::Mobi
                    && !crate::mobi_converter::is_kindlegen_available()
                {
                    return Err(ErrorInfo::error(
                            "KindleGen not installed",
                            "Please install KindleGen and make sure it's in your PATH",
                            Some("Install Kindle Previewer(3) from Amazon\n\nhttps://www.amazon.com/Kindle-Previewer/b?ie=UTF8&node=21381691011".into()),
                        ));
                }

                let _ = config.save();
                app.state = AppState::Processing(progress::ProgressState::new(
                    app.theme,
                    config.output_format,
                ));

                let event_tx = event_tx.clone();
                rayon::spawn(move || {
                    process_files(files, config, output_dir, event_tx);
                });
            }
        }
    }
    Ok(true)
}

fn init(input_dir: &Path, output_dir: &Path) -> Result<Vec<MangaFile>, ErrorInfo> {
    if let Err(e) = create_dir_all(output_dir) {
        return Err(ErrorInfo::error(
            "failed to create output directory",
            format!("directory {}: {e}", output_dir.display()),
            Some("check permissions and disk space".into()),
        ));
    }

    match find_manga_files(input_dir) {
        Ok(files) => {
            if files.is_empty() {
                Err(ErrorInfo::error(
                    "no files found",
                    format!("directory: {}", input_dir.display()),
                    Some("supports .cbz .cbr .zip .rar".into()),
                ))
            } else {
                Ok(files)
            }
        }
        Err(e) => Err(ErrorInfo::error(
            "failed to read directory",
            format!("directory {}: {e}", input_dir.display()),
            Some("check that the directory exists".into()),
        )),
    }
}

fn find_manga_files(dir: &std::path::Path) -> anyhow::Result<Vec<MangaFile>> {
    let mut files = Vec::new();

    for entry in std::fs::read_dir(dir).context("failed to read dir")? {
        let entry = entry.context("failed to read dir entry")?;
        let path = entry.path();

        if let Some(ext) = path.extension() {
            if matches!(
                ext.to_str(),
                Some("cbz") | Some("cbr") | Some("zip") | Some("rar")
            ) {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                files.push(MangaFile {
                    archive_path: path,
                    name,
                });
            }
        }
    }

    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

pub fn render_title(theme: &Theme) -> impl Widget {
    let modifier = Modifier::BOLD | Modifier::ITALIC;

    let (c1, c2, c3, c4, c5) = match theme.mode {
        ThemeMode::Dark => (
            palette::tailwind::SLATE.c300,
            palette::tailwind::SLATE.c400,
            palette::tailwind::CYAN.c600,
            palette::tailwind::CYAN.c500,
            palette::tailwind::CYAN.c400,
        ),
        ThemeMode::Light => (
            Color::Rgb(131, 148, 150), // Solarized base0
            Color::Rgb(101, 123, 131), // Solarized base00
            Color::Rgb(88, 110, 117),  // Solarized base01
            Color::Rgb(38, 139, 210),  // Solarized blue
            Color::Rgb(42, 161, 152),  // Solarized cyan
        ),
    };

    let styled_title = Line::from(vec![
        Span::styled("C", Style::default().fg(c1).add_modifier(modifier)),
        Span::styled("O", Style::default().fg(c1).add_modifier(modifier)),
        Span::styled("M", Style::default().fg(c2).add_modifier(modifier)),
        Span::styled("I", Style::default().fg(c2).add_modifier(modifier)),
        Span::styled("C", Style::default().fg(c3).add_modifier(modifier)),
        Span::styled("A", Style::default().fg(c3).add_modifier(modifier)),
        Span::styled("L", Style::default().fg(c4).add_modifier(modifier)),
        Span::styled("L", Style::default().fg(c4).add_modifier(modifier)),
        Span::styled("Y", Style::default().fg(c5).add_modifier(modifier)),
    ]);

    Paragraph::new(styled_title.centered()).block(utils::themed_block(None, theme))
}
