use imageproc::image::DynamicImage;
use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::{Alignment, Constraint, Direction, Flex, Layout, Position, Rect},
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
    },
};
use ratatui_image::{
    picker::Picker,
    thread::{ResizeRequest, ResizeResponse, ThreadProtocol},
    FilterType, Resize, ResizeEncodeRender, StatefulImage,
};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use crate::{
    comic::ComicConfig,
    comic_archive::{self, ArchiveFile},
    tui::{
        button::{Button, ButtonVariant},
        Theme,
    },
};

pub struct ConfigState {
    pub files: Vec<MangaFile>,
    pub selected_files: Vec<bool>,
    pub list_state: ListState,
    pub config: ComicConfig,
    pub prefix: Option<String>,
    pub focus: Focus,
    pub selected_field: Option<SelectedField>,
    pub preview_state: PreviewState,
    pub picker: Picker,
    event_tx: std::sync::mpsc::Sender<crate::Event>,
    last_mouse_click: Option<MouseEvent>,
    show_help: bool,
}

#[derive(Debug)]
pub struct MangaFile {
    pub archive_path: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    FileList,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelectedField {
    Quality,
    Brightness,
    Gamma,
}

enum PreviewProtocolState {
    None,
    PendingResize { thread_protocol: ThreadProtocol },
    Ready { thread_protocol: ThreadProtocol },
}

pub struct PreviewState {
    protocol_state: PreviewProtocolState,
    preview_tx: mpsc::Sender<PreviewRequest>,
    resize_tx: mpsc::Sender<ResizeRequest>,
    loaded_image: Option<LoadedPreviewImage>,
}

#[derive(Debug, Clone)]
pub struct LoadedPreviewImage {
    idx: usize,
    total_pages: usize,
    archive_path: PathBuf,
    image_file: ArchiveFile,
    width: u32,
    height: u32,
    config: ComicConfig,
}

enum PreviewRequest {
    LoadFile {
        archive_path: PathBuf,
        config: ComicConfig,
        page_index: Option<usize>,
    },
}

pub enum ConfigEvent {
    ImageLoaded {
        idx: usize,
        total_pages: usize,
        archive_path: PathBuf,
        image: DynamicImage,
        file: ArchiveFile,
        config: ComicConfig,
    },
    ResizeComplete(ResizeResponse),
    Error(String),
}

impl ConfigState {
    pub fn new(
        event_tx: mpsc::Sender<crate::Event>,
        picker: Picker,
        files: Vec<MangaFile>,
    ) -> anyhow::Result<Self> {
        let selected_files = vec![true; files.len()]; // Select all by default

        let mut list_state = ListState::default();
        if !files.is_empty() {
            list_state.select(Some(0));
        }

        // Create channels for preview processing
        let (preview_tx, worker_rx) = mpsc::channel::<PreviewRequest>();
        // Create channel for resize requests
        let (resize_tx, resize_rx) = mpsc::channel::<ResizeRequest>();

        let event_tx_clone = event_tx.clone();
        thread::spawn(move || {
            preview_worker(worker_rx, resize_rx, event_tx_clone);
        });

        let config = ComicConfig::load().unwrap_or_default();

        let mut state = Self {
            files,
            selected_files,
            list_state,
            config,
            prefix: None,
            focus: Focus::FileList,
            selected_field: None,
            preview_state: PreviewState {
                protocol_state: PreviewProtocolState::None,
                preview_tx,
                resize_tx,
                loaded_image: None,
            },
            picker,
            event_tx,
            last_mouse_click: None,
            show_help: false,
        };

        // Auto-load the first image
        state.reload_preview();

        Ok(state)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') => {
                self.show_help = !self.show_help;
            }
            KeyCode::Esc if self.show_help => {
                self.show_help = false;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::FileList => Focus::Settings,
                    Focus::Settings => Focus::FileList,
                };
                self.selected_field = None;
            }
            KeyCode::Enter => {
                self.send_start_processing();
            }

            _ => match self.focus {
                Focus::FileList => self.handle_file_list_keys(key),
                Focus::Settings => self.handle_settings_keys(key),
            },
        }
    }

    fn send_start_processing(&self) {
        let selected_paths: Vec<PathBuf> = self
            .files
            .iter()
            .zip(&self.selected_files)
            .filter(|(_, selected)| **selected)
            .map(|(file, _)| file.archive_path.clone())
            .collect();

        if !selected_paths.is_empty() {
            let _ = self.event_tx.send(crate::Event::StartProcessing {
                files: selected_paths,
                config: self.config,
                prefix: self.prefix.clone(),
            });
        }
    }

    pub fn handle_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        match mouse.kind {
            MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Down(MouseButton::Left) => {
                self.last_mouse_click = Some(mouse);
            }
            MouseEventKind::ScrollUp => {
                self.select_previous();
            }
            MouseEventKind::ScrollDown => {
                self.select_next();
            }
            _ => {}
        }
    }

    fn handle_file_list_keys(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(selected) = self.list_state.selected() {
                    if selected > 0 {
                        self.select_previous();
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(selected) = self.list_state.selected() {
                    if selected < self.files.len() - 1 {
                        self.select_next();
                    }
                }
            }
            KeyCode::Char(' ') => {
                if let Some(selected) = self.list_state.selected() {
                    self.selected_files[selected] = !self.selected_files[selected];
                }
            }
            KeyCode::Char('a') => {
                // Toggle all
                let all_selected = self.selected_files.iter().all(|&s| s);
                for selected in &mut self.selected_files {
                    *selected = !all_selected;
                }
            }
            _ => {}
        }
    }

    // request a preview for the selected file
    fn reload_preview(&mut self) {
        if let Some(file_idx) = self.list_state.selected() {
            if let Some(file) = self.files.get(file_idx) {
                // Reset protocol state when loading a new image
                self.preview_state.protocol_state = PreviewProtocolState::None;

                // if we have a loaded image, use the same index
                let idx = self
                    .preview_state
                    .loaded_image
                    .as_ref()
                    .map(|i| i.idx)
                    .unwrap_or(0);

                let _ = self
                    .preview_state
                    .preview_tx
                    .send(PreviewRequest::LoadFile {
                        archive_path: file.archive_path.clone(),
                        config: self.config,
                        page_index: Some(idx),
                    });
            }
        }
    }

    // request a random page preview for the selected file
    fn request_random_preview_for_selected(&mut self) {
        if let Some(file_idx) = self.list_state.selected() {
            if let Some(file) = self.files.get(file_idx) {
                // Reset protocol state when loading a new image
                self.preview_state.protocol_state = PreviewProtocolState::None;

                let _ = self
                    .preview_state
                    .preview_tx
                    .send(PreviewRequest::LoadFile {
                        archive_path: file.archive_path.clone(),
                        config: self.config,
                        page_index: None,
                    });
            }
        }
    }

    // navigate to next page in preview
    fn next_preview_page(&mut self) {
        if let Some(file_idx) = self.list_state.selected() {
            if let Some(file) = self.files.get(file_idx) {
                // Reset protocol state when loading a new image
                self.preview_state.protocol_state = PreviewProtocolState::None;

                // Get current index and increment
                let current_idx = self
                    .preview_state
                    .loaded_image
                    .as_ref()
                    .map(|i| i.idx)
                    .unwrap_or(0);

                let next_idx = current_idx + 1;

                let _ = self
                    .preview_state
                    .preview_tx
                    .send(PreviewRequest::LoadFile {
                        archive_path: file.archive_path.clone(),
                        config: self.config,
                        page_index: Some(next_idx),
                    });
            }
        }
    }

    // navigate to previous page in preview
    fn previous_preview_page(&mut self) {
        if let Some(file_idx) = self.list_state.selected() {
            if let Some(file) = self.files.get(file_idx) {
                // Reset protocol state when loading a new image
                self.preview_state.protocol_state = PreviewProtocolState::None;

                // Get current index and decrement
                let current_idx = self
                    .preview_state
                    .loaded_image
                    .as_ref()
                    .map(|i| i.idx)
                    .unwrap_or(0);

                let prev_idx = current_idx.saturating_sub(1);

                let _ = self
                    .preview_state
                    .preview_tx
                    .send(PreviewRequest::LoadFile {
                        archive_path: file.archive_path.clone(),
                        config: self.config,
                        page_index: Some(prev_idx),
                    });
            }
        }
    }

    pub fn update_picker(&mut self, new_picker: Picker) {
        self.picker = new_picker;
        self.preview_state.protocol_state = PreviewProtocolState::None;
        if let Some(loaded_image) = self.preview_state.loaded_image.as_ref() {
            let file = self
                .files
                .iter()
                .any(|f| f.archive_path == loaded_image.archive_path);
            if file {
                let _ = self
                    .preview_state
                    .preview_tx
                    .send(PreviewRequest::LoadFile {
                        archive_path: loaded_image.archive_path.clone(),
                        config: self.config,
                        page_index: Some(loaded_image.idx),
                    });
            }
        }
    }

    fn select_previous(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected > 0 {
                self.list_state.select(Some(selected - 1));
            }
        }
    }

    fn select_next(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected < self.files.len() - 1 {
                self.list_state.select(Some(selected + 1));
            }
        }
    }

    fn adjust_setting(&mut self, field: SelectedField, increase: bool, is_fine: bool) {
        match field {
            SelectedField::Quality => {
                let step = if is_fine { 1 } else { 5 };
                self.config.compression_quality = if increase {
                    self.config
                        .compression_quality
                        .saturating_add(step)
                        .min(100)
                } else {
                    self.config.compression_quality.saturating_sub(step)
                };
            }
            SelectedField::Brightness => {
                let step = if is_fine { 1 } else { 5 };
                let current = self.config.brightness;
                self.config.brightness = if increase {
                    (current + step).min(100)
                } else {
                    (current - step).max(-100)
                };
            }
            SelectedField::Gamma => {
                let step = if is_fine { 0.05 } else { 0.1 };
                let current = self.config.gamma;
                self.config.gamma = if increase {
                    (current + step).min(3.0)
                } else {
                    (current - step).max(0.1)
                };
            }
        };
    }

    pub fn handle_event(&mut self, event: ConfigEvent) {
        match event {
            ConfigEvent::ImageLoaded {
                idx,
                total_pages,
                image,
                archive_path,
                file,
                config,
            } => {
                self.preview_state.loaded_image = Some(LoadedPreviewImage {
                    idx,
                    total_pages,
                    archive_path,
                    image_file: file,
                    width: image.width(),
                    height: image.height(),
                    config,
                });
                let protocol = self.picker.new_resize_protocol(image);
                let thread_protocol =
                    ThreadProtocol::new(self.preview_state.resize_tx.clone(), Some(protocol));
                self.preview_state.protocol_state =
                    PreviewProtocolState::PendingResize { thread_protocol };
            }
            ConfigEvent::ResizeComplete(response) => match &mut self.preview_state.protocol_state {
                PreviewProtocolState::PendingResize { thread_protocol } => {
                    if thread_protocol.update_resized_protocol(response) {
                        if let PreviewProtocolState::PendingResize { thread_protocol } =
                            std::mem::replace(
                                &mut self.preview_state.protocol_state,
                                PreviewProtocolState::None,
                            )
                        {
                            self.preview_state.protocol_state =
                                PreviewProtocolState::Ready { thread_protocol };
                        }
                    }
                }
                PreviewProtocolState::Ready { thread_protocol } => {
                    let _ = thread_protocol.update_resized_protocol(response);
                }
                PreviewProtocolState::None => {
                    log::warn!("ResizeComplete received but no protocol exists");
                }
            },
            ConfigEvent::Error(err) => {
                tracing::warn!("Preview error: {}", err);
            }
        }
    }

    fn handle_settings_keys(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('m') => {
                self.config.right_to_left = !self.config.right_to_left;
            }
            KeyCode::Char('s') => {
                self.config.split_double_page = !self.config.split_double_page;
            }
            KeyCode::Char('r') => {
                self.config.auto_crop = !self.config.auto_crop;
            }
            KeyCode::Char('u') => {
                self.selected_field = Some(SelectedField::Quality);
            }
            KeyCode::Char('b') => {
                self.selected_field = Some(SelectedField::Brightness);
            }
            KeyCode::Char('c') => {
                self.selected_field = Some(SelectedField::Gamma);
            }
            KeyCode::Left => {
                if let Some(field) = self.selected_field {
                    let is_fine = key
                        .modifiers
                        .contains(ratatui::crossterm::event::KeyModifiers::SHIFT);
                    self.adjust_setting(field, false, is_fine);
                }
            }
            KeyCode::Right => {
                if let Some(field) = self.selected_field {
                    let is_fine = key
                        .modifiers
                        .contains(ratatui::crossterm::event::KeyModifiers::SHIFT);
                    self.adjust_setting(field, true, is_fine);
                }
            }
            KeyCode::Esc => {
                self.selected_field = None;
            }
            _ => {}
        }
    }
}

pub struct ConfigScreen<'a> {
    state: &'a mut ConfigState,
    theme: &'a Theme,
}

impl<'a> ConfigScreen<'a> {
    pub fn new(state: &'a mut ConfigState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl<'a> Widget for ConfigScreen<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, Style::default().bg(self.theme.background));

        let [header_area, main_area, footer_area] = Layout::vertical([
            Constraint::Length(3), // Header
            Constraint::Min(0),    // Main content
            Constraint::Length(3), // Footer
        ])
        .areas(area);

        super::render_title(self.theme).render(header_area, buf);

        let [file_list_area, settings_area, preview_area] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Fill(2),
            Constraint::Fill(1),
        ])
        .areas(main_area);

        FileListWidget::new(self.state, self.theme).render(file_list_area, buf);

        SettingsWidget::new(self.state, self.theme).render(settings_area, buf);

        PreviewWidget::new(self.state, self.theme).render(preview_area, buf);

        let footer_text = match (self.state.focus, self.state.selected_field) {
            (Focus::FileList, _) => {
                "↑/↓: navigate | space: toggle | a: toggle all | tab: switch panel | h: help | t: theme | q: quit"
            }
            (Focus::Settings, Some(_)) => {
                "←/→: adjust | shift+←/→: fine adjust | esc: cancel | enter: process | h: help | t: theme | q: quit"
            }
            (Focus::Settings, None) => "enter: start | tab: switch | h: help | t: theme | q: quit",
        };
        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(self.theme.content))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(self.theme.border),
            );
        footer.render(footer_area, buf);

        // Render help popup if shown
        if self.state.show_help {
            render_help_popup(area, buf, self.theme);
        }

        // clear the mouse click state
        self.state.last_mouse_click = None;
    }
}

struct FileListWidget<'a> {
    state: &'a mut ConfigState,
    theme: &'a Theme,
}

impl<'a> FileListWidget<'a> {
    fn new(state: &'a mut ConfigState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl<'a> Widget for FileListWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Some(mouse) = self.state.last_mouse_click {
            if area.contains(Position::new(mouse.column, mouse.row)) {
                self.state.selected_field = None;
                self.state.focus = Focus::FileList;
            }
        }

        let items: Vec<ListItem> = self
            .state
            .files
            .iter()
            .zip(&self.state.selected_files)
            .map(|(file, selected)| {
                let checkbox = if *selected { "[✓]" } else { "[ ]" };
                let content = format!("{} {}", checkbox, file.name);
                ListItem::new(content).style(self.theme.content)
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!(
                        "files ({} selected)",
                        self.state.selected_files.iter().filter(|&&s| s).count()
                    ))
                    .borders(Borders::ALL)
                    .border_style(if self.state.focus == Focus::FileList {
                        self.theme.focused
                    } else {
                        self.theme.border
                    }),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");

        StatefulWidget::render(list, area, buf, &mut self.state.list_state);
    }
}

struct SettingsWidget<'a> {
    state: &'a mut ConfigState,
    theme: &'a Theme,
}

impl<'a> SettingsWidget<'a> {
    fn new(state: &'a mut ConfigState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }

    fn render_toggle_button(
        &mut self,
        label: &str,
        value: &str,
        key: &str,
        area: Rect,
        buf: &mut Buffer,
        mut on_click: impl FnMut(&mut ConfigState),
    ) {
        let [label_area, value_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

        let [text_area, key_area] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(key.len() as u16 + 1)])
                .areas(label_area);

        Paragraph::new(label)
            .style(Style::default().fg(self.theme.content))
            .render(text_area, buf);

        Button::new(value, self.theme, self.state.last_mouse_click, || {
            on_click(self.state);
        })
        .render(value_area, buf);

        // Render the key hint
        Paragraph::new(format!(" {}", key))
            .style(Style::default().fg(self.theme.key_hint))
            .render(key_area, buf);
    }

    fn render_adjustable_setting(
        &mut self,
        label: &str,
        value: &str,
        key: &str,
        area: Rect,
        buf: &mut Buffer,
        selected: bool,
        mut on_select: impl FnMut(&mut ConfigState),
        mut on_adjust: impl FnMut(&mut ConfigState, bool),
    ) {
        let style = if selected {
            Style::default().fg(self.theme.content).underlined()
        } else {
            Style::default().fg(self.theme.content)
        };

        let [header_area, buttons_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(3)]).areas(area);

        let [text_area, shortcut_area] = Layout::horizontal([
            Constraint::Length(label.len() as u16 + 1),
            Constraint::Length(key.len() as u16 + 1),
        ])
        .flex(Flex::Start)
        .spacing(1)
        .areas(header_area);

        Paragraph::new(label).style(style).render(text_area, buf);

        Paragraph::new(format!(" {}", key))
            .style(Style::default().fg(self.theme.key_hint))
            .render(shortcut_area, buf);

        let [minus_area, value_area, plus_area] = Layout::horizontal([
            Constraint::Length(5), // [-] button
            Constraint::Length(5), // value
            Constraint::Length(5), // [+] button
        ])
        .spacing(1)
        .areas(buttons_area);

        // Render [-] button
        Button::new("-", self.theme, self.state.last_mouse_click, || {
            on_select(self.state);
            on_adjust(self.state, false);
        })
        .render(minus_area, buf);

        let [value_layout] = Layout::vertical([Constraint::Length(1)])
            .flex(Flex::Center)
            .areas(value_area);

        Paragraph::new(value)
            .style(
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center)
            .render(value_layout, buf);

        // Render [+] button
        Button::new("+", self.theme, self.state.last_mouse_click, || {
            on_select(self.state);
            on_adjust(self.state, true);
        })
        .render(plus_area, buf);
    }

    fn render_dimension_presets(&mut self, area: Rect, buf: &mut Buffer) {
        let [title_area, values_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

        let [label_area, current_dims_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(title_area);

        Paragraph::new("dimensions:")
            .style(Style::default().fg(self.theme.content))
            .render(label_area, buf);

        let current_dims = self.state.config.device_dimensions;
        Paragraph::new(format!("{}x{}", current_dims.0, current_dims.1))
            .style(Style::default().fg(self.theme.primary))
            .render(current_dims_area, buf);

        const LEN: usize = 4;

        let presets: [_; LEN] = [
            ("Kindle PW 11", (1236, 1648)),
            ("Kindle PW 12", (1264, 1680)),
            ("Kindle 12", (1072, 1448)),
            ("Kindle Basic", (800, 600)),
        ];

        let current_dims = self.state.config.device_dimensions;

        let cells = make_grid_layout::<LEN>(
            values_area,
            GridLayout {
                row_length: 2,
                height: Some(Constraint::Length(3)),
                width: None,
                spacing_x: None,
                spacing_y: None,
            },
        );

        for (cell, (name, dims)) in cells.into_iter().zip(presets.iter()) {
            let is_current = *dims == current_dims;

            let button_text = format!("{}\n{}x{}", name, dims.0, dims.1);

            Button::new(button_text, self.theme, self.state.last_mouse_click, || {
                self.state.config.device_dimensions = *dims;
            })
            .variant(if is_current {
                ButtonVariant::Secondary
            } else {
                ButtonVariant::Primary
            })
            .render(cell, buf);
        }
    }
}

impl<'a> Widget for SettingsWidget<'a> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        if let Some(mouse) = self.state.last_mouse_click {
            if area.contains(Position::new(mouse.column, mouse.row)) {
                self.state.focus = Focus::Settings;
            }
        }

        let block = Block::default()
            .title("settings")
            .borders(Borders::ALL)
            .style(Style::default().fg(if self.state.focus == Focus::Settings {
                self.theme.focused
            } else {
                self.theme.border
            }));
        let inner = block.inner(area);
        block.render(area, buf);

        // Create layout for all settings sections
        let constraints = [
            Constraint::Length(1), // top spacer
            Constraint::Min(9),    // Toggles ( reading direction, split double pages, auto crop)
            Constraint::Min(10),   // Buttons (quality, brightness, contrast)
            Constraint::Min(12),   // Dimensions (dynamic grid)
            Constraint::Min(3),    // bottom button
        ];

        let [_, toggles_area, buttons_area, device_presets_area, process_button_area] =
            Layout::vertical(constraints).spacing(1).areas(inner);

        let [reading_direction_area, split_double_pages_area, auto_crop_area] = make_grid_layout::<3>(
            toggles_area,
            GridLayout {
                row_length: 2,
                height: Some(Constraint::Length(4)),
                width: None,
                spacing_x: None,
                spacing_y: None,
            },
        );

        self.render_toggle_button(
            "reading direction",
            if self.state.config.right_to_left {
                "right to left (manga)"
            } else {
                "left to right"
            },
            "[m]",
            reading_direction_area,
            buf,
            |state| {
                state.config.right_to_left = !state.config.right_to_left;
            },
        );

        // Split Double Pages toggle
        self.render_toggle_button(
            "split double pages",
            if self.state.config.split_double_page {
                "yes"
            } else {
                "no"
            },
            "[s]",
            split_double_pages_area,
            buf,
            |state| {
                state.config.split_double_page = !state.config.split_double_page;
            },
        );

        self.render_toggle_button(
            "auto crop",
            if self.state.config.auto_crop {
                "yes"
            } else {
                "no"
            },
            "[c]",
            auto_crop_area,
            buf,
            |state| {
                state.config.auto_crop = !state.config.auto_crop;
            },
        );

        let [quality_area, brightness_area, contrast_area] = make_grid_layout::<3>(
            buttons_area,
            GridLayout {
                row_length: 2,
                height: Some(Constraint::Length(4)),
                width: None,
                spacing_x: None,
                spacing_y: None,
            },
        );

        self.render_adjustable_setting(
            "quality",
            &format!("{:3}", self.state.config.compression_quality),
            "[u]",
            quality_area,
            buf,
            self.state.selected_field == Some(SelectedField::Quality),
            |state| {
                state.selected_field = Some(SelectedField::Quality);
            },
            |state, increase| {
                if let Some(SelectedField::Quality) = state.selected_field {
                    state.adjust_setting(SelectedField::Quality, increase, false);
                }
            },
        );

        self.render_adjustable_setting(
            "contrast",
            &format!("{:3.2}", self.state.config.gamma),
            "[r]",
            contrast_area,
            buf,
            self.state.selected_field == Some(SelectedField::Gamma),
            |state| {
                state.selected_field = Some(SelectedField::Gamma);
            },
            |state, increase| {
                if let Some(SelectedField::Gamma) = state.selected_field {
                    state.adjust_setting(SelectedField::Gamma, increase, false);
                }
            },
        );

        self.render_adjustable_setting(
            "brightness",
            &format!("{:4}", self.state.config.brightness),
            "[b]",
            brightness_area,
            buf,
            self.state.selected_field == Some(SelectedField::Brightness),
            |state| {
                state.selected_field = Some(SelectedField::Brightness);
            },
            |state, increase| {
                if let Some(SelectedField::Brightness) = state.selected_field {
                    state.adjust_setting(SelectedField::Brightness, increase, false);
                }
            },
        );

        self.render_dimension_presets(device_presets_area, buf);

        let [process_button_area] = Layout::default()
            .direction(Direction::Vertical)
            .flex(Flex::End)
            .constraints([Constraint::Length(3)])
            .areas(process_button_area);

        Button::new("start ⏵", self.theme, self.state.last_mouse_click, || {
            self.state.send_start_processing();
        })
        .render(process_button_area, buf);
    }
}

struct PreviewWidget<'a> {
    state: &'a mut ConfigState,
    theme: &'a Theme,
}

impl<'a> PreviewWidget<'a> {
    fn new(state: &'a mut ConfigState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl<'a> Widget for PreviewWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title("preview")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border))
            .style(Style::default());

        let inner = block.inner(area);
        block.render(area, buf);

        // Split the area to have buttons at the bottom
        let [preview_area, buttons_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),    // Preview area
                Constraint::Length(8), // Buttons area (increased for 4 buttons)
            ])
            .areas(inner);

        let config_changed = self
            .state
            .preview_state
            .loaded_image
            .as_ref()
            .map(|loaded| &loaded.config != &self.state.config)
            .unwrap_or(true);

        let file_changed = self
            .state
            .list_state
            .selected()
            .and_then(|idx| self.state.files.get(idx))
            .and_then(|selected_file| {
                self.state
                    .preview_state
                    .loaded_image
                    .as_ref()
                    .map(|loaded| loaded.archive_path != selected_file.archive_path)
            })
            .unwrap_or(true);

        // Split buttons area: 1 button on top, 3 buttons below
        let [top_button_area, bottom_buttons_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Load preview button
                Constraint::Length(3), // Navigation buttons
            ])
            .spacing(1)
            .areas(buttons_area);

        // Load preview button (full width)
        Button::new(
            "load preview",
            self.theme,
            self.state.last_mouse_click,
            || {
                self.state.reload_preview();
            },
        )
        .enabled(config_changed || file_changed)
        .render(top_button_area, buf);

        // Split bottom area into 3 buttons
        let [prev_button_area, random_button_area, next_button_area] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(34),
                Constraint::Percentage(33),
            ])
            .spacing(1)
            .areas(bottom_buttons_area);

        // Previous button
        Button::new("◀ prev", self.theme, self.state.last_mouse_click, || {
            self.state.previous_preview_page();
        })
        .render(prev_button_area, buf);

        // Random button
        Button::new("random", self.theme, self.state.last_mouse_click, || {
            self.state.request_random_preview_for_selected();
        })
        .render(random_button_area, buf);

        // Next button
        Button::new("next ▶", self.theme, self.state.last_mouse_click, || {
            self.state.next_preview_page();
        })
        .render(next_button_area, buf);

        if let Some(loaded_image) = &self.state.preview_state.loaded_image {
            let image = StatefulImage::new().resize(Resize::Scale(Some(FilterType::Lanczos3)));

            let [title_area, image_area] =
                Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(preview_area);

            let file_name = loaded_image
                .archive_path
                .file_stem()
                .unwrap()
                .to_string_lossy();

            let page_info = format!(
                "page {} of {}",
                loaded_image.idx + 1,
                loaded_image.total_pages
            );

            let text = vec![
                Line::from(file_name),
                Line::from(page_info).style(Style::default().fg(self.theme.border)),
            ];

            Paragraph::new(text)
                .style(Style::default().fg(self.theme.content))
                .alignment(Alignment::Center)
                .render(title_area, buf);

            let image_area = calculate_centered_image_area(
                image_area,
                loaded_image,
                self.state.picker.font_size(),
            );

            match &mut self.state.preview_state.protocol_state {
                PreviewProtocolState::None => {
                    render_image_placeholder(image_area, buf, self.theme);
                }
                PreviewProtocolState::PendingResize { thread_protocol } => {
                    if let Some(rect) =
                        thread_protocol.needs_resize(&Resize::Scale(None), image_area)
                    {
                        thread_protocol.resize_encode(&Resize::Scale(None), rect);
                    }
                    render_image_placeholder(image_area, buf, self.theme);
                }
                PreviewProtocolState::Ready { thread_protocol } => {
                    StatefulWidget::render(image, image_area, buf, thread_protocol);
                }
            }
        } else {
        }
    }
}

fn preview_worker(
    rx: mpsc::Receiver<PreviewRequest>,
    resize_rx: mpsc::Receiver<ResizeRequest>,
    tx: mpsc::Sender<crate::Event>,
) {
    // Handle both preview requests and resize requests
    loop {
        if let Some(request) = get_latest(&rx) {
            match request {
                PreviewRequest::LoadFile {
                    archive_path: path,
                    config,
                    page_index,
                } => {
                    let result = load_and_process_preview(&path, &config, page_index);

                    match result {
                        Ok((image, file, idx, total_pages)) => {
                            let _ = tx.send(crate::Event::Config(ConfigEvent::ImageLoaded {
                                idx,
                                total_pages,
                                archive_path: path,
                                image,
                                file,
                                config,
                            }));
                        }
                        Err(e) => {
                            let _ =
                                tx.send(crate::Event::Config(ConfigEvent::Error(e.to_string())));
                        }
                    }
                }
            }
        }

        if let Some(resize_request) = get_latest(&resize_rx) {
            match resize_request.resize_encode() {
                Ok(response) => {
                    let _ = tx.send(crate::Event::Config(ConfigEvent::ResizeComplete(response)));
                }
                Err(e) => {
                    log::warn!("preview_worker: Resize error: {:?}", e);
                }
            }
        }

        thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn load_and_process_preview(
    path: &PathBuf,
    config: &ComicConfig,
    page_index: Option<usize>,
) -> anyhow::Result<(DynamicImage, ArchiveFile, usize, usize)> {
    let mut archive_files: Vec<_> = comic_archive::unarchive_comic_iter(path)?
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();

    // Sort by filename to ensure consistent ordering
    archive_files.sort_by(|a, b| a.file_stem().cmp(b.file_stem()));

    if archive_files.is_empty() {
        return Err(anyhow::anyhow!("No images in archive"));
    }

    let total_pages = archive_files.len();

    let idx = match page_index {
        None => {
            use rand::Rng;
            let random_idx = rand::thread_rng().gen_range(0..archive_files.len());
            random_idx
        }
        Some(idx) => idx.clamp(0, archive_files.len() - 1),
    };

    let archive_file = archive_files.into_iter().nth(idx).unwrap();

    let img = imageproc::image::load_from_memory(&archive_file.data)?;

    let processed_images = crate::image_processor::process_image(img, config);

    let first_image = processed_images
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No processed images"))?;

    let mut compressed_buffer = Vec::new();
    crate::image_processor::compress_to_jpeg(
        &first_image,
        &mut compressed_buffer,
        config.compression_quality,
    )?;

    let compressed_img = imageproc::image::load_from_memory(&compressed_buffer)?;

    Ok((compressed_img, archive_file, idx, total_pages))
}

fn get_latest<T>(rx: &mpsc::Receiver<T>) -> Option<T> {
    let mut latest = None;
    while let Ok(event) = rx.try_recv() {
        latest = Some(event);
    }
    latest
}

fn calculate_centered_image_area(
    area: Rect,
    img: &LoadedPreviewImage,
    font_size: (u16, u16),
) -> Rect {
    // Get terminal cell dimensions from picker (pixels per cell)
    let cell_width_px = font_size.0 as f32;
    let cell_height_px = font_size.1 as f32;

    // Calculate image aspect ratio
    let img_aspect = img.width as f32 / img.height as f32;
    let area_aspect = (area.width as f32 * cell_width_px) / (area.height as f32 * cell_height_px);

    let (target_width_cells, target_height_cells) = if img_aspect > area_aspect {
        // Image is wider - constrain by width
        let width_cells = area.width;
        let height_cells =
            ((width_cells as f32 * cell_width_px) / img_aspect / cell_height_px) as u16;
        (width_cells, height_cells.min(area.height))
    } else {
        // Image is taller - constrain by height
        let height_cells = area.height;
        let width_cells =
            ((height_cells as f32 * cell_height_px) * img_aspect / cell_width_px) as u16;
        (width_cells.min(area.width), height_cells)
    };

    // Center the calculated dimensions
    let [centered_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(target_height_cells)])
        .flex(Flex::Center)
        .areas(area);

    let [final_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(target_width_cells)])
        .flex(Flex::Center)
        .areas(centered_area);

    final_area
}

struct GridLayout<const N: usize> {
    row_length: u16,
    height: Option<Constraint>,
    width: Option<Constraint>,
    spacing_x: Option<u16>,
    spacing_y: Option<u16>,
}

fn make_grid_layout<const N: usize>(area: Rect, layout: GridLayout<N>) -> [Rect; N] {
    let GridLayout {
        row_length,
        height,
        width,
        spacing_x,
        spacing_y,
    } = layout;
    let width = width.unwrap_or(Constraint::Min(0));
    let height = height.unwrap_or(Constraint::Min(0));
    let spacing_x = spacing_x.unwrap_or(1);
    let spacing_y = spacing_y.unwrap_or(1);

    let col_constraints = (0..row_length).map(|_| width);
    let row_constraints =
        (0..((N + row_length as usize - 1) / row_length as usize)).map(|_| height);
    let horizontal = Layout::horizontal(col_constraints).spacing(spacing_x);
    let vertical = Layout::vertical(row_constraints).spacing(spacing_y);

    let rows = vertical.split(area);
    rows.iter()
        .flat_map(move |&row| horizontal.split(row).to_vec())
        .take(N)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap()
}

fn render_image_placeholder(area: Rect, buf: &mut Buffer, theme: &Theme) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_style(Style::default().bg(theme.muted));
                cell.set_symbol(" ");
            }
        }
    }

    let loading_text = "loading...";
    let text_width = loading_text.len() as u16;
    let text_x = area.left() + (area.width.saturating_sub(text_width)) / 2;
    let text_y = area.top() + area.height / 2;

    Paragraph::new(loading_text)
        .style(Style::default().fg(theme.content))
        .render(Rect::new(text_x, text_y, text_width, 1), buf);
}

fn render_help_popup(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let popup_width = (area.width * 4 / 5).min(80);
    let popup_height = (area.height * 9 / 10).min(40);

    let popup_x = area.left() + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.top() + (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    Clear.render(popup_area, buf);

    let help_text = vec![
        "",
        "processing settings",
        "",
        "reading direction (default: right to left):",
        "  • left to right: standard western comic/book reading order",
        "  • right to left: manga reading order - pages flow from right to left",
        "",
        "split double pages (default: yes):",
        "  when enabled, detects and splits two-page spreads into individual pages.",
        "",
        "auto crop (default: yes):",
        "  automatically removes white borders and margins from pages, making the page fit the screen better.",
        "",
        "quality (default: 85, range: 0-100):",
        "  jpeg compression quality for the output images.",
        "  • higher values = better quality but larger file sizes",
        "  • lower values = smaller files but more compression artifacts",
        "",
        "brightness (default: -10, range: -100 to 100):",
        "  adjusts the overall lightness/darkness of pages.",
        "  • positive values make pages brighter",
        "  • negative values make pages darker",
        "  • 0 = no adjustment",
        "",
        "contrast/gamma (default: 1.8, range: 0.1 to 3.0):",
        "  controls the contrast and tone curve of the images.",
        "  • values < 1.0 = lower contrast, lifted shadows",
        "  • values > 1.0 = higher contrast, deeper shadows",
        "  • 1.0 = no adjustment",
        "",
        "device dimensions (default: 1236x1648 - kindle paperwhite 11):",
        "  target resolution for the output. images will be scaled to fit",
        "  within these dimensions while preserving aspect ratio.",
    ];

    let help_paragraph = Paragraph::new(help_text.join("\n"))
        .style(Style::default().fg(theme.content))
        .block(
            Block::default()
                .title(" settings help (press 'h' or esc to close) ")
                .borders(Borders::ALL)
                .border_style(theme.focused)
                .style(Style::default().bg(theme.background)),
        )
        .alignment(Alignment::Left);

    help_paragraph.render(popup_area, buf);
}
