pub mod config;
pub mod processing;

use ratatui::{
    backend::Backend,
    buffer::Buffer,
    crossterm::event,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
    Terminal,
};
use std::{sync::mpsc, time::Duration};

use crate::{Event, process_files, poll_kindlegen, Comic};
use std::thread;

pub enum AppState {
    Config(config::ConfigState),
    Processing(processing::ProcessingState),
    Complete,
}

pub fn run(
    terminal: &mut Terminal<impl Backend>,
    _event_rx: mpsc::Receiver<Event>,
) -> anyhow::Result<()> {
    let mut state = AppState::Config(config::ConfigState::new()?);
    let (event_tx, new_event_rx) = mpsc::channel();

    loop {
        // Update preview if in config state
        if let AppState::Config(config_state) = &mut state {
            config_state.update_preview();
        }
        
        terminal.draw(|frame| match &mut state {
            AppState::Config(config_state) => {
                config::ConfigScreen::new(config_state).render(frame.area(), frame.buffer_mut());
            }
            AppState::Processing(processing_state) => {
                processing::ProcessingScreen::new(processing_state)
                    .render(frame.area(), frame.buffer_mut());
            }
            AppState::Complete => {
                render_completion_screen(frame.area(), frame.buffer_mut());
            }
        })?;

        // Handle events
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                event::Event::Key(key) => match &mut state {
                    AppState::Config(config_state) => {
                        match config_state.handle_key(key) {
                            config::ConfigAction::StartProcessing(files, config, prefix) => {
                                // Transition to processing state
                                state = AppState::Processing(processing::ProcessingState::new());
                                
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
                                    event_tx_clone.send(Event::ProcessingComplete).unwrap();
                                });
                            }
                            config::ConfigAction::Quit => return Ok(()),
                            config::ConfigAction::Continue => {}
                        }
                    }
                    AppState::Processing(processing_state) => {
                        if key.code == event::KeyCode::Char('q') {
                            return Ok(());
                        } else if key.code == event::KeyCode::Up || key.code == event::KeyCode::Char('k') {
                            processing_state.handle_scroll(processing::ScrollDirection::Up);
                        } else if key.code == event::KeyCode::Down || key.code == event::KeyCode::Char('j') {
                            processing_state.handle_scroll(processing::ScrollDirection::Down);
                        }
                    }
                    AppState::Complete => {
                        if key.code == event::KeyCode::Char('q') {
                            return Ok(());
                        }
                    }
                },
                _ => {}
            }
        }

        // Handle processing events
        let mut should_complete = false;
        match &mut state {
            AppState::Processing(processing_state) => {
                // Check new event receiver
                while let Ok(event) = new_event_rx.try_recv() {
                    // Check if processing is complete
                    if matches!(event, Event::ProcessingComplete) {
                        should_complete = true;
                    }
                    processing_state.handle_event(event);
                }
            }
            _ => {}
        }
        
        if should_complete {
            state = AppState::Complete;
        }
    }
}

fn render_completion_screen(area: Rect, buf: &mut Buffer) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(area);

    // Title
    let title = Paragraph::new("Processing Complete!")
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::NONE));
    title.render(chunks[0], buf);

    // Success message
    let message = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("✓ ", Style::default().fg(Color::Green)),
            Span::raw("All manga files have been processed successfully!"),
        ]),
        Line::from(""),
        Line::from("The converted .mobi files are saved in the same directory as the source files."),
        Line::from(""),
        Line::from("You can now transfer them to your Kindle device."),
    ];
    
    let content = Paragraph::new(message)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .title("Summary")
                .title_alignment(Alignment::Center),
        );
    content.render(chunks[1], buf);

    // Footer
    let footer = Paragraph::new("Press 'q' to quit")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    footer.render(chunks[2], buf);
}