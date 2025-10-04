use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Clear, List, ListItem, ListState, StatefulWidget, Widget},
};

use comically::comic::DevicePreset;
use crate::{
    tui::{
        button::{Button, ButtonVariant},
        config::{ConfigState, ModalState},
        utils::popup_block,
    },
};

pub struct DeviceSelectorState {
    pub list_state: ListState,
    pub selected_index: Option<usize>,
}

impl DeviceSelectorState {
    pub fn new(current_preset: DevicePreset) -> Self {
        let selected_index = DEVICE_PRESETS.iter().position(|preset| {
            preset.name == current_preset.name && preset.dimensions == current_preset.dimensions
        });

        let mut list_state = ListState::default();
        if let Some(idx) = selected_index {
            list_state.select(Some(idx));
        }

        Self {
            list_state,
            selected_index,
        }
    }

    pub fn confirm_selection(&mut self) -> Option<&DevicePreset> {
        if let Some(selected) = self.list_state.selected() {
            self.selected_index = Some(selected);
            Some(&DEVICE_PRESETS[selected])
        } else {
            None
        }
    }

    pub fn select_next(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected < DEVICE_PRESETS.len() - 1 {
                self.list_state.select(Some(selected + 1));
            }
        }
    }

    pub fn select_previous(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected > 0 {
                self.list_state.select(Some(selected - 1));
            }
        }
    }

    // returns device preset if it was selected
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<DevicePreset> {
        match key.code {
            KeyCode::Enter => return self.confirm_selection().cloned(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_previous();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
            }
            _ => {}
        }
        None
    }
}

pub const DEVICE_PRESETS: &[DevicePreset] = &[
    DevicePreset {
        name: Cow::Borrowed("Kindle PW 11"),
        dimensions: (1236, 1648),
    },
    DevicePreset {
        name: Cow::Borrowed("Kindle PW 12"),
        dimensions: (1264, 1680),
    },
    DevicePreset {
        name: Cow::Borrowed("Kindle Oasis"),
        dimensions: (1264, 1680),
    },
    DevicePreset {
        name: Cow::Borrowed("Kindle Scribe"),
        dimensions: (1860, 2480),
    },
    DevicePreset {
        name: Cow::Borrowed("Kindle Basic"),
        dimensions: (600, 800),
    },
    DevicePreset {
        name: Cow::Borrowed("Kindle 11"),
        dimensions: (1072, 1448),
    },
    DevicePreset {
        name: Cow::Borrowed("Kobo Clara HD"),
        dimensions: (1072, 1448),
    },
    DevicePreset {
        name: Cow::Borrowed("Kobo Clara 2E"),
        dimensions: (1072, 1448),
    },
    DevicePreset {
        name: Cow::Borrowed("Kobo Libra 2"),
        dimensions: (1264, 1680),
    },
    DevicePreset {
        name: Cow::Borrowed("Kobo Sage"),
        dimensions: (1440, 1920),
    },
    DevicePreset {
        name: Cow::Borrowed("Kobo Elipsa"),
        dimensions: (1404, 1872),
    },
    DevicePreset {
        name: Cow::Borrowed("reMarkable 2"),
        dimensions: (1404, 1872),
    },
    DevicePreset {
        name: Cow::Borrowed("iPad Mini"),
        dimensions: (1488, 2266),
    },
    DevicePreset {
        name: Cow::Borrowed("iPad 10.9"),
        dimensions: (1640, 2360),
    },
    DevicePreset {
        name: Cow::Borrowed("iPad Pro 11"),
        dimensions: (1668, 2388),
    },
    DevicePreset {
        name: Cow::Borrowed("Onyx Boox Nova"),
        dimensions: (1200, 1600),
    },
    DevicePreset {
        name: Cow::Borrowed("Onyx Boox Note"),
        dimensions: (1404, 1872),
    },
    DevicePreset {
        name: Cow::Borrowed("PocketBook Era"),
        dimensions: (1200, 1600),
    },
];

pub fn render_device_selector_popup(area: Rect, buf: &mut Buffer, state: &mut ConfigState) {
    let popup_width = 50.min(area.width * 3 / 4);
    let popup_height = 20.min(area.height * 3 / 4);

    let popup_x = area.left() + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.top() + (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    Clear.render(popup_area, buf);

    let block = popup_block("select device", &state.theme);

    let inner = block.inner(popup_area);
    block.render(popup_area, buf);

    // Split into list area and button area
    let [list_area, button_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(4)])
        .spacing(1)
        .areas(inner);

    // Render device list
    let current_preset = &state.config.device;
    let items: Vec<ListItem> = DEVICE_PRESETS
        .iter()
        .map(|preset| {
            let checkmark = if preset.name == current_preset.name
                && preset.dimensions == current_preset.dimensions
            {
                " ✓"
            } else {
                "  "
            };
            let content = format!(
                "{:<20} {:>4}x{:<4}{}",
                preset.name, preset.dimensions.0, preset.dimensions.1, checkmark
            );
            ListItem::new(content).style(state.theme.content)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    if let ModalState::DeviceSelector(s) = &mut state.modal_state {
        StatefulWidget::render(list, list_area, buf, &mut s.list_state);
    }

    // Render buttons
    let [confirm_area, cancel_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .spacing(2)
            .areas(button_area);

    Button::new("confirm", state.theme)
        .hint("[enter]")
        .on_click(|| {
            if let ModalState::DeviceSelector(selector_state) = &mut state.modal_state {
                if let Some(preset) = selector_state.confirm_selection() {
                    state.config.device = preset.clone();
                }
            }
            state.modal_state = ModalState::None;
        })
        .mouse_event(state.last_mouse_click)
        .render(confirm_area, buf);

    Button::new("cancel", state.theme)
        .hint("[esc]")
        .on_click(|| {
            state.modal_state = ModalState::None;
        })
        .mouse_event(state.last_mouse_click)
        .variant(ButtonVariant::Secondary)
        .render(cancel_area, buf);
}
