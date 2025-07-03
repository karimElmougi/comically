use ratatui::{
    buffer::Buffer,
    crossterm::event::{MouseButton, MouseEvent, MouseEventKind},
    layout::{Alignment, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::Text,
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::tui::Theme;

pub trait CallOnce {
    fn call_once(self);
}

impl<F: FnOnce()> CallOnce for F {
    fn call_once(self) {
        self();
    }
}

pub struct DoNothing;

impl CallOnce for DoNothing {
    fn call_once(self) {}
}

pub struct Button<'a, F = DoNothing> {
    text: Text<'a>,
    theme: &'a Theme,
    state: State,
    enabled: bool,
    variant: ButtonVariant,
    mouse_event: Option<MouseEvent>,

    on_click: Option<F>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum State {
    #[default]
    Normal,
    Pressed,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
}

impl<'a> Button<'a> {
    pub fn new(text: impl Into<Text<'a>>, theme: &'a Theme) -> Button<'a> {
        Button {
            text: text.into(),
            theme,
            state: State::default(),
            enabled: true,
            variant: ButtonVariant::default(),
            mouse_event: None,
            on_click: None,
        }
    }
}

impl<'a, F> Button<'a, F>
where
    F: CallOnce,
{
    pub fn on_click<F2>(self, on_click: F2) -> Button<'a, F2>
    where
        F2: FnOnce(),
    {
        Button {
            text: self.text,
            theme: self.theme,
            state: self.state,
            enabled: self.enabled,
            variant: self.variant,
            mouse_event: self.mouse_event,
            on_click: Some(on_click),
        }
    }

    pub fn mouse_event(mut self, mouse_event: Option<MouseEvent>) -> Self {
        self.mouse_event = mouse_event;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    fn handle_mouse(&mut self, area: Rect) {
        if let Some(mouse) = self.mouse_event {
            if area.contains(Position::new(mouse.column, mouse.row)) {
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        if self.enabled {
                            self.state = State::Pressed;
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        if self.enabled {
                            if let Some(on_click) = self.on_click.take() {
                                on_click.call_once();
                            }
                            self.state = State::Normal;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // fg, bg, top, bottom
    fn get_colors(&self) -> (Color, Color, Color, Color) {
        if !self.enabled {
            return (
                self.theme.content,
                self.theme.muted,
                self.theme.border,
                self.theme.border,
            );
        }
        match (self.state, self.variant) {
            (State::Normal, ButtonVariant::Primary) => (
                self.theme.primary,
                self.theme.primary_bg,
                self.theme.primary,
                self.theme.primary,
            ),
            (State::Pressed, ButtonVariant::Primary) => (
                self.theme.primary,
                self.theme.primary_pressed,
                self.theme.primary,
                self.theme.border,
            ),
            (State::Normal, ButtonVariant::Secondary) => (
                self.theme.secondary,
                self.theme.secondary_bg,
                self.theme.secondary,
                self.theme.secondary,
            ),
            (State::Pressed, ButtonVariant::Secondary) => (
                self.theme.secondary,
                self.theme.secondary_pressed,
                self.theme.secondary,
                self.theme.border,
            ),
        }
    }
}

impl<'a, F: FnOnce() + 'a> Widget for Button<'a, F> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        self.handle_mouse(area);

        let (fg, bg, top, bottom) = self.get_colors();

        buf.set_style(area, Style::default().fg(fg).bg(bg));

        let rows = area.rows().collect::<Vec<_>>();
        let last_index = rows.len().saturating_sub(1);
        let (first, middle, last) = match rows.len() {
            0 | 1 => (None, &rows[..], None),
            2 => (None, &rows[..last_index], Some(rows[last_index])),
            _ => (Some(rows[0]), &rows[1..last_index], Some(rows[last_index])),
        };

        if let Some(first) = first {
            "▔"
                .repeat(area.width as usize)
                .fg(top)
                .bg(bg)
                .render(first, buf);
        }

        if let Some(last) = last {
            "▁"
                .repeat(area.width as usize)
                .fg(bottom)
                .bg(bg)
                .render(last, buf);
        }

        if !middle.is_empty() {
            let block = Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(if self.enabled {
                    top
                } else {
                    self.theme.border
                }));

            let inner = block.inner(middle[0]);
            block.render(middle[0], buf);

            let style = if self.enabled {
                Style::default().fg(fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(fg).add_modifier(Modifier::DIM)
            };

            Paragraph::new(self.text.clone())
                .style(style)
                .alignment(Alignment::Center)
                .render(inner, buf);
        }
    }
}
