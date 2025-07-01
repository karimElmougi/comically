pub mod config;
pub mod progress;
pub mod theme;

use ratatui::{
    backend::Backend,
    crossterm::event,
    style::{palette, Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
    Terminal,
};
use std::sync::mpsc;

use crate::{
    comic::Comic,
    pipeline::{poll_kindlegen, process_files},
    Event, ProgressEvent,
};
use std::thread;

pub use theme::{Theme, ThemeMode};

pub struct App {
    pub state: AppState,
    pub theme: Theme,
}

pub enum AppState {
    NoFiles,
    Config(config::ConfigState),
    Processing(progress::ProgressState),
}

pub fn run(
    terminal: &mut Terminal<impl Backend>,
    event_tx: mpsc::Sender<Event>,
    event_rx: mpsc::Receiver<Event>,
    picker: ratatui_image::picker::Picker,
    theme: Theme,
) -> anyhow::Result<()> {
    // Check if there are any manga files first
    let files = config::find_manga_files(".")?;

    let initial_state = if files.is_empty() {
        AppState::NoFiles
    } else {
        AppState::Config(config::ConfigState::new(event_tx.clone(), picker, files)?)
    };

    let mut app = App {
        state: initial_state,
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
            terminal.draw(|frame| {
                let render_start = std::time::Instant::now();

                match &mut app.state {
                    AppState::NoFiles => {
                        render_no_files_screen(&app.theme, frame.area(), frame.buffer_mut());
                    }
                    AppState::Config(config_state) => {
                        config::ConfigScreen::new(config_state, &app.theme)
                            .render(frame.area(), frame.buffer_mut());
                    }
                    AppState::Processing(processing_state) => {
                        progress::ProgressScreen::new(processing_state, &app.theme)
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
                AppState::NoFiles => {}
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
                    continue;
                }

                match &mut app.state {
                    AppState::NoFiles => {}
                    AppState::Config(c) => c.handle_key(key),
                    AppState::Processing(p) => p.handle_key(key),
                }
            }
            Event::Resize(picker) => {
                terminal.autoresize()?;
                match &mut app.state {
                    AppState::Config(c) => {
                        if let Some(picker) = picker {
                            c.update_picker(picker);
                        }
                    }
                    _ => {}
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
                // Transition to processing state
                app.state = AppState::Processing(progress::ProgressState::new());

                // Create channels for processing
                let (kindlegen_tx, kindlegen_rx) = mpsc::channel::<Comic>();

                // Start processing thread
                let event_tx_clone = event_tx.clone();
                let kindlegen_tx_clone = kindlegen_tx.clone();
                thread::spawn(move || {
                    process_files(files, config, prefix, event_tx_clone, kindlegen_tx_clone);
                });

                // Start kindlegen polling thread
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
        Span::styled("c", Style::default().fg(c1).add_modifier(modifier)),
        Span::styled("o", Style::default().fg(c1).add_modifier(modifier)),
        Span::styled("m", Style::default().fg(c2).add_modifier(modifier)),
        Span::styled("i", Style::default().fg(c2).add_modifier(modifier)),
        Span::styled("c", Style::default().fg(c3).add_modifier(modifier)),
        Span::styled("a", Style::default().fg(c3).add_modifier(modifier)),
        Span::styled("l", Style::default().fg(c4).add_modifier(modifier)),
        Span::styled("l", Style::default().fg(c4).add_modifier(modifier)),
        Span::styled("y", Style::default().fg(c5).add_modifier(modifier)),
    ]);

    Paragraph::new(styled_title.centered()).block(
        Block::new()
            .borders(Borders::ALL)
            .border_style(theme.border),
    )
}

fn render_no_files_screen(
    theme: &Theme,
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
) {
    use ratatui::layout::{Alignment, Constraint, Direction, Flex, Layout};

    buf.set_style(area, Style::default().bg(theme.background));

    let [header_area, main_area, footer_area] = Layout::vertical([
        Constraint::Length(3), // Header
        Constraint::Min(0),    // Main content
        Constraint::Length(3), // Footer
    ])
    .areas(area);

    render_title(theme).render(header_area, buf);

    let message_block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.border)
        .title("no files found")
        .title_alignment(Alignment::Center);

    let [centered_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10)])
        .flex(Flex::Center)
        .areas(main_area);

    let [centered_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(60)])
        .flex(Flex::Center)
        .areas(centered_area);

    let inner = message_block.inner(centered_area);
    message_block.render(centered_area, buf);

    let message_lines = vec![
        Line::from(""),
        Line::from("no comic/manga files found in the current directory").style(
            Style::default()
                .fg(theme.content)
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::from("supported formats:").style(Style::default().fg(theme.content)),
        Line::from(" .cbz .cbr .zip .rar").style(Style::default().fg(theme.primary)),
        Line::from("").style(Style::default().fg(theme.content)),
    ];

    let [msg_area] = Layout::vertical([Constraint::Length(message_lines.len() as u16)])
        .flex(Flex::Center)
        .areas(inner);

    let msg = Paragraph::new(message_lines)
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.content));
    msg.render(msg_area, buf);

    // Footer
    let footer = Paragraph::new("t: toggle theme | q: quit")
        .style(Style::default().fg(theme.content))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border),
        );
    footer.render(footer_area, buf);
}
