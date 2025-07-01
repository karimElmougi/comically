use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::{Alignment, Constraint, Direction, Flex, Layout, Position, Rect},
    style::{Modifier, Style, Stylize},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};
use ratatui_image::{
    picker::Picker,
    thread::{ResizeRequest, ResizeResponse, ThreadProtocol},
    Resize, ResizeEncodeRender, StatefulImage,
};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use crate::{
    comic::ComicConfig,
    comic_archive::{self, ArchiveFile},
    tui::Theme,
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
    Contrast,
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
    },
}

pub enum ConfigEvent {
    ImageLoaded {
        archive_path: PathBuf,
        image: image::DynamicImage,
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

        let mut state = Self {
            files,
            selected_files,
            list_state,
            config: ComicConfig {
                device_dimensions: (1236, 1648),
                right_to_left: true,
                split_double_page: true,
                auto_crop: true,
                compression_quality: 75,
                brightness: -10,
                contrast: 1.0,
            },
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
        };

        // Auto-load the first image
        state.request_preview_for_selected();

        Ok(state)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
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
            MouseEventKind::Up(MouseButton::Left) => {
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
    fn request_preview_for_selected(&mut self) {
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
            SelectedField::Contrast => {
                let step = if is_fine { 0.05 } else { 0.1 };
                let current = self.config.contrast;
                self.config.contrast = if increase {
                    (current + step).min(2.0)
                } else {
                    (current - step).max(0.0)
                };
            }
        };
    }

    pub fn handle_event(&mut self, event: ConfigEvent) {
        match event {
            ConfigEvent::ImageLoaded {
                image,
                archive_path,
                file,
                config,
            } => {
                self.preview_state.loaded_image = Some(LoadedPreviewImage {
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
            KeyCode::Char('c') => {
                self.config.auto_crop = !self.config.auto_crop;
            }
            KeyCode::Char('u') => {
                self.selected_field = Some(SelectedField::Quality);
            }
            KeyCode::Char('b') => {
                self.selected_field = Some(SelectedField::Brightness);
            }
            KeyCode::Char('k') => {
                self.selected_field = Some(SelectedField::Contrast);
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
                "↑/↓: navigate | space: toggle | a: toggle all | tab: switch panel | t: theme | q: quit"
            }
            (Focus::Settings, Some(_)) => {
                "←/→: adjust | shift+←/→: fine adjust | esc: cancel | enter: process | t: theme | q: quit"
            }
            (Focus::Settings, None) => "enter: start | tab: switch | t: theme| q: quit",
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

        if let Some(mouse) = self.state.last_mouse_click {
            if value_area.contains(Position::new(mouse.column, mouse.row)) {
                on_click(self.state);
            }
        }

        let value_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.primary));

        let value_inner = value_block.inner(value_area);
        value_block.render(value_area, buf);

        Paragraph::new(value)
            .style(Style::default().fg(self.theme.primary))
            .alignment(Alignment::Center)
            .render(value_inner, buf);

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

        let button_style = Style::default().fg(self.theme.primary);

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

        // Render [-] button with border
        if let Some(mouse) = self.state.last_mouse_click {
            if minus_area.contains(Position::new(mouse.column, mouse.row)) {
                on_select(self.state);
                on_adjust(self.state, false);
            }
        }
        let minus_block = Block::default()
            .borders(Borders::ALL)
            .border_style(button_style);
        let minus_inner = minus_block.inner(minus_area);
        minus_block.render(minus_area, buf);
        Paragraph::new("-")
            .style(button_style)
            .alignment(Alignment::Center)
            .render(minus_inner, buf);

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

        // Render [+] button with border
        if let Some(mouse) = self.state.last_mouse_click {
            if plus_area.contains(Position::new(mouse.column, mouse.row)) {
                on_select(self.state);
                on_adjust(self.state, true);
            }
        }
        let plus_block = Block::default()
            .borders(Borders::ALL)
            .border_style(button_style);
        let plus_inner = plus_block.inner(plus_area);
        plus_block.render(plus_area, buf);
        Paragraph::new("+")
            .style(button_style)
            .alignment(Alignment::Center)
            .render(plus_inner, buf);
    }

    fn render_dimension_presets(&mut self, area: Rect, buf: &mut Buffer) {
        let [title_area, values_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

        let [label_area, current_dims_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(title_area);

        Paragraph::new("Dimensions:")
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

        let cells = make_grid_layout::<LEN>(values_area, 2, Constraint::Length(3));

        for (cell, (name, dims)) in cells.into_iter().zip(presets.iter()) {
            let is_current = *dims == current_dims;

            // Handle mouse clicks
            if let Some(mouse) = self.state.last_mouse_click {
                if cell.contains(Position::new(mouse.column, mouse.row)) {
                    self.state.config.device_dimensions = *dims;
                }
            }

            let button_style = if is_current {
                Style::default()
                    .fg(self.theme.secondary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.primary)
            };

            let button_block = Block::default()
                .borders(Borders::ALL)
                .border_style(button_style);
            let button_inner = button_block.inner(cell);
            button_block.render(cell, buf);

            let [name_area, dims_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
                    .areas(button_inner);

            Paragraph::new(*name)
                .style(button_style)
                .alignment(Alignment::Center)
                .render(name_area, buf);

            Paragraph::new(format!("{}x{}", dims.0, dims.1))
                .style(button_style)
                .alignment(Alignment::Center)
                .render(dims_area, buf);
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
            Constraint::Length(1),  // top spacer
            Constraint::Length(8),  // Toggles ( reading direction, split double pages, auto crop)
            Constraint::Length(8),  // Buttons (quality, brightness, contrast)
            Constraint::Length(12), // Dimensions (dynamic grid)
            Constraint::Min(3),     // bottom button
        ];

        let [_, toggles_area, buttons_area, device_presets_area, process_button_area] =
            Layout::vertical(constraints).spacing(1).areas(inner);

        let [reading_direction_area, split_double_pages_area, auto_crop_area] =
            make_grid_layout::<3>(toggles_area, 2, Constraint::Length(4));

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

        let [quality_area, brightness_area, contrast_area] =
            make_grid_layout::<3>(buttons_area, 3, Constraint::Length(4));

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

        self.render_adjustable_setting(
            "Contrast",
            &format!("{:3.1}", self.state.config.contrast),
            "[k]",
            contrast_area,
            buf,
            self.state.selected_field == Some(SelectedField::Contrast),
            |state| {
                state.selected_field = Some(SelectedField::Contrast);
            },
            |state, increase| {
                if let Some(SelectedField::Contrast) = state.selected_field {
                    state.adjust_setting(SelectedField::Contrast, increase, false);
                }
            },
        );

        self.render_dimension_presets(device_presets_area, buf);

        let [process_button_area] = Layout::default()
            .direction(Direction::Vertical)
            .flex(Flex::End)
            .constraints([Constraint::Length(3)])
            .areas(process_button_area);

        ButtonWidget::new(self.theme)
            .text("Start ⏵".to_string())
            .with_mouse_event(self.state.last_mouse_click)
            .on_click(|| {
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

        // Split the area to have a button at the bottom
        let [preview_area, button_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),    // Preview area
                Constraint::Length(3), // Button area
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

        ButtonWidget::new(self.theme)
            .text("Load Preview".to_string())
            .with_mouse_event(self.state.last_mouse_click)
            .enabled(config_changed || file_changed)
            .on_click(|| {
                self.state.request_preview_for_selected();
            })
            .render(button_area, buf);

        if let Some(loaded_image) = &self.state.preview_state.loaded_image {
            let image = StatefulImage::new().resize(Resize::Scale(None));

            let [title_area, image_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(preview_area);

            let name = format!(
                "{} - {}",
                loaded_image.archive_path.file_stem().unwrap().display(),
                loaded_image.image_file.file_stem().display()
            );

            Paragraph::new(name)
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
                    // Don't render anything, waiting for image to load
                }
                PreviewProtocolState::PendingResize { thread_protocol } => {
                    if let Some(rect) =
                        thread_protocol.needs_resize(&Resize::Scale(None), image_area)
                    {
                        thread_protocol.resize_encode(&Resize::Scale(None), rect);
                    }
                }
                PreviewProtocolState::Ready { thread_protocol } => {
                    StatefulWidget::render(image, image_area, buf, thread_protocol);
                }
            }
        } else {
            // Preview is loading - show nothing while we wait
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
                } => {
                    let result = load_and_process_preview(&path, &config);

                    match result {
                        Ok((image, file)) => {
                            let _ = tx.send(crate::Event::Config(ConfigEvent::ImageLoaded {
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

pub struct ButtonWidget<'a> {
    pub text: String,
    pub style: Style,
    pub enabled: bool,
    pub mouse_event: Option<MouseEvent>,
    pub on_click: Option<Box<dyn FnOnce() + 'a>>,
    pub theme: &'a Theme,
}

impl<'a> ButtonWidget<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            text: "".to_string(),
            style: Style::default()
                .fg(theme.secondary)
                .add_modifier(Modifier::BOLD),
            mouse_event: None,
            on_click: None,
            enabled: true,
            theme,
        }
    }

    pub fn on_click<'b>(self, on_click: impl FnOnce() + 'b) -> ButtonWidget<'b>
    where
        'a: 'b,
    {
        ButtonWidget {
            text: self.text,
            style: self.style,
            enabled: self.enabled,
            mouse_event: self.mouse_event,
            on_click: Some(Box::new(on_click)),
            theme: self.theme,
        }
    }

    pub fn text(mut self, text: String) -> Self {
        self.text = text;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_mouse_event(mut self, mouse_event: Option<MouseEvent>) -> Self {
        self.mouse_event = mouse_event;
        self
    }
}

impl<'a> Widget for ButtonWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let button_text = format!(" {} ", self.text);
        let desired_button_width = button_text.len() as u16 + 10;
        let desired_button_height = 3;

        let button_width = desired_button_width.min(area.width);
        let button_height = desired_button_height.min(area.height);

        let button_x = area.x + (area.width.saturating_sub(button_width)) / 2;
        let button_y = area.y + (area.height.saturating_sub(button_height)) / 2;

        let button_area = Rect::new(button_x, button_y, button_width, button_height);

        let button_area = area.intersection(button_area);

        let style = if self.enabled {
            self.style
        } else {
            self.style.add_modifier(Modifier::DIM)
        };

        if button_area.width > 0 && button_area.height > 0 {
            let button_block = Block::default().borders(Borders::ALL).border_style(style);

            let button_inner = button_block.inner(button_area);
            button_block.render(button_area, buf);

            if self.enabled {
                if let Some(event) = self.mouse_event {
                    if button_area.contains(Position::new(event.column, event.row)) {
                        if let Some(on_click) = self.on_click {
                            on_click();
                        }
                    }
                }
            }

            Paragraph::new(button_text)
                .style(style)
                .alignment(Alignment::Center)
                .render(button_inner, buf);
        }
    }
}

fn load_and_process_preview(
    path: &PathBuf,
    config: &ComicConfig,
) -> anyhow::Result<(image::DynamicImage, ArchiveFile)> {
    let mut files = comic_archive::unarchive_comic_iter(path)?;
    let archive_file = files
        .next()
        .ok_or_else(|| anyhow::anyhow!("No images in archive"))??;

    let img = image::load_from_memory(&archive_file.data)?;

    let processed_images = crate::image_processor::process_image(img, config);

    let first_image = processed_images
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No processed images"))?;

    // Compress the image to JPEG with the configured quality
    let mut compressed_buffer = Vec::new();
    crate::image_processor::compress_to_jpeg(
        &first_image,
        &mut compressed_buffer,
        config.compression_quality,
    )?;

    let compressed_img = image::load_from_memory(&compressed_buffer)?;

    Ok((compressed_img, archive_file))
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

fn make_grid_layout<const N: usize>(
    area: Rect,
    items_per_row: u16,
    height: Constraint,
) -> [Rect; N] {
    let col_constraints = (0..items_per_row).map(|_| Constraint::Min(0));
    let row_constraints =
        (0..((N + items_per_row as usize - 1) / items_per_row as usize)).map(|_| height);
    let horizontal = Layout::horizontal(col_constraints).spacing(1);
    let vertical = Layout::vertical(row_constraints);

    let rows = vertical.split(area);
    rows.iter()
        .flat_map(move |&row| horizontal.split(row).to_vec())
        .take(N)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap()
}
