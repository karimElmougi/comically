pub mod device_selector;

use imageproc::image::DynamicImage;
use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::{Alignment, Constraint, Direction, Flex, Layout, Position, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
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
    comic::{ComicConfig, OutputFormat, SplitStrategy},
    comic_archive,
    tui::{
        button::{Button, ButtonVariant},
        config::device_selector::DeviceSelectorState,
        utils::{padding, popup_block, themed_block, Side},
        Theme,
    },
};

pub struct ConfigState {
    pub files: Vec<(MangaFile, bool)>,
    pub file_list_state: ListState,
    pub selected_field: Option<SelectedField>,
    pub preview_state: PreviewState,

    pub config: ComicConfig,
    pub theme: Theme,
    pub event_tx: std::sync::mpsc::Sender<crate::Event>,
    pub last_mouse_click: Option<MouseEvent>,
    pub output_dir: PathBuf,

    pub modal_state: ModalState,
}

pub enum ModalState {
    None,
    Help,
    DeviceSelector(DeviceSelectorState),
}

#[derive(Debug)]
pub struct MangaFile {
    pub archive_path: PathBuf,
    pub name: String,
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
    picker: Picker,
    protocol_state: PreviewProtocolState,
    preview_tx: mpsc::Sender<PreviewRequest>,
    resize_tx: mpsc::Sender<ResizeRequest>,
    loaded_image: Option<LoadedPreviewImage>,
}

#[derive(Debug, Clone)]
pub struct LoadedPreviewImage {
    file_idx: usize,
    page_idx: usize,
    total_pages: usize,
    archive_path: PathBuf,
    width: u32,
    height: u32,
    config: ComicConfig,
}

enum PreviewRequest {
    LoadFile {
        archive_path: PathBuf,
        config: ComicConfig,
        page_idx: Option<usize>,
        file_idx: usize,
    },
}

pub enum ConfigEvent {
    ImageLoaded {
        file_idx: usize,
        page_idx: usize,
        total_pages: usize,
        archive_path: PathBuf,
        image: DynamicImage,
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
        theme: Theme,
        output_dir: PathBuf,
    ) -> Self {
        let files: Vec<(MangaFile, bool)> = files.into_iter().map(|f| (f, true)).collect();

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
            file_list_state: list_state,
            config,
            selected_field: None,
            preview_state: PreviewState {
                picker,
                protocol_state: PreviewProtocolState::None,
                preview_tx,
                resize_tx,
                loaded_image: None,
            },
            theme,
            event_tx,
            last_mouse_click: None,
            output_dir,
            modal_state: ModalState::None,
        };

        // Auto-load the first image
        state.load_preview();

        state
    }

    pub fn is_modal_open(&self) -> bool {
        !matches!(self.modal_state, ModalState::None)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.modal_state = ModalState::None;
            self.selected_field = None;
            return;
        }

        match &mut self.modal_state {
            ModalState::DeviceSelector(selector) => {
                if key.code == KeyCode::Char('d') {
                    self.modal_state = ModalState::None;
                    return;
                }

                if let Some(preset) = selector.handle_key(key) {
                    self.modal_state = ModalState::None;
                    self.config.device = preset;
                    return;
                }
            }
            ModalState::Help => {
                if key.code == KeyCode::Char('h') {
                    self.modal_state = ModalState::None;
                    return;
                }
            }
            ModalState::None => {}
        }

        match key.code {
            KeyCode::Char('h') => {
                self.modal_state = ModalState::Help;
            }
            KeyCode::Enter => {
                self.send_start_processing();
            }
            // File list navigation
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_previous();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
            }
            KeyCode::Char(' ') => {
                if let Some(selected) = self.file_list_state.selected() {
                    self.files[selected].1 = !self.files[selected].1;
                }
            }
            KeyCode::Char('a') => {
                let all_selected = self.files.iter().all(|(_, selected)| *selected);
                for (_, selected) in &mut self.files {
                    *selected = !all_selected;
                }
            }

            // Settings toggles
            KeyCode::Char('m') => {
                self.config.right_to_left = !self.config.right_to_left;
            }
            KeyCode::Char('s') => {
                use crate::comic::SplitStrategy;
                self.config.split = match self.config.split {
                    SplitStrategy::None => SplitStrategy::Split,
                    SplitStrategy::Split => SplitStrategy::Rotate,
                    SplitStrategy::Rotate => SplitStrategy::RotateAndSplit,
                    SplitStrategy::RotateAndSplit => SplitStrategy::None,
                };
            }
            KeyCode::Char('c') => {
                self.config.auto_crop = !self.config.auto_crop;
            }
            KeyCode::Char('f') => {
                self.config.output_format = match self.config.output_format {
                    OutputFormat::Mobi => OutputFormat::Epub,
                    OutputFormat::Epub => OutputFormat::Cbz,
                    OutputFormat::Cbz => OutputFormat::Mobi,
                };
            }
            KeyCode::Char('u') => {
                self.selected_field = Some(SelectedField::Quality);
            }
            KeyCode::Char('b') => {
                self.selected_field = Some(SelectedField::Brightness);
            }
            KeyCode::Char('g') => {
                self.selected_field = Some(SelectedField::Gamma);
            }
            KeyCode::Char('d') => {
                self.modal_state = ModalState::DeviceSelector(DeviceSelectorState::new(
                    self.config.device.clone(),
                ));
            }
            KeyCode::Char('p') => {
                self.load_preview();
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

            _ => {}
        }
    }

    fn send_start_processing(&self) {
        let selected_paths: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, selected)| *selected)
            .map(|(file, _)| file.archive_path.clone())
            .collect();

        if !selected_paths.is_empty() {
            let _ = self.event_tx.send(crate::Event::StartProcessing {
                files: selected_paths,
                config: self.config.clone(),
                output_dir: self.output_dir.clone(),
            });
        }
    }

    pub fn handle_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        match mouse.kind {
            MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Down(MouseButton::Left) => {
                self.last_mouse_click = Some(mouse);
            }
            MouseEventKind::ScrollUp => match &mut self.modal_state {
                ModalState::DeviceSelector(s) => {
                    s.select_previous();
                }
                ModalState::Help => {}
                ModalState::None => {
                    self.select_previous();
                }
            },
            MouseEventKind::ScrollDown => match &mut self.modal_state {
                ModalState::DeviceSelector(s) => {
                    s.select_next();
                }
                ModalState::Help => {}
                ModalState::None => {
                    self.select_next();
                }
            },
            _ => {}
        }
    }

    // request a preview for the selected file
    fn load_preview(&mut self) {
        if let Some(file_idx) = self.file_list_state.selected() {
            if let Some((file, _)) = self.files.get(file_idx) {
                self.preview_state.protocol_state = PreviewProtocolState::None;

                let idx = self
                    .preview_state
                    .loaded_image
                    .as_ref()
                    .filter(|i| i.file_idx == file_idx)
                    .map(|i| i.page_idx)
                    .unwrap_or(0);

                let _ = self
                    .preview_state
                    .preview_tx
                    .send(PreviewRequest::LoadFile {
                        archive_path: file.archive_path.clone(),
                        config: self.config.clone(),
                        page_idx: Some(idx),
                        file_idx,
                    });
            }
        }
    }

    // request a random page preview for the selected file
    fn request_random_preview_for_current(&mut self) {
        if let Some(file) = self.preview_state.loaded_image.as_ref() {
            self.preview_state.protocol_state = PreviewProtocolState::None;

            let _ = self
                .preview_state
                .preview_tx
                .send(PreviewRequest::LoadFile {
                    archive_path: file.archive_path.clone(),
                    config: self.config.clone(),
                    page_idx: None,
                    file_idx: file.file_idx,
                });
        }
    }

    // navigate to next page in preview
    fn next_preview_page(&mut self) {
        if let Some(file) = self.preview_state.loaded_image.as_ref() {
            self.preview_state.protocol_state = PreviewProtocolState::None;

            let next_idx = file.page_idx.saturating_add(1);

            let _ = self
                .preview_state
                .preview_tx
                .send(PreviewRequest::LoadFile {
                    archive_path: file.archive_path.clone(),
                    config: self.config.clone(),
                    page_idx: Some(next_idx),
                    file_idx: file.file_idx,
                });
        }
    }

    // navigate to previous page in preview
    fn previous_preview_page(&mut self) {
        if let Some(file) = self.preview_state.loaded_image.as_ref() {
            self.preview_state.protocol_state = PreviewProtocolState::None;

            let prev_idx = file.page_idx.saturating_sub(1);

            let _ = self
                .preview_state
                .preview_tx
                .send(PreviewRequest::LoadFile {
                    archive_path: file.archive_path.clone(),
                    config: self.config.clone(),
                    page_idx: Some(prev_idx),
                    file_idx: file.file_idx,
                });
        }
    }

    // update picker and reload the current image
    pub fn update_picker(&mut self, new_picker: Picker) {
        self.preview_state.picker = new_picker;
        self.preview_state.protocol_state = PreviewProtocolState::None;
        if let Some(loaded_image) = self.preview_state.loaded_image.as_ref() {
            let _ = self
                .preview_state
                .preview_tx
                .send(PreviewRequest::LoadFile {
                    archive_path: loaded_image.archive_path.clone(),
                    config: self.config.clone(),
                    page_idx: Some(loaded_image.page_idx),
                    file_idx: loaded_image.file_idx,
                });
        }
    }

    fn select_previous(&mut self) {
        if let Some(selected) = self.file_list_state.selected() {
            if selected > 0 {
                self.file_list_state.select(Some(selected - 1));
            }
        }
    }

    fn select_next(&mut self) {
        if let Some(selected) = self.file_list_state.selected() {
            if selected < self.files.len() - 1 {
                self.file_list_state.select(Some(selected + 1));
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
                file_idx,
                page_idx,
                total_pages,
                image,
                archive_path,
                config,
            } => {
                self.preview_state.loaded_image = Some(LoadedPreviewImage {
                    file_idx,
                    page_idx,
                    total_pages,
                    archive_path,
                    width: image.width(),
                    height: image.height(),
                    config,
                });
                let protocol = self.preview_state.picker.new_resize_protocol(image);
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
}

pub struct ConfigScreen<'a> {
    state: &'a mut ConfigState,
}

impl<'a> ConfigScreen<'a> {
    pub fn new(state: &'a mut ConfigState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for ConfigScreen<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, Style::default().bg(self.state.theme.background));

        let [header_area, main_area, footer_area] = Layout::vertical([
            Constraint::Length(3), // Header
            Constraint::Min(0),    // Main content
            Constraint::Length(3), // Footer
        ])
        .areas(area);

        super::render_title(&self.state.theme).render(header_area, buf);

        let [file_list_area, settings_area, preview_area] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Fill(2),
            Constraint::Fill(1),
        ])
        .areas(main_area);

        FileListWidget::new(self.state).render(file_list_area, buf);

        SettingsWidget::new(self.state).render(settings_area, buf);

        PreviewWidget::new(self.state).render(preview_area, buf);

        let footer_text = if self.state.selected_field.is_some() {
            "←/→: adjust | shift+←/→: fine adjust | esc: cancel | h: help | t: theme | q: quit"
        } else {
            "↑/↓/j/k: navigate | space: toggle | a: all | h: help | t: theme | q: quit"
        };
        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(self.state.theme.content))
            .alignment(Alignment::Center)
            .block(themed_block(None, &self.state.theme));
        footer.render(footer_area, buf);

        match &self.state.modal_state {
            ModalState::Help => render_help_popup(area, buf, &self.state.theme),
            ModalState::DeviceSelector(_) => {
                device_selector::render_device_selector_popup(area, buf, self.state);
            }
            ModalState::None => {}
        }

        self.state.last_mouse_click = None;
    }
}

struct FileListWidget<'a> {
    state: &'a mut ConfigState,
}

impl<'a> FileListWidget<'a> {
    fn new(state: &'a mut ConfigState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for FileListWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Some(mouse) = self.state.last_mouse_click {
            if area.contains(Position::new(mouse.column, mouse.row)) {
                self.state.selected_field = None;
            }
        }

        let items: Vec<ListItem> = self
            .state
            .files
            .iter()
            .map(|(file, selected)| {
                let checkbox = if *selected { "[✓]" } else { "[ ]" };
                let content = format!("{} {}", checkbox, file.name);
                ListItem::new(content).style(self.state.theme.content)
            })
            .collect();

        let selected_count = self
            .state
            .files
            .iter()
            .filter(|(_, selected)| *selected)
            .count();

        let list = List::new(items)
            .block(
                themed_block(Some("files"), &self.state.theme).title(
                    Line::from(format!("{selected_count}"))
                        .right_aligned()
                        .style(self.state.theme.accent),
                ),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");

        StatefulWidget::render(list, area, buf, &mut self.state.file_list_state);
    }
}

struct SettingsWidget<'a> {
    state: &'a mut ConfigState,
}

impl<'a> SettingsWidget<'a> {
    fn new(state: &'a mut ConfigState) -> Self {
        Self { state }
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
            Style::default().fg(self.state.theme.accent).underlined()
        } else {
            Style::default().fg(self.state.theme.content)
        };

        let [header_area, buttons_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(3)]).areas(area);

        let [text_area, shortcut_area] = Layout::horizontal([
            Constraint::Length(label.len() as u16 + 1),
            Constraint::Length(key.len() as u16 + 1),
        ])
        .flex(Flex::SpaceBetween)
        .areas(header_area);

        Paragraph::new(label).style(style).render(text_area, buf);

        Paragraph::new(format!(" {}", key))
            .style(Style::default().fg(self.state.theme.accent))
            .render(shortcut_area, buf);

        let [minus_area, value_area, plus_area] =
            Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(buttons_area);

        // Render [-] button
        base_button("-", self.state)
            .on_click(|| {
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
                    .fg(self.state.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center)
            .render(value_layout, buf);

        base_button("+", self.state)
            .on_click(|| {
                on_select(self.state);
                on_adjust(self.state, true);
            })
            .render(plus_area, buf);
    }

    fn render_device_selector_button(&mut self, area: Rect, buf: &mut Buffer) {
        let current_preset = &self.state.config.device;
        let (width, height) = current_preset.dimensions;
        let button_text = format!("{} ({}x{})", current_preset.name, width, height);

        base_button(button_text, self.state)
            .on_click(|| {
                // make sure the mouse click is not used in the popup layer
                self.state.last_mouse_click = None;
                self.state.modal_state = ModalState::DeviceSelector(DeviceSelectorState::new(
                    self.state.config.device.clone(),
                ));
            })
            .label("device")
            .hint("[d]")
            .render(area, buf);
    }
}

impl<'a> Widget for SettingsWidget<'a> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        let block = themed_block(Some("settings"), &self.state.theme);
        let inner = block.inner(area);
        block.render(area, buf);

        let [toggles_area, buttons_area, device_selector_area, process_button_area] =
            Layout::vertical([
                Constraint::Length(9), // Toggles ( reading direction, split double pages, auto crop)
                Constraint::Length(4), // Buttons (quality, brightness, contrast)
                Constraint::Length(4), // Device selector button
                Constraint::Min(3),    // bottom button
            ])
            .flex(Flex::Start)
            .spacing(1)
            .areas(padding(inner, Constraint::Length(1), Side::Top));

        let [row1, row2] = Layout::vertical([Constraint::Length(4), Constraint::Length(4)])
            .spacing(1)
            .areas(toggles_area);

        let [reading_direction_area, split_double_pages_area] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .spacing(1)
                .areas(row1);

        let [auto_crop_area, output_format_area] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .spacing(1)
                .areas(row2);

        base_button(
            if self.state.config.right_to_left {
                "right to left (manga)"
            } else {
                "left to right"
            },
            self.state,
        )
        .label("reading direction")
        .hint("[m]")
        .on_click(|| {
            self.state.config.right_to_left = !self.state.config.right_to_left;
        })
        .render(reading_direction_area, buf);

        base_button(
            match self.state.config.split {
                SplitStrategy::None => "none",
                SplitStrategy::Split => "split",
                SplitStrategy::Rotate => "rotate",
                SplitStrategy::RotateAndSplit => "split & rotate",
            },
            self.state,
        )
        .label("spread splitter")
        .hint("[s]")
        .on_click(|| {
            self.state.config.split = match self.state.config.split {
                SplitStrategy::None => SplitStrategy::Split,
                SplitStrategy::Split => SplitStrategy::Rotate,
                SplitStrategy::Rotate => SplitStrategy::RotateAndSplit,
                SplitStrategy::RotateAndSplit => SplitStrategy::None,
            };
        })
        .render(split_double_pages_area, buf);

        base_button(
            if self.state.config.auto_crop {
                "yes"
            } else {
                "no"
            },
            self.state,
        )
        .label("auto crop")
        .hint("[c]")
        .on_click(|| {
            self.state.config.auto_crop = !self.state.config.auto_crop;
        })
        .render(auto_crop_area, buf);

        base_button(
            match self.state.config.output_format {
                OutputFormat::Mobi => "AZW3/MOBI",
                OutputFormat::Epub => "EPUB",
                OutputFormat::Cbz => "CBZ",
            },
            self.state,
        )
        .label("output format")
        .hint("[f]")
        .on_click(|| {
            self.state.config.output_format = match self.state.config.output_format {
                OutputFormat::Mobi => OutputFormat::Epub,
                OutputFormat::Epub => OutputFormat::Cbz,
                OutputFormat::Cbz => OutputFormat::Mobi,
            };
        })
        .render(output_format_area, buf);

        // Create a horizontal layout for the three adjustable settings
        let [quality_area, brightness_area, contrast_area] =
            Layout::horizontal([Constraint::Ratio(1, 3); 3])
                .flex(Flex::SpaceBetween)
                .spacing(2)
                .areas(buttons_area);

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
            "gamma",
            &format!("{:3.2}", self.state.config.gamma),
            "[g]",
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

        self.render_device_selector_button(device_selector_area, buf);

        let [process_button_area] = Layout::default()
            .direction(Direction::Vertical)
            .flex(Flex::End)
            .constraints([Constraint::Length(4)])
            .areas(process_button_area);

        base_button("start ⏵", self.state)
            .hint("[enter]")
            .on_click(|| {
                self.state.send_start_processing();
            })
            .variant(ButtonVariant::Secondary)
            .render(process_button_area, buf);
    }
}

struct PreviewWidget<'a> {
    state: &'a mut ConfigState,
}

impl<'a> PreviewWidget<'a> {
    fn new(state: &'a mut ConfigState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for PreviewWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = themed_block(Some("preview"), &self.state.theme);

        let inner = block.inner(area);
        block.render(area, buf);

        // Split the area to have buttons at the bottom
        let [preview_area, buttons_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(8)])
            .areas(inner);

        let config_changed = self
            .state
            .preview_state
            .loaded_image
            .as_ref()
            .map(|loaded| loaded.config != self.state.config)
            .unwrap_or(true);

        let file_changed = self
            .state
            .file_list_state
            .selected()
            .and_then(|idx| self.state.files.get(idx))
            .and_then(|(selected_file, _)| {
                self.state
                    .preview_state
                    .loaded_image
                    .as_ref()
                    .map(|loaded| loaded.archive_path != selected_file.archive_path)
            })
            .unwrap_or(true);

        let modal_open = self.state.is_modal_open();

        // Split buttons area: 1 button on top, 3 buttons below
        let [top_button_area, bottom_buttons_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // Load preview button
                Constraint::Length(3), // Navigation buttons
            ])
            .spacing(1)
            .flex(Flex::End)
            .areas(buttons_area);

        // Load preview button (full width)
        base_button("load preview", self.state)
            .hint("[p]")
            .on_click(|| {
                self.state.load_preview();
            })
            .enabled((config_changed || file_changed) && !modal_open)
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

        base_button("◀ prev", self.state)
            .on_click(|| {
                self.state.previous_preview_page();
            })
            .render(prev_button_area, buf);

        base_button("random", self.state)
            .on_click(|| {
                self.state.request_random_preview_for_current();
            })
            .render(random_button_area, buf);

        base_button("next ▶", self.state)
            .on_click(|| {
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
                loaded_image.page_idx + 1,
                loaded_image.total_pages
            );

            let text = vec![
                Line::from(file_name),
                Line::from(page_info).style(Style::default().fg(self.state.theme.border)),
            ];

            Paragraph::new(text)
                .style(Style::default().fg(self.state.theme.content))
                .alignment(Alignment::Center)
                .render(title_area, buf);

            let image_area = calculate_centered_image_area(
                image_area,
                loaded_image,
                self.state.preview_state.picker.font_size(),
            );

            match &mut self.state.preview_state.protocol_state {
                PreviewProtocolState::None => {
                    render_image_placeholder(image_area, buf, &self.state.theme);
                }
                PreviewProtocolState::PendingResize { thread_protocol } => {
                    if let Some(rect) =
                        thread_protocol.needs_resize(&Resize::Scale(None), image_area)
                    {
                        thread_protocol.resize_encode(&Resize::Scale(None), rect);
                    }
                    render_image_placeholder(image_area, buf, &self.state.theme);
                }
                PreviewProtocolState::Ready { thread_protocol } => {
                    StatefulWidget::render(image, image_area, buf, thread_protocol);
                }
            }
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
                    page_idx,
                    file_idx,
                } => {
                    let result = load_and_process_preview(&path, &config, page_idx);

                    match result {
                        Ok((image, idx, total_pages)) => {
                            let _ = tx.send(crate::Event::Config(ConfigEvent::ImageLoaded {
                                file_idx,
                                page_idx: idx,
                                total_pages,
                                archive_path: path,
                                image,
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

// - default enabled = !modal_open
// - default mouse_event = last_mouse_click
fn base_button<'input, 'state>(
    text: impl Into<ratatui::text::Text<'input>>,
    config: &'state ConfigState,
) -> Button<'input> {
    Button::new(text, config.theme)
        .enabled(!config.is_modal_open())
        .mouse_event(config.last_mouse_click)
}

fn load_and_process_preview(
    path: &PathBuf,
    config: &ComicConfig,
    page_index: Option<usize>,
) -> anyhow::Result<(DynamicImage, usize, usize)> {
    let mut archive_files: Vec<_> = comic_archive::unarchive_comic_iter(path)?
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
            rand::rng().random_range(0..archive_files.len())
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

    Ok((compressed_img, idx, total_pages))
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

    let lines = vec![
        Line::from(vec![
            Span::styled(
                "reading direction ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "(default: right to left)",
                Style::default()
                    .fg(theme.content)
                    .add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("left to right", Style::default().fg(theme.primary)),
            Span::raw(": standard western comic/book order"),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("right to left", Style::default().fg(theme.primary)),
            Span::raw(": manga order - pages flow right to left"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "spread splitter ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "(default: rotate & split)",
                Style::default()
                    .fg(theme.content)
                    .add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("none", Style::default().fg(theme.primary)),
            Span::raw(": keep spreads as-is"),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("split", Style::default().fg(theme.primary)),
            Span::raw(": cut spreads into separate pages"),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("rotate", Style::default().fg(theme.primary)),
            Span::raw(": rotate spreads 90° for better viewing"),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("rotate & split", Style::default().fg(theme.primary)),
            Span::raw(": show twice - rotated then split"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "auto crop ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "(default: yes)",
                Style::default()
                    .fg(theme.content)
                    .add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from("  removes blank space for better fit"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "quality ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "(default: 85, range: 0-100)",
                Style::default()
                    .fg(theme.content)
                    .add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from("  jpeg compression quality"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "brightness ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "(default: -10, range: -100 to 100)",
                Style::default()
                    .fg(theme.content)
                    .add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from("  adjusts overall lightness/darkness"),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("+", Style::default().fg(theme.primary)),
            Span::raw(" values: brighter"),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("−", Style::default().fg(theme.primary)),
            Span::raw(" values: darker"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "gamma ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "(default: 1.8, range: 0.1-3.0)",
                Style::default()
                    .fg(theme.content)
                    .add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from("  controls contrast and tone curve"),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("< 1.0", Style::default().fg(theme.primary)),
            Span::raw(": lower contrast, lifted shadows"),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("> 1.0", Style::default().fg(theme.primary)),
            Span::raw(": higher contrast, deeper shadows"),
        ]),
        Line::from(vec![
            Span::raw("  • "),
            Span::styled("= 1.0", Style::default().fg(theme.primary)),
            Span::raw(": no adjustment"),
        ]),
        Line::from(""),
    ];

    let help_text = Text::from(lines);
    let help_paragraph = Paragraph::new(help_text)
        .style(Style::default().fg(theme.content))
        .block(popup_block("help", theme).title(Line::from("[esc/h]").right_aligned()))
        .alignment(Alignment::Left);

    help_paragraph.render(popup_area, buf);
}
