use ratatui::{
    backend::Backend,
    buffer::Buffer,
    crossterm::event,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{palette, Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Padding, Paragraph, Widget},
    Frame, Terminal,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{ComicStage, ComicStatus, Event};

struct AppState {
    start: Instant,
    comics: Vec<ComicState>,
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
        comics: Vec::new(),
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
                    if !state.comics.is_empty() {
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
                debug_assert!(state.comics.get(id).is_none(), "comic already registered");

                if id == state.comics.len() {
                    state.comics.push(ComicState {
                        title: file_name,
                        status: vec![ComicStatus::Waiting],
                        timings: StageTimings::new(),
                    });
                } else {
                    state.comics[id] = ComicState {
                        title: file_name,
                        status: vec![ComicStatus::Waiting],
                        timings: StageTimings::new(),
                    };
                }
            }
            Event::ComicUpdate { id, status } => {
                if let Some(state) = state.comics.get_mut(id) {
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

const BORDER: Color = palette::tailwind::STONE.c300;
const CONTENT: Color = palette::tailwind::STONE.c100;

fn draw(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();

    frame
        .buffer_mut()
        .set_style(area, Style::default().bg(palette::tailwind::STONE.c950));

    let vertical = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .margin(1);

    let [header_area, main_area, footer_area] = vertical.areas(area);

    draw_header(frame, state, header_area);
    draw_main_content(frame, state, main_area);
    draw_footer(frame, state, footer_area);
}

fn draw_main_content(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let [names_area, status_area, scrollbar_area] = Layout::horizontal([
        Constraint::Percentage(15),
        Constraint::Percentage(85),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(area);

    let names_block = Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER)
        .title("files")
        .title_alignment(Alignment::Center);

    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER)
        .title(Line::from("status").centered())
        .title(Line::from("total").right_aligned());

    frame.render_widget(names_block.clone(), names_area);
    frame.render_widget(status_block.clone(), status_area);

    if state.comics.is_empty() {
        return;
    }

    let names_inner_area = names_block.inner(names_area);
    let status_inner_area = status_block.inner(status_area);

    let visible_height = names_inner_area.height as usize;

    let max_scroll = state.comics.len().saturating_sub(visible_height);
    if state.scroll_offset > max_scroll {
        state.scroll_offset = max_scroll;
    }

    let visible_items = {
        let end_idx = (state.scroll_offset + visible_height).min(state.comics.len());
        &state.comics[state.scroll_offset..end_idx]
    };

    let names_layout =
        Layout::vertical(vec![Constraint::Length(1); visible_items.len()]).split(names_inner_area);
    let status_layout =
        Layout::vertical(vec![Constraint::Length(1); visible_items.len()]).split(status_inner_area);

    for (i, comic) in visible_items.iter().enumerate() {
        draw_file_title(frame, comic, names_layout[i]);
    }

    for (i, comic) in visible_items.iter().enumerate() {
        draw_file_status(frame, comic, status_layout[i]);
    }

    draw_scrollbar(
        frame,
        state,
        scrollbar_area,
        state.comics.len(),
        visible_height,
    );
}

fn draw_file_title(frame: &mut Frame, comic_state: &ComicState, area: Rect) {
    Paragraph::new(comic_state.title.clone())
        .style(CONTENT)
        .alignment(Alignment::Left)
        .block(Block::default().padding(Padding::horizontal(1)))
        .render(area, frame.buffer_mut());
}

fn draw_file_status(frame: &mut Frame, comic_state: &ComicState, area: Rect) {
    match comic_state.current_status() {
        ComicStatus::Waiting => {
            let gauge = Gauge::default()
                .gauge_style(palette::tailwind::STONE.c500)
                .ratio(0.0)
                .label("waiting");

            frame.render_widget(gauge, area);
        }
        ComicStatus::Processing {
            stage,
            progress,
            start,
        } => {
            let elapsed = start.elapsed();
            let style: Style = stage_color(*stage).into();
            let gauge = Gauge::default()
                .gauge_style(style)
                .ratio(*progress / 100.0)
                .label(format!("{} {:.1}s", stage, elapsed.as_secs_f64()));

            frame.render_widget(gauge, area);
        }
        ComicStatus::StageCompleted { .. } => unreachable!(),
        ComicStatus::Success => {
            frame.render_widget(
                StageTimingBar::new(&comic_state.timings).width(area.width),
                area,
            );
        }
        ComicStatus::Failed { error, .. } => {
            let error = error.to_string();

            let gauge = Gauge::default()
                .gauge_style(palette::tailwind::RED.c500)
                .ratio(1.0)
                .label(error);

            frame.render_widget(gauge, area);
        }
    }
}

fn draw_scrollbar(
    frame: &mut Frame,
    state: &mut AppState,
    area: Rect,
    total_items: usize,
    visible_height: usize,
) {
    let show_scrollbar = total_items > visible_height;
    if show_scrollbar {
        let mut scroll_state = ratatui::widgets::ScrollbarState::default()
            .content_length(total_items.saturating_sub(visible_height))
            .position(state.scroll_offset);

        frame.render_stateful_widget(
            ratatui::widgets::Scrollbar::default()
                .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(Color::White))
                .thumb_style(Style::default().fg(palette::tailwind::STONE.c300)),
            area,
            &mut scroll_state,
        );
    }
}

fn draw_footer(frame: &mut Frame, state: &AppState, area: Rect) {
    let show_scrollbar = !state.comics.is_empty();

    let [controls_area, legend_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);

    let keys = if show_scrollbar {
        "q: quit | ↑/k: up | ↓/j: down"
    } else {
        "q: quit"
    };

    let keys = Paragraph::new(keys)
        .style(CONTENT)
        .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(keys, controls_area);

    if !state.comics.is_empty() {
        draw_stage_legend(frame, legend_area);
    }
}

fn draw_stage_legend(frame: &mut Frame, area: Rect) {
    let stages = [
        ComicStage::Extract,
        ComicStage::Process,
        ComicStage::Epub,
        ComicStage::Mobi,
    ];

    let constraints = vec![Constraint::Length(16); stages.len()];

    let layout = Layout::horizontal(constraints).flex(ratatui::layout::Flex::Start);

    let areas = layout.split(area);

    for (i, &stage) in stages.iter().enumerate() {
        let color = stage_color(stage);

        let [block_area, text_area] =
            Layout::horizontal([Constraint::Length(2), Constraint::Fill(1)]).areas(areas[i]);

        frame
            .buffer_mut()
            .set_style(block_area, Style::default().bg(color));

        Paragraph::new(stage.to_string())
            .style(CONTENT)
            .alignment(Alignment::Left)
            .block(Block::default().padding(Padding::horizontal(1)))
            .render(text_area, frame.buffer_mut());
    }
}

fn draw_header(frame: &mut Frame, state: &mut AppState, header_area: ratatui::layout::Rect) {
    let [title_area, progress] =
        Layout::horizontal([Constraint::Percentage(15), Constraint::Percentage(85)])
            .areas(header_area);

    frame.render_widget(render_title(), title_area);

    let total = state.comics.len();
    let completed = state
        .comics
        .iter()
        .filter(|state| {
            matches!(
                state.current_status(),
                ComicStatus::Success { .. } | ComicStatus::Failed { .. }
            )
        })
        .count();

    let successful = state
        .comics
        .iter()
        .filter(|state| matches!(state.current_status(), ComicStatus::Success { .. }))
        .count();

    let progress_ratio = if total > 0 {
        completed as f64 / total as f64
    } else {
        0.0
    };
    let elapsed = state
        .processing_complete
        .unwrap_or_else(|| state.start.elapsed());

    Gauge::default()
        .gauge_style(Style::default().fg(palette::tailwind::SKY.c200))
        .label(format!(
            "{}/{} ({:.1}s)",
            successful,
            total,
            elapsed.as_secs_f64()
        ))
        .ratio(progress_ratio)
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(BORDER)
                .title("progress")
                .title_alignment(Alignment::Center),
        )
        .render(progress, frame.buffer_mut());
}

fn stage_color(stage: ComicStage) -> Color {
    match stage {
        ComicStage::Extract => palette::tailwind::STONE.c100,
        ComicStage::Process => palette::tailwind::STONE.c300,
        ComicStage::Mobi => palette::tailwind::STONE.c400,
        ComicStage::Epub => palette::tailwind::STONE.c500,
    }
}

fn render_title() -> impl Widget {
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
                    let label = format!("{:.1}s", stage.duration.as_secs_f64());

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
                    .bg(palette::tailwind::GREEN.c700),
            )
            .alignment(ratatui::layout::Alignment::Center)
            .render(total_label_area, buf);
    }
}
