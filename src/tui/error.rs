use std::path::Path;

use ratatui::{
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::tui::{render_title, Theme};

#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub title: String,
    pub message: Vec<String>,
    pub hint: Option<String>,
}

impl ErrorInfo {
    pub fn no_files(dir: &Path) -> Self {
        let dir_str = dir.display().to_string();
        Self {
            title: "no files found".to_string(),
            message: vec![
                "no comic/manga files found".to_string(),
                "".to_string(),
                format!("directory: {}", dir_str),
            ],
            hint: Some("supports .cbz .cbr .zip .rar".to_string()),
        }
    }

    pub fn directory_error(dir: &Path, error: &str) -> Self {
        let dir_str = dir.display().to_string();
        Self {
            title: "can't read dir".to_string(),
            message: vec![
                "failed to read directory".to_string(),
                "".to_string(),
                format!("directory: {}", dir_str),
                "".to_string(),
                format!("error: {}", error),
            ],
            hint: Some("check that the directory exists".to_string()),
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

    let [header_area, main_area, footer_area] = Layout::vertical([
        Constraint::Length(3), // Header
        Constraint::Min(0),    // Main content
        Constraint::Length(3), // Footer
    ])
    .areas(area);

    render_title(theme).render(header_area, buf);

    // Create a centered box for the error message
    let message_block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.error_fg)
        .bg(theme.error_bg)
        .title(error_info.title.as_str())
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
                Line::from(line.as_str()).style(
                    Style::default()
                        .fg(theme.content)
                        .add_modifier(Modifier::BOLD),
                ),
            );
        } else {
            message_lines.push(Line::from(line.as_str()).style(Style::default().fg(theme.content)));
        }
    }

    // Add hint if present
    if let Some(hint) = &error_info.hint {
        message_lines.push(Line::from(""));
        message_lines
            .push(Line::from(hint.as_str()).style(Style::default().fg(theme.content).italic()));
    }

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
