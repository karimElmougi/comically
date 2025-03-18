use ratatui::{
    backend::Backend,
    buffer::Buffer,
    crossterm::event,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Gauge, Paragraph, Widget},
    Frame, Terminal,
};
use std::{
    collections::HashMap,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{ComicStatus, Event, StageTiming};

struct AppState {
    start: Instant,
    comic_order: Vec<usize>,
    comic_states: HashMap<usize, ComicState>,
    processing_complete: Option<Duration>,

    scroll_offset: usize,
}

#[derive(Debug)]
struct ComicState {
    title: String,
    status: Vec<ComicStatus>,
}

impl ComicState {
    fn last_status(&self) -> &ComicStatus {
        self.status.last().unwrap()
    }
}

pub fn run(terminal: &mut Terminal<impl Backend>, rx: mpsc::Receiver<Event>) -> anyhow::Result<()> {
    let mut state = AppState {
        start: Instant::now(),
        comic_order: Vec::new(),
        comic_states: HashMap::new(),
        processing_complete: None,

        scroll_offset: 0,
    };

    loop {
        terminal.draw(|frame| draw(frame, &mut state))?;

        match rx.recv()? {
            Event::Input(event) => match event.code {
                event::KeyCode::Char('q') => {
                    break;
                }
                event::KeyCode::Down | event::KeyCode::Char('j') => {
                    if !state.comic_order.is_empty() {
                        state.scroll_offset = state.scroll_offset.saturating_add(1);
                    }
                }
                event::KeyCode::Up | event::KeyCode::Char('k') => {
                    if state.scroll_offset > 0 {
                        state.scroll_offset = state.scroll_offset.saturating_sub(1);
                    }
                }
                _ => {}
            },
            Event::Resize => {
                terminal.autoresize()?;
            }
            Event::Tick => {}
            Event::RegisterComic { id, file_name } => {
                let _ = state.comic_states.insert(
                    id,
                    ComicState {
                        title: file_name,
                        status: vec![ComicStatus::Waiting],
                    },
                );
                state.comic_order.push(id);
            }
            Event::ComicUpdate { id, status } => {
                if let Some(state) = state.comic_states.get_mut(&id) {
                    state.status.push(status);
                } else {
                    panic!("Comic state not found for id: {}", id);
                }
            }
            Event::ProcessingComplete => {
                state.processing_complete = Some(state.start.elapsed());
            }
        };
    }
    Ok(())
}

fn draw(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();

    let vertical = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .margin(1);

    let [header_area, main_area, footer_area] = vertical.areas(area);

    draw_header(frame, state, header_area);

    if state.comic_order.is_empty() {
        return;
    }

    let visible_height = main_area.height as usize;
    let total_comics = state.comic_order.len();

    // Calculate the maximum valid scroll position
    let max_scroll = total_comics.saturating_sub(visible_height);
    // Ensure scroll_offset is within bounds
    if state.scroll_offset > max_scroll {
        state.scroll_offset = max_scroll;
    }

    let visible_items = {
        let end_idx = (state.scroll_offset + visible_height).min(total_comics);
        &state.comic_order[state.scroll_offset..end_idx]
    };

    let constraints = vec![Constraint::Length(1); visible_items.len()];
    let comic_layout = Layout::vertical(constraints).split(main_area);

    for (i, &id) in visible_items.iter().enumerate() {
        let state = &state.comic_states[&id];
        let comic_area = comic_layout[i];

        let horizontal_layout =
            Layout::horizontal([Constraint::Percentage(15), Constraint::Percentage(85)])
                .split(comic_area);

        let title_style = match state.last_status() {
            ComicStatus::Waiting => Style::default().fg(Color::Gray),
            ComicStatus::Processing { .. } => Style::default().fg(Color::Yellow),
            ComicStatus::Success { .. } => Style::default().fg(Color::Green),
            ComicStatus::Failed { .. } => Style::default().fg(Color::Red),
        };

        let title_paragraph = Paragraph::new(state.title.clone())
            .style(title_style)
            .block(Block::default().padding(ratatui::widgets::Padding::horizontal(1)));

        frame.render_widget(title_paragraph, horizontal_layout[0]);

        match state.last_status() {
            ComicStatus::Waiting => {
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(Color::Gray))
                    .ratio(0.0)
                    .label("waiting");

                frame.render_widget(gauge, horizontal_layout[1]);
            }
            ComicStatus::Processing { stage, progress } => {
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(Color::Yellow))
                    .ratio(*progress / 100.0)
                    .label(format!("{} - {:.1}%", stage, progress));

                frame.render_widget(gauge, horizontal_layout[1]);
            }
            ComicStatus::Success { stage_timing } => {
                frame.render_widget(
                    StageTimingBar::new(stage_timing).width(horizontal_layout[1].width),
                    horizontal_layout[1],
                );
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

    if total_comics > visible_height {
        let mut scroll_state = ratatui::widgets::ScrollbarState::default()
            .content_length(max_scroll)
            .position(state.scroll_offset);

        frame.render_stateful_widget(
            ratatui::widgets::Scrollbar::default()
                .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(Color::White))
                .thumb_style(Style::default().fg(Color::Blue)),
            main_area,
            &mut scroll_state,
        );
    }

    let keys_text = "q: Quit | ↑/k: Up | ↓/j: Down";
    let keys = Paragraph::new(keys_text)
        .style(Style::default().fg(Color::White))
        .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(keys, footer_area);
}

fn draw_header(frame: &mut Frame, state: &mut AppState, header_area: ratatui::layout::Rect) {
    let block = Block::new().title(Line::from("comically").centered());
    frame.render_widget(block, frame.area());

    let [progress_area] = Layout::vertical([Constraint::Length(1)]).areas(header_area);

    let total = state.comic_order.len();
    let completed = state
        .comic_states
        .values()
        .filter(|state| {
            matches!(
                state.last_status(),
                ComicStatus::Success { .. } | ComicStatus::Failed { .. }
            )
        })
        .count();

    let successful = state
        .comic_states
        .values()
        .filter(|state| matches!(state.last_status(), ComicStatus::Success { .. }))
        .count();

    let progress = {
        let progress_ratio = if total > 0 {
            completed as f64 / total as f64
        } else {
            0.0
        };
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
}

struct StageTimingBar<'a> {
    timing: &'a StageTiming,
    width: u16,
}

impl<'a> StageTimingBar<'a> {
    fn new(timing: &'a StageTiming) -> Self {
        Self { timing, width: 0 }
    }

    fn width(mut self, width: u16) -> Self {
        self.width = width;
        self
    }
}

impl<'a> Widget for StageTimingBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let colors = [
            Color::Yellow,  // Extract
            Color::Green,   // Process
            Color::Blue,    // EPUB
            Color::Magenta, // MOBI
        ];

        let total = self.timing.total().as_secs_f64();
        if total == 0.0 || self.width == 0 {
            return;
        }

        let horizontal = Layout::horizontal([Constraint::Fill(4), Constraint::Length(15)])
            .flex(ratatui::layout::Flex::Start)
            .split(area);

        let bar_area = horizontal[0];
        let total_label_area = horizontal[1];

        let stage_durations = [
            self.timing.extract.as_secs_f64(),
            self.timing.process.as_secs_f64(),
            self.timing.epub.as_secs_f64(),
            self.timing.mobi.as_secs_f64(),
        ];

        let stages: Vec<_> = stage_durations
            .iter()
            .filter(|duration| **duration > 0.0)
            .collect();

        if !stages.is_empty() {
            // Create Fill constraints proportional to each stage's duration
            let constraints: Vec<Constraint> = stages
                .iter()
                .map(|duration| Constraint::Fill((*duration / total * 100.0).round() as u16))
                .collect();

            let stage_areas = Layout::horizontal(&constraints)
                .flex(ratatui::layout::Flex::Start)
                .split(bar_area);

            for (i, (duration, area)) in stages.iter().zip(stage_areas.iter()).enumerate() {
                let color = colors[i];

                buf.set_style(area.clone(), Style::default().bg(color));

                // Add a label if there's enough space (minimum 10 chars width)
                if area.width >= 10 {
                    let stage_name = match i {
                        0 => "extract",
                        1 => "process",
                        2 => "epub",
                        3 => "mobi",
                        _ => "",
                    };

                    let label = format!("{}: {:.1}s", stage_name, duration);
                    let label_width = label.len() as u16;

                    if label_width <= area.width {
                        // Center the label
                        let x = area.x + (area.width - label_width) / 2;
                        buf.set_string(
                            x,
                            area.y + area.height / 2,
                            label,
                            Style::default().fg(Color::Black).bg(color),
                        );
                    }
                }
            }
        }

        Block::default()
            .style(Style::default().bg(Color::DarkGray))
            .render(total_label_area, buf);

        let total_label = format!("{:.1}s", total);
        buf.set_string(
            total_label_area.x + 2,
            total_label_area.y + total_label_area.height / 2,
            total_label,
            Style::default().fg(Color::White).bg(Color::DarkGray),
        );
    }
}
