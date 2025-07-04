use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Styled,
    text::Line,
    widgets::{Block, BorderType, Borders},
};

use crate::tui::Theme;

pub fn center(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

pub fn themed_block(title: Option<&str>, theme: &Theme) -> Block<'static> {
    let title = title.map(|t| format!(" {t} "));
    let mut block = Block::default();
    if let Some(title) = title {
        block = block.title(Line::from(title).set_style(theme.content));
    }
    block
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

pub fn padding(area: Rect, constraint: Constraint, side: Side) -> Rect {
    let content = Constraint::Min(0);
    match side {
        Side::Top => {
            let [_, area] = Layout::vertical([constraint, content]).areas(area);
            area
        }
        Side::Bottom => {
            let [area, _] = Layout::vertical([content, constraint]).areas(area);
            area
        }
        Side::Left => {
            let [_, area] = Layout::horizontal([constraint, content]).areas(area);
            area
        }
        Side::Right => {
            let [area, _] = Layout::horizontal([content, constraint]).areas(area);
            area
        }
    }
}
