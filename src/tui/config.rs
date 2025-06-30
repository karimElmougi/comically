use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
    },
};
use ratatui_image::{
    errors::Errors,
    picker::Picker,
    thread::{ResizeRequest, ResizeResponse, ThreadProtocol},
    Resize, StatefulImage,
};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use crate::{comic_archive, ComicConfig};

pub struct ConfigState {
    pub files: Vec<MangaFile>,
    pub selected_files: Vec<bool>,
    pub list_state: ListState,
    pub config: ComicConfig,
    pub prefix: Option<String>,
    pub focus: Focus,
    pub selected_field: Option<SelectedField>,
    pub input_buffer: String,
    pub preview_state: PreviewState,
    picker: Picker,
    event_tx: std::sync::mpsc::Sender<crate::Event>,
    last_mouse_click: Option<MouseEvent>,
}

#[derive(Debug)]
pub struct MangaFile {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    FileList,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelectedField {
    Prefix,
    Quality,
    Brightness,
    Contrast,
}

pub struct PreviewState {
    thread_protocol: ThreadProtocol,
    preview_tx: mpsc::Sender<PreviewRequest>,
    resize_tx: mpsc::Sender<ResizeRequest>,
    loading: bool,
    selection_changed_at: Option<Instant>,
}

enum PreviewRequest {
    LoadFile { path: PathBuf, config: ComicConfig },
}

pub enum ConfigEvent {
    ImageLoaded(image::DynamicImage),
    ResizeComplete(Result<ResizeResponse, Errors>),
    Error(String),
}

impl ConfigState {
    pub fn new(event_tx: mpsc::Sender<crate::Event>, picker: Picker) -> anyhow::Result<Self> {
        let files = find_manga_files(".")?;
        let selected_files = vec![true; files.len()]; // Select all by default

        let mut list_state = ListState::default();
        if !files.is_empty() {
            list_state.select(Some(0));
        }

        let has_files = !files.is_empty();

        // Create channels for preview processing
        let (preview_tx, worker_rx) = mpsc::channel::<PreviewRequest>();
        // Create channel for resize requests
        let (resize_tx, resize_rx) = mpsc::channel::<ResizeRequest>();

        // Create ThreadProtocol for handling resizing
        let thread_protocol = ThreadProtocol::new(resize_tx.clone(), None);

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
                brightness: Some(-10),
                contrast: Some(1.0),
            },
            prefix: None,
            focus: Focus::FileList,
            selected_field: None,
            input_buffer: String::new(),
            preview_state: PreviewState {
                thread_protocol,
                preview_tx,
                resize_tx,
                loading: false,
                selection_changed_at: None,
            },
            picker,
            event_tx,
            last_mouse_click: None,
        };

        // Mark selection changed to trigger initial preview after debounce
        if has_files {
            state.preview_state.selection_changed_at = Some(Instant::now());
        }

        Ok(state)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Check if we're editing the prefix field
        if let Some(SelectedField::Prefix) = self.selected_field {
            if !self.input_buffer.is_empty() || key.code != KeyCode::Esc {
                self.handle_prefix_editing(key);
                return;
            }
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::FileList => Focus::Settings,
                    Focus::Settings => Focus::FileList,
                };
                self.selected_field = None; // Clear selection when switching focus
            }
            KeyCode::Enter => {
                if self.focus == Focus::Settings {
                    self.send_start_processing();
                }
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
            .map(|(file, _)| file.path.clone())
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

    fn request_preview(&mut self) {
        if let Some(file_idx) = self.list_state.selected() {
            if let Some(file) = self.files.get(file_idx) {
                self.preview_state.loading = true;
                self.preview_state.selection_changed_at = None; // Clear it once we start processing
                let _ = self
                    .preview_state
                    .preview_tx
                    .send(PreviewRequest::LoadFile {
                        path: file.path.clone(),
                        config: self.config,
                    });
            }
        }
    }

    pub fn check_preview_debounce(&mut self) {
        if let Some(changed_at) = self.preview_state.selection_changed_at {
            if changed_at.elapsed().as_millis() >= 500 && !self.preview_state.loading {
                self.request_preview();
            }
        }
    }

    pub fn mark_preview_dirty(&mut self) {
        self.preview_state.selection_changed_at = Some(Instant::now());
    }

    fn select_previous(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected > 0 {
                self.list_state.select(Some(selected - 1));
                self.mark_preview_dirty();
            }
        }
    }

    fn select_next(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected < self.files.len() - 1 {
                self.list_state.select(Some(selected + 1));
                self.mark_preview_dirty();
            }
        }
    }

    fn adjust_setting(&mut self, field: SelectedField, increase: bool, is_fine: bool) {
        let should_update_preview = match field {
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
                true
            }
            SelectedField::Brightness => {
                let step = if is_fine { 1 } else { 5 };
                let current = self.config.brightness.unwrap_or(-10);
                self.config.brightness = Some(if increase {
                    (current + step).min(100)
                } else {
                    (current - step).max(-100)
                });
                true
            }
            SelectedField::Contrast => {
                let step = if is_fine { 0.05 } else { 0.1 };
                let current = self.config.contrast.unwrap_or(1.0);
                self.config.contrast = Some(if increase {
                    (current + step).min(2.0)
                } else {
                    (current - step).max(0.5)
                });
                true
            }
            SelectedField::Prefix => false, // Prefix doesn't use adjustment
        };

        if should_update_preview {
            self.mark_preview_dirty();
        }
    }

    pub fn handle_event(&mut self, event: ConfigEvent) {
        match event {
            ConfigEvent::ImageLoaded(img) => {
                tracing::info!("Received new image for preview");
                self.preview_state.loading = false;
                // Create a new resize protocol for the image
                let protocol = self.picker.new_resize_protocol(img);
                self.preview_state.thread_protocol =
                    ThreadProtocol::new(self.preview_state.resize_tx.clone(), Some(protocol));
            }
            ConfigEvent::ResizeComplete(result) => match result {
                Ok(response) => {
                    let _ = self
                        .preview_state
                        .thread_protocol
                        .update_resized_protocol(response);
                }
                Err(e) => {
                    tracing::warn!("Resize error: {:?}", e);
                }
            },
            ConfigEvent::Error(err) => {
                tracing::warn!("Preview error: {}", err);
                self.preview_state.loading = false;
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
                self.mark_preview_dirty();
            }
            KeyCode::Char('u') => {
                self.selected_field = Some(SelectedField::Quality);
            }
            KeyCode::Char('b') => {
                self.selected_field = Some(SelectedField::Brightness);
            }
            KeyCode::Char('t') => {
                self.selected_field = Some(SelectedField::Contrast);
            }
            KeyCode::Char('p') => {
                self.selected_field = Some(SelectedField::Prefix);
                self.input_buffer = self.prefix.clone().unwrap_or_default();
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
                self.input_buffer.clear();
            }
            _ => {}
        }
    }

    fn handle_prefix_editing(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.selected_field = None;
                self.input_buffer.clear();
            }
            KeyCode::Enter => {
                self.prefix = if self.input_buffer.is_empty() {
                    None
                } else {
                    Some(self.input_buffer.clone())
                };
                self.selected_field = None;
                self.input_buffer.clear();
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            _ => {}
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
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(0),    // Main content
                Constraint::Length(3), // Footer
            ])
            .split(area);

        // Header with current directory
        let current_dir = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let header_text = vec![
            Line::from(vec![Span::styled(
                "Comically - Manga Configuration",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![
                Span::raw("Directory: "),
                Span::styled(current_dir, Style::default().fg(Color::Yellow)),
            ]),
        ];
        let header = Paragraph::new(header_text)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        header.render(chunks[0], buf);

        // Main content area
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25), // File list
                Constraint::Percentage(35), // Settings
                Constraint::Percentage(40), // Preview - now the largest!
            ])
            .split(chunks[1]);

        // File list
        FileListWidget::new(&self.state).render(main_chunks[0], buf);

        // Settings panel
        SettingsWidget::new(self.state).render(main_chunks[1], buf);

        // Preview panel
        PreviewWidget::new(self.state).render(main_chunks[2], buf);

        // Footer
        let footer_text = match (self.state.focus, self.state.selected_field) {
            (Focus::FileList, _) => {
                "↑/↓: Navigate | Space: Toggle | a: Toggle All | Tab: Switch Panel | q: Quit"
            }
            (Focus::Settings, Some(SelectedField::Prefix)) => {
                "Type to edit | Enter: Save | Esc: Cancel"
            }
            (Focus::Settings, Some(_)) => {
                "←/→: Adjust | Shift+←/→: Fine adjust | Esc: Cancel | Enter: Process"
            }
            (Focus::Settings, None) => {
                "u/b/t: Select setting | m/s/c: Toggle | p: Prefix | Enter: Process | Tab: Switch"
            }
        };
        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        footer.render(chunks[2], buf);
    }
}

struct FileListWidget<'a> {
    state: &'a ConfigState,
}

impl<'a> FileListWidget<'a> {
    fn new(state: &'a ConfigState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for FileListWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let items: Vec<ListItem> = self
            .state
            .files
            .iter()
            .zip(&self.state.selected_files)
            .map(|(file, selected)| {
                let checkbox = if *selected { "[✓]" } else { "[ ]" };
                let content = format!("{} {}", checkbox, file.name);
                ListItem::new(content)
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!(
                        "Files ({} selected)",
                        self.state.selected_files.iter().filter(|&&s| s).count()
                    ))
                    .borders(Borders::ALL)
                    .style(Style::default().fg(if self.state.focus == Focus::FileList {
                        Color::Yellow
                    } else {
                        Color::White
                    })),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");

        let mut list_state = self.state.list_state.clone();
        ratatui::widgets::StatefulWidget::render(list, area, buf, &mut list_state);
    }
}

struct SettingsWidget<'a> {
    state: &'a mut ConfigState,
}

impl<'a> SettingsWidget<'a> {
    fn new(state: &'a mut ConfigState) -> Self {
        Self { state }
    }

    fn render_setting_line(
        &self,
        label: &str,
        value: &str,
        key: &str,
        selected: bool,
    ) -> Line<'static> {
        let style = if selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let button_style = if selected {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        Line::from(vec![
            Span::styled(format!("{}: ", label), style),
            Span::styled("[-]", button_style),
            Span::raw(" "),
            Span::styled(value.to_string(), Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled("[+]", button_style),
            Span::styled(format!(" ({})", key), Style::default().fg(Color::DarkGray)),
        ])
    }
}

impl<'a> Widget for SettingsWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title("Settings")
            .borders(Borders::ALL)
            .style(Style::default().fg(if self.state.focus == Focus::Settings {
                Color::Yellow
            } else {
                Color::White
            }));
        let inner = block.inner(area);
        block.render(area, buf);

        let settings_text = vec![
            Line::from(vec![
                Span::raw("Title Prefix: "),
                Span::styled(
                    self.state
                        .prefix
                        .clone()
                        .unwrap_or_else(|| "(none)".to_string()),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" [p]"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("Reading Direction: "),
                Span::styled(
                    if self.state.config.right_to_left {
                        "Right to Left (Manga)"
                    } else {
                        "Left to Right"
                    },
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" [m]"),
            ]),
            Line::from(vec![
                Span::raw("Split Double Pages: "),
                Span::styled(
                    if self.state.config.split_double_page {
                        "Yes"
                    } else {
                        "No"
                    },
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" [s]"),
            ]),
            Line::from(vec![
                Span::raw("Auto Crop: "),
                Span::styled(
                    if self.state.config.auto_crop {
                        "Yes"
                    } else {
                        "No"
                    },
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" [c]"),
            ]),
            self.render_setting_line(
                "Quality",
                &format!("{:3}", self.state.config.compression_quality),
                "u",
                self.state.selected_field == Some(SelectedField::Quality),
            ),
            self.render_setting_line(
                "Brightness",
                &format!("{:4}", self.state.config.brightness.unwrap_or(-10)),
                "b",
                self.state.selected_field == Some(SelectedField::Brightness),
            ),
            self.render_setting_line(
                "Contrast",
                &format!("{:3.1}", self.state.config.contrast.unwrap_or(1.0)),
                "t",
                self.state.selected_field == Some(SelectedField::Contrast),
            ),
            Line::from(""),
            Line::from(vec![
                Span::raw("Device: "),
                Span::styled(
                    format!(
                        "{}x{}",
                        self.state.config.device_dimensions.0,
                        self.state.config.device_dimensions.1
                    ),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
            Line::from(""),
        ];

        let paragraph = Paragraph::new(settings_text);
        paragraph.render(inner, buf);

        // Render the process button at the bottom of the settings area
        let button_height = 3;
        let button_y = inner.y + inner.height.saturating_sub(button_height + 1);
        let button_area = Rect::new(inner.x, button_y, inner.width, button_height);

        ButtonWidget::new("Start Processing".to_string(), || {
            self.state.send_start_processing();
        })
        .style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .with_mouse_event(self.state.last_mouse_click)
        .render(button_area, buf);

        // Render editing overlay if editing prefix
        if let Some(SelectedField::Prefix) = self.state.selected_field {
            if !self.state.input_buffer.is_empty()
                || self.state.selected_field == Some(SelectedField::Prefix)
            {
                let popup_area = centered_rect(50, 20, area);
                Clear.render(popup_area, buf);

                let popup = Paragraph::new(self.state.input_buffer.as_str()).block(
                    Block::default()
                        .title("Edit Title Prefix")
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::Yellow)),
                );
                popup.render(popup_area, buf);
            }
        }
    }
}

fn find_manga_files(dir: &str) -> anyhow::Result<Vec<MangaFile>> {
    let mut files = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if let Some(ext) = path.extension() {
            if matches!(
                ext.to_str(),
                Some("cbz") | Some("cbr") | Some("zip") | Some("rar")
            ) {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                files.push(MangaFile { path, name });
            }
        }
    }

    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
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
        let start = Instant::now();
        let block = Block::default()
            .title("Preview")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default());

        let inner = block.inner(area);
        block.render(area, buf);

        if self.state.preview_state.loading {
            let msg = Paragraph::new("Loading preview...")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Yellow));
            msg.render(inner, buf);
        } else {
            // Render using ThreadProtocol
            let image = StatefulImage::new().resize(Resize::Scale(None));
            StatefulWidget::render(
                image,
                inner,
                buf,
                &mut self.state.preview_state.thread_protocol,
            );
        }
        if start.elapsed() > std::time::Duration::from_millis(100) {
            log::error!("PreviewWidget render took {:?}", start.elapsed());
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
        // Check for preview requests
        if let Some(request) = get_latest(&rx) {
            match request {
                PreviewRequest::LoadFile { path, config } => rayon::spawn({
                    let tx = tx.clone();
                    move || {
                        let result = load_and_process_preview(&path, &config);

                        match result {
                            Ok(img) => {
                                let _ = tx
                                    .send(crate::Event::ConfigEvent(ConfigEvent::ImageLoaded(img)));
                            }
                            Err(e) => {
                                let _ = tx.send(crate::Event::ConfigEvent(ConfigEvent::Error(
                                    e.to_string(),
                                )));
                            }
                        }
                    }
                }),
            }
        }

        // Check for resize requests
        if let Some(resize_request) = get_latest(&resize_rx) {
            log::info!("Processing resize request");
            rayon::spawn({
                let tx = tx.clone();
                move || {
                    let result = resize_request.resize_encode();
                    let _ = tx.send(crate::Event::ConfigEvent(ConfigEvent::ResizeComplete(
                        result,
                    )));
                }
            });
        }

        // Small sleep to prevent busy waiting
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn load_and_process_preview(
    path: &PathBuf,
    config: &ComicConfig,
) -> anyhow::Result<image::DynamicImage> {
    let config = ComicConfig {
        device_dimensions: (600, 800),
        ..config.clone()
    };

    // Load first image from archive
    let mut files = comic_archive::unarchive_comic_iter(path)?;
    let archive_file = files
        .next()
        .ok_or_else(|| anyhow::anyhow!("No images in archive"))??;

    let img = image::load_from_memory(&archive_file.data)?;

    // Process using the same pipeline as the main processing
    let processed_images = crate::image_processor::process_image(img, &config);

    // Take the first processed image and convert back to DynamicImage
    let first_image = processed_images
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No processed images"))?;

    Ok(image::DynamicImage::ImageLuma8(first_image))
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn get_latest<T>(rx: &mpsc::Receiver<T>) -> Option<T> {
    let mut latest = None;
    while let Ok(event) = rx.try_recv() {
        latest = Some(event);
    }
    latest
}

pub struct ButtonWidget<F> {
    pub text: String,
    pub style: Style,
    pub mouse_event: Option<MouseEvent>,
    pub on_click: F,
}

impl<F> ButtonWidget<F>
where
    F: FnOnce(),
{
    pub fn new(text: String, on_click: F) -> Self {
        Self {
            text,
            style: Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            mouse_event: None,
            on_click,
        }
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn with_mouse_event(mut self, mouse_event: Option<MouseEvent>) -> Self {
        self.mouse_event = mouse_event;
        self
    }
}

impl<F> Widget for ButtonWidget<F>
where
    F: FnOnce(),
{
    fn render(self, area: Rect, buf: &mut Buffer) {
        let button_text = format!(" {} ", self.text);
        let button_width = button_text.len() as u16 + 10;
        let button_height = 3;

        // Center the button in the given area
        let button_x = area.x + (area.width.saturating_sub(button_width)) / 2;
        let button_y = area.y + (area.height.saturating_sub(button_height)) / 2;
        let button_area = Rect::new(button_x, button_y, button_width, button_height);

        // Draw the button
        let button_block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.style);

        let button_inner = button_block.inner(button_area);
        button_block.render(button_area, buf);

        // Check for click event during render (immediate-mode pattern)
        if let Some(event) = self.mouse_event {
            if button_area.contains(Position::new(event.column, event.row)) {
                (self.on_click)();
            }
        }

        Paragraph::new(button_text)
            .style(self.style)
            .alignment(Alignment::Center)
            .render(button_inner, buf);
    }
}
