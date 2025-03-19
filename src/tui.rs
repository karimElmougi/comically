use ratatui::{
    backend::Backend,
    buffer::Buffer,
    crossterm::event,
    layout::{Constraint, Layout, Rect},
    style::{palette, Color, Style},
    text::Line,
    widgets::{Block, Gauge, Paragraph, Widget},
    Frame, Terminal,
};
use std::{
    collections::HashMap,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{ComicStage, ComicStatus, Event};

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
    timings: StageTimings,
}

#[derive(Debug, Clone)]
pub struct StageTimings {
    stages: Vec<StageMetrics>,
}

impl StageTimings {
    pub fn add_stage(&mut self, stage: ComicStage, duration: Duration) {
        self.stages.push(StageMetrics { stage, duration });
    }

    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    pub fn total(&self) -> Duration {
        self.stages.iter().map(|s| s.duration).sum()
    }
}

#[derive(Debug, Clone)]
struct StageMetrics {
    stage: ComicStage,
    duration: Duration,
}

impl ComicState {
    // search for non-completed stage status
    fn current_status(&self) -> &ComicStatus {
        self.status
            .iter()
            .rev()
            .find(|status| !matches!(status, ComicStatus::StageCompleted { .. }))
            .unwrap()
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
                        timings: StageTimings::new(),
                    },
                );
                state.comic_order.push(id);
            }
            Event::ComicUpdate { id, status } => {
                if let Some(state) = state.comic_states.get_mut(&id) {
                    match status {
                        ComicStatus::StageCompleted { stage, duration } => {
                            state.timings.add_stage(stage, duration);
                        }
                        _ => {}
                    }
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

        Paragraph::new(state.title.clone())
            .style(
                Style::default()
                    .fg(palette::tailwind::STONE.c200)
                    .bg(palette::tailwind::STONE.c800),
            )
            .block(Block::default().padding(ratatui::widgets::Padding::horizontal(1)))
            .render(horizontal_layout[0], frame.buffer_mut());

        match state.current_status() {
            ComicStatus::Waiting => {
                let gauge = Gauge::default()
                    .gauge_style(palette::tailwind::GRAY.c400)
                    .ratio(0.0)
                    .label("waiting");

                frame.render_widget(gauge, horizontal_layout[1]);
            }
            ComicStatus::Processing { stage, progress } => {
                let stage_color = stage_color(*stage);
                let gauge = Gauge::default()
                    .gauge_style(stage_color)
                    .ratio(*progress / 100.0)
                    .label(format!("{}", stage));

                frame.render_widget(gauge, horizontal_layout[1]);
            }
            ComicStatus::StageCompleted { .. } => unreachable!(),
            ComicStatus::Success => {
                frame.render_widget(
                    StageTimingBar::new(&state.timings).width(horizontal_layout[1].width),
                    horizontal_layout[1],
                );
            }
            ComicStatus::Failed { error, .. } => {
                let error = error.to_string();

                let gauge = Gauge::default()
                    .gauge_style(palette::tailwind::RED.c500)
                    .ratio(1.0)
                    .label(error);

                frame.render_widget(gauge, horizontal_layout[1]);
            }
        }
    }

    let show_scrollbar = total_comics > visible_height;
    if show_scrollbar {
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

    let keys = if show_scrollbar {
        "q: quit | ↑/k: up | ↓/j: down"
    } else {
        "q: quit"
    };

    let keys = Paragraph::new(keys)
        .style(Style::default().fg(Color::White))
        .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(keys, footer_area);
}

fn stage_color(stage: ComicStage) -> Color {
    match stage {
        ComicStage::Extract => palette::tailwind::STONE.c300,
        ComicStage::Process => palette::tailwind::STONE.c400,
        ComicStage::Epub => palette::tailwind::STONE.c500,
        ComicStage::Mobi => palette::tailwind::STONE.c600,
    }
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
                state.current_status(),
                ComicStatus::Success { .. } | ComicStatus::Failed { .. }
            )
        })
        .count();

    let successful = state
        .comic_states
        .values()
        .filter(|state| matches!(state.current_status(), ComicStatus::Success { .. }))
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
    timing: &'a StageTimings,
    width: u16,
}

impl<'a> StageTimingBar<'a> {
    fn new(timing: &'a StageTimings) -> Self {
        Self { timing, width: 0 }
    }

    fn width(mut self, width: u16) -> Self {
        self.width = width;
        self
    }
}

impl<'a> Widget for StageTimingBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let total = self.timing.total().as_secs_f64();
        if total == 0.0 || self.width == 0 {
            return;
        }

        let horizontal = Layout::horizontal([Constraint::Fill(4), Constraint::Length(15)])
            .flex(ratatui::layout::Flex::Start)
            .split(area);

        let bar_area = horizontal[0];
        let total_label_area = horizontal[1];

        if !self.timing.stages.is_empty() {
            // Create Fill constraints proportional to each stage's duration
            let constraints: Vec<Constraint> = self
                .timing
                .stages
                .iter()
                .map(|stage| {
                    Constraint::Fill((stage.duration.as_secs_f64() / total * 100.0).round() as u16)
                })
                .collect();

            let stage_areas = Layout::horizontal(&constraints)
                .flex(ratatui::layout::Flex::Start)
                .split(bar_area);

            for (stage, area) in self.timing.stages.iter().zip(stage_areas.iter()) {
                let color = stage_color(stage.stage);

                buf.set_style(area.clone(), Style::default().bg(color));

                if area.width >= 10 {
                    let stage_name = stage.stage.to_string();
                    let label = format!("{} {:.1}s", stage_name, stage.duration.as_secs_f64());

                    Paragraph::new(label)
                        .alignment(ratatui::layout::Alignment::Center)
                        .render(area.clone(), buf);
                }
            }
        }

        let total_label = format!("{:.1}s", total);

        Paragraph::new(total_label)
            .style(
                Style::default()
                    .fg(palette::tailwind::GREEN.c100)
                    .bg(palette::tailwind::GREEN.c900),
            )
            .alignment(ratatui::layout::Alignment::Center)
            .render(total_label_area, buf);
    }
}
