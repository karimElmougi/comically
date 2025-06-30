pub mod config;
pub mod processing;

use ratatui::{backend::Backend, crossterm::event, widgets::Widget, Terminal};
use std::sync::mpsc;

use crate::{poll_kindlegen, process_files, Comic, Event, ProcessingEvent};
use std::thread;

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

        // Update preview if in config state
        if let AppState::Config(config_state) = &mut state {
            config_state.check_preview_debounce();
        }

        // Draw if there were pending events
        if pending {
            let draw_start = std::time::Instant::now();
            terminal.draw(|frame| match &mut state {
                AppState::Config(config_state) => {
                    config::ConfigScreen::new(config_state)
                        .render(frame.area(), frame.buffer_mut());
                    if draw_start.elapsed() > std::time::Duration::from_millis(100) {
                        log::error!("ConfigScreen render took {:?}", draw_start.elapsed());
                    }
                }
                AppState::Processing(processing_state) => {
                    processing::ProcessingScreen::new(processing_state)
                        .render(frame.area(), frame.buffer_mut());
                }
            })?;
            let elapsed = draw_start.elapsed();
            if elapsed > std::time::Duration::from_millis(100) {
                log::error!("Terminal draw took {:?}", elapsed);
            }
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
