use std::{borrow::Cow, path::Path};

use ratatui::{
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::tui::{render_title, Theme};

#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub title: Cow<'static, str>,
    pub message: Vec<Cow<'static, str>>,
    pub hint: Option<Cow<'static, str>>,
}

impl ErrorInfo {
    pub fn unknown_error(error: impl Into<anyhow::Error>) -> Self {
        Self {
            title: "unknown error".into(),
            message: vec![error.into().to_string().into()],
            hint: None,
        }
    }
    pub fn no_files(dir: &Path) -> Self {
        let dir_str = dir.display().to_string();
        Self {
            title: "no files found".into(),
            message: vec![
                "no comic/manga files found".into(),
                "".into(),
                format!("directory: {}", dir_str).into(),
            ],
            hint: Some("supports .cbz .cbr .zip .rar".into()),
        }
    }

    pub fn directory_error(dir: &Path, error: &str) -> Self {
        let dir_str = dir.display().to_string();
        Self {
            title: "can't read dir".into(),
            message: vec![
                "failed to read directory".into(),
                "".into(),
                format!("directory: {}", dir_str).into(),
                "".into(),
                format!("error: {}", error).into(),
            ],
            hint: Some("check that the directory exists".into()),
        }
    }

    pub fn output_dir_error(dir: &Path, error: &str) -> Self {
        let dir_str = dir.display().to_string();
        Self {
            title: "fatal error".into(),
            message: vec![
                "failed to create output directory".into(),
                "".into(),
                format!("directory: {}", dir_str).into(),
                "".into(),
                format!("error: {}", error).into(),
            ],
            hint: Some("check permissions and disk space".into()),
        }
    }
}

pub fn render_error_screen(
    theme: &Theme,
    error_info: &ErrorInfo,
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
) {
    use ratatui::layout::{Alignment, Constraint, Direction, Flex, Layout};

    buf.set_style(area, Style::default().bg(theme.background));

    let [header_area, main_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    render_title(theme).render(header_area, buf);

    // Create a centered box for the error message
    let message_block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.error_fg)
        .bg(theme.error_bg)
        .title(error_info.title.as_ref())
        .title(Line::from("[esc/q]").fg(theme.accent).right_aligned())
        .title_alignment(Alignment::Center);

    let content_height = error_info.message.len() + if error_info.hint.is_some() { 3 } else { 1 };
    let content_width = error_info
        .message
        .iter()
        .map(|line| line.len())
        .max()
        .unwrap_or(0);
    let box_height = (content_height as u16 + 4).min(area.height - 8);
    let box_width = (content_width as u16 + 4).min(area.width - 4);

    let [centered_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(box_height)])
        .flex(Flex::Center)
        .areas(main_area);

    let [centered_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(box_width)])
        .flex(Flex::Center)
        .areas(centered_area);

    let inner = message_block.inner(centered_area);
    message_block.render(centered_area, buf);

    // Build message lines
    let mut message_lines = vec![Line::from("")];

    // Add main message lines
    for (i, line) in error_info.message.iter().enumerate() {
        if i == 0 {
            // First line is bold
            message_lines.push(
                Line::from(line.as_ref()).style(
                    Style::default()
                        .fg(theme.content)
                        .add_modifier(Modifier::BOLD),
                ),
            );
        } else {
            message_lines.push(Line::from(line.as_ref()).style(Style::default().fg(theme.content)));
        }
    }

    // Add hint if present
    if let Some(hint) = &error_info.hint {
        message_lines.push(Line::from(""));
        message_lines
            .push(Line::from(hint.as_ref()).style(Style::default().fg(theme.content).italic()));
    }

    let [msg_area] = Layout::vertical([Constraint::Length(message_lines.len() as u16)])
        .flex(Flex::Center)
        .areas(inner);

    let msg = Paragraph::new(message_lines)
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.content));
    msg.render(msg_area, buf);
}
