pub mod config;
pub mod processing;

use ratatui::{
    backend::Backend,
    crossterm::event,
    style::{palette, Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
    Terminal,
};
use std::sync::mpsc;

use crate::{poll_kindlegen, process_files, Comic, Event, ProcessingEvent};
use std::thread;

pub const BORDER: Color = palette::tailwind::STONE.c300;
pub const CONTENT: Color = palette::tailwind::STONE.c100;
pub const BACKGROUND: Color = palette::tailwind::STONE.c950;
pub const FOCUSED: Color = palette::tailwind::AMBER.c400;
pub const CONFIG_BUTTON: Color = palette::tailwind::CYAN.c400;
pub const ACTION_BUTTON: Color = palette::tailwind::EMERALD.c400;

pub enum AppState {
    Config(config::ConfigState),
    Processing(processing::ProcessingState),
}

pub fn run(
    terminal: &mut Terminal<impl Backend>,
    event_tx: mpsc::Sender<Event>,
    event_rx: mpsc::Receiver<Event>,
    picker: ratatui_image::picker::Picker,
) -> anyhow::Result<()> {
    let mut state = AppState::Config(config::ConfigState::new(event_tx.clone(), picker)?);
    let mut pending_events = Vec::new();

    'outer: loop {
        // Collect all pending events
        while let Ok(event) = event_rx.try_recv() {
            pending_events.push(event);
        }

        let pending = !pending_events.is_empty();

        // Process events
        if !process_events(terminal, &mut state, &mut pending_events, &event_tx)? {
            break 'outer;
        }

        // Draw if there were pending events
        if pending {
            terminal.draw(|frame| {
                let render_start = std::time::Instant::now();

                match &mut state {
                    AppState::Config(config_state) => {
                        config::ConfigScreen::new(config_state)
                            .render(frame.area(), frame.buffer_mut());
                    }
                    AppState::Processing(processing_state) => {
                        processing::ProcessingScreen::new(processing_state)
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
    state: &mut AppState,
    pending_events: &mut Vec<Event>,
    event_tx: &mpsc::Sender<Event>,
) -> anyhow::Result<bool> {
    for event in pending_events.drain(..) {
        match event {
            Event::Mouse(mouse) => match state {
                AppState::Config(config_state) => {
                    config_state.handle_mouse(mouse);
                }
                AppState::Processing(_) => {}
            },
            Event::Key(key) => {
                if key.code == event::KeyCode::Char('q') {
                    return Ok(false);
                }

                match state {
                    AppState::Config(config_state) => config_state.handle_key(key),
                    AppState::Processing(processing_state) => processing_state.handle_key(key),
                }
            }
            Event::Resize => {
                terminal.autoresize()?;
            }
            Event::Tick => {}
            Event::ProcessingEvent(event) => {
                if let AppState::Processing(processing_state) = state {
                    processing_state.handle_event(event);
                }
            }
            Event::ConfigEvent(event) => {
                if let AppState::Config(config_state) = state {
                    config_state.handle_event(event);
                }
            }
            Event::StartProcessing {
                files,
                config,
                prefix,
            } => {
                // Transition to processing state
                *state = AppState::Processing(processing::ProcessingState::new());

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
                        .send(Event::ProcessingEvent(ProcessingEvent::ProcessingComplete))
                        .unwrap();
                });
            }
        }
    }
    Ok(true)
}

pub fn render_title() -> impl Widget {
    let modifier = Modifier::BOLD | Modifier::ITALIC;
    let styled_title = Line::from(vec![
        Span::styled(
            "c",
            Style::default()
                .fg(palette::tailwind::STONE.c100)
                .add_modifier(modifier),
        ),
        Span::styled(
            "o",
            Style::default()
                .fg(palette::tailwind::STONE.c100)
                .add_modifier(modifier),
        ),
        Span::styled(
            "m",
            Style::default()
                .fg(palette::tailwind::STONE.c200)
                .add_modifier(modifier),
        ),
        Span::styled(
            "i",
            Style::default()
                .fg(palette::tailwind::STONE.c200)
                .add_modifier(modifier),
        ),
        Span::styled(
            "c",
            Style::default()
                .fg(palette::tailwind::STONE.c300)
                .add_modifier(modifier),
        ),
        Span::styled(
            "a",
            Style::default()
                .fg(palette::tailwind::STONE.c300)
                .add_modifier(modifier),
        ),
        Span::styled(
            "l",
            Style::default()
                .fg(palette::tailwind::STONE.c400)
                .add_modifier(modifier),
        ),
        Span::styled(
            "l",
            Style::default()
                .fg(palette::tailwind::STONE.c400)
                .add_modifier(modifier),
        ),
        Span::styled(
            "y",
            Style::default()
                .fg(palette::tailwind::STONE.c500)
                .add_modifier(modifier),
        ),
    ]);

    Paragraph::new(styled_title.centered())
        .block(Block::new().borders(Borders::ALL).border_style(BORDER))
}
