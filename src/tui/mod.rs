pub mod button;
pub mod config;
pub mod error;
pub mod progress;
pub mod splash;
pub mod theme;
pub mod utils;

use ratatui::{
    backend::Backend,
    crossterm::event,
    style::{palette, Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Terminal,
};
use std::{path::PathBuf, sync::mpsc};

use crate::{
    comic::Comic,
    pipeline::{poll_kindlegen, process_files},
    tui::{config::MangaFile, error::ErrorInfo},
    Event, ProgressEvent,
};
use std::thread;

pub use theme::{Theme, ThemeMode};

pub struct App {
    pub state: AppState,
    pub theme: Theme,
}

pub enum AppState {
    Error(ErrorInfo),
    Config(config::ConfigState),
    Processing(progress::ProgressState),
}

pub fn run(
    directory: Option<PathBuf>,
    terminal: &mut Terminal<impl Backend>,
    event_tx: mpsc::Sender<Event>,
    event_rx: mpsc::Receiver<Event>,
    picker: ratatui_image::picker::Picker,
    theme: Theme,
) -> anyhow::Result<()> {
    let dir =
        directory.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let state = match find_manga_files(&dir) {
        Ok(files) => {
            if files.is_empty() {
                AppState::Error(ErrorInfo::no_files(&dir))
            } else {
                match config::ConfigState::new(event_tx.clone(), picker, files, theme) {
                    Ok(config_state) => AppState::Config(config_state),
                    Err(e) => AppState::Error(ErrorInfo::directory_error(&dir, &e.to_string())),
                }
            }
        }
        Err(e) => AppState::Error(ErrorInfo::directory_error(&dir, &e.to_string())),
    };

    let mut app = App { state, theme };
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
            terminal.draw(|frame| {
                let render_start = std::time::Instant::now();

                match &mut app.state {
                    AppState::Error(error_info) => {
                        error::render_error_screen(
                            &app.theme,
                            error_info,
                            frame.area(),
                            frame.buffer_mut(),
                        );
                    }
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
            })?;
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
) -> anyhow::Result<bool> {
    for event in pending_events.drain(..) {
        match event {
            Event::Mouse(mouse) => match &mut app.state {
                AppState::Error(_) => {}
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
                        AppState::Error(_) => {}
                    }
                    continue;
                }

                match &mut app.state {
                    AppState::Error(_) => {}
                    AppState::Config(c) => c.handle_key(key),
                    AppState::Processing(p) => p.handle_key(key),
                }
            }
            Event::Resize(picker) => {
                terminal.autoresize()?;
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
                prefix,
            } => {
                let _ = config.save();
                app.state = AppState::Processing(progress::ProgressState::new(app.theme));

                let (kindlegen_tx, kindlegen_rx) = mpsc::channel::<Comic>();

                let event_tx_clone = event_tx.clone();
                let kindlegen_tx_clone = kindlegen_tx.clone();
                thread::spawn(move || {
                    process_files(files, config, prefix, event_tx_clone, kindlegen_tx_clone);
                });

                let event_tx_clone = event_tx.clone();
                thread::spawn(move || {
                    poll_kindlegen(kindlegen_rx);
                    event_tx_clone
                        .send(Event::Progress(ProgressEvent::ProcessingComplete))
                        .unwrap();
                });
            }
        }
    }
    Ok(true)
}

fn find_manga_files(dir: &std::path::Path) -> anyhow::Result<Vec<MangaFile>> {
    let mut files = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
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
