use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
    },
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, Resize, StatefulImage};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use crate::{comic_archive, ComicConfig};
use image::GenericImageView;

pub struct ConfigState {
    pub files: Vec<MangaFile>,
    pub selected_files: Vec<bool>,
    pub list_state: ListState,
    pub config: ComicConfig,
    pub prefix: Option<String>,
    pub focus: Focus,
    pub editing_field: Option<EditingField>,
    pub input_buffer: String,
    pub preview_state: PreviewState,
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
pub enum EditingField {
    Prefix,
    Quality,
    Brightness,
    Contrast,
    Width,
    Height,
}

pub struct PreviewState {
    current_file: Option<usize>,
    protocol: Option<StatefulProtocol>,
    preview_rx: mpsc::Receiver<PreviewResult>,
    preview_tx: mpsc::Sender<PreviewRequest>,
    loading: bool,
    last_request: Option<Instant>,
}

enum PreviewRequest {
    LoadFile(PathBuf, ComicConfig, usize), // Add file index to track
}

enum PreviewResult {
    Loaded(StatefulProtocol),
    Error(String),
}

pub enum ConfigAction {
    Continue,
    StartProcessing(Vec<PathBuf>, ComicConfig, Option<String>),
    Quit,
}

impl ConfigState {
    pub fn new() -> anyhow::Result<Self> {
        let files = find_manga_files(".")?;
        let selected_files = vec![true; files.len()]; // Select all by default

        let mut list_state = ListState::default();
        if !files.is_empty() {
            list_state.select(Some(0));
        }

        let has_files = !files.is_empty();

        // Create preview processing thread
        let (preview_tx, worker_rx) = mpsc::channel::<PreviewRequest>();
        let (worker_tx, preview_rx) = mpsc::channel::<PreviewResult>();

        thread::spawn(move || {
            preview_worker(worker_rx, worker_tx);
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
            editing_field: None,
            input_buffer: String::new(),
            preview_state: PreviewState {
                current_file: if has_files { Some(0) } else { None },
                protocol: None,
                preview_rx,
                preview_tx,
                loading: false,
                last_request: None,
            },
        };

        // Request initial preview
        if has_files {
            state.request_preview();
        }

        Ok(state)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ConfigAction {
        if let Some(field) = self.editing_field {
            return self.handle_editing(key, field);
        }

        match key.code {
            KeyCode::Char('q') => ConfigAction::Quit,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::FileList => Focus::Settings,
                    Focus::Settings => Focus::FileList,
                };
                ConfigAction::Continue
            }
            KeyCode::Enter => {
                if self.focus == Focus::Settings {
                    // Start processing
                    let selected_paths: Vec<PathBuf> = self
                        .files
                        .iter()
                        .zip(&self.selected_files)
                        .filter(|(_, selected)| **selected)
                        .map(|(file, _)| file.path.clone())
                        .collect();

                    if !selected_paths.is_empty() {
                        return ConfigAction::StartProcessing(
                            selected_paths,
                            self.config,
                            self.prefix.clone(),
                        );
                    }
                }
                ConfigAction::Continue
            }
            _ => match self.focus {
                Focus::FileList => self.handle_file_list_keys(key),
                Focus::Settings => self.handle_settings_keys(key),
            },
        }
    }

    fn handle_file_list_keys(&mut self, key: KeyEvent) -> ConfigAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let start = Instant::now();
                if let Some(selected) = self.list_state.selected() {
                    if selected > 0 {
                        self.list_state.select(Some(selected - 1));
                        self.preview_state.current_file = Some(selected - 1);
                        self.request_preview();
                    }
                }
                tracing::info!("Up key handler took {:?}", start.elapsed());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let start = Instant::now();
                if let Some(selected) = self.list_state.selected() {
                    if selected < self.files.len() - 1 {
                        self.list_state.select(Some(selected + 1));
                        self.preview_state.current_file = Some(selected + 1);
                        self.request_preview();
                    }
                }
                tracing::info!("Down key handler took {:?}", start.elapsed());
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
        ConfigAction::Continue
    }

    fn request_preview(&mut self) {
        // Simple debouncing - don't send multiple requests too quickly
        let now = Instant::now();
        if let Some(last) = self.preview_state.last_request {
            if now.duration_since(last).as_millis() < 50 {
                return;
            }
        }

        if let Some(file_idx) = self.preview_state.current_file {
            if let Some(file) = self.files.get(file_idx) {
                self.preview_state.loading = true;
                self.preview_state.last_request = Some(now);
                let _ = self.preview_state.preview_tx.send(PreviewRequest::LoadFile(
                    file.path.clone(),
                    self.config,
                    file_idx,
                ));
            }
        }
    }

    pub fn update_preview(&mut self) {
        // Check for preview results
        while let Ok(result) = self.preview_state.preview_rx.try_recv() {
            self.preview_state.loading = false;
            match result {
                PreviewResult::Loaded(protocol) => {
                    tracing::info!("Received new protocol for preview");
                    self.preview_state.protocol = Some(protocol);
                }
                PreviewResult::Error(err) => {
                    tracing::warn!("Preview error: {}", err);
                    self.preview_state.protocol = None;
                }
            }
        }
    }

    fn handle_settings_keys(&mut self, key: KeyEvent) -> ConfigAction {
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
            KeyCode::Char('q') => {
                self.editing_field = Some(EditingField::Quality);
                self.input_buffer = self.config.compression_quality.to_string();
            }
            KeyCode::Char('b') => {
                self.editing_field = Some(EditingField::Brightness);
                self.input_buffer = self.config.brightness.unwrap_or(-10).to_string();
            }
            KeyCode::Char('t') => {
                self.editing_field = Some(EditingField::Contrast);
                self.input_buffer = self.config.contrast.unwrap_or(1.0).to_string();
            }
            KeyCode::Char('p') => {
                self.editing_field = Some(EditingField::Prefix);
                self.input_buffer = self.prefix.clone().unwrap_or_default();
            }
            _ => {}
        }
        ConfigAction::Continue
    }

    fn handle_editing(&mut self, key: KeyEvent, field: EditingField) -> ConfigAction {
        match key.code {
            KeyCode::Esc => {
                self.editing_field = None;
                self.input_buffer.clear();
            }
            KeyCode::Enter => {
                match field {
                    EditingField::Quality => {
                        if let Ok(val) = self.input_buffer.parse::<u8>() {
                            self.config.compression_quality = val.clamp(0, 100);
                        }
                    }
                    EditingField::Brightness => {
                        if let Ok(val) = self.input_buffer.parse::<i32>() {
                            self.config.brightness = Some(val.clamp(-100, 100));
                            self.request_preview();
                        }
                    }
                    EditingField::Contrast => {
                        if let Ok(val) = self.input_buffer.parse::<f32>() {
                            self.config.contrast = Some(val.clamp(0.5, 2.0));
                            self.request_preview();
                        }
                    }
                    EditingField::Prefix => {
                        self.prefix = if self.input_buffer.is_empty() {
                            None
                        } else {
                            Some(self.input_buffer.clone())
                        };
                    }
                    _ => {}
                }
                self.editing_field = None;
                self.input_buffer.clear();
            }
            KeyCode::Char(c) => {
                match field {
                    EditingField::Prefix => {
                        // Allow any character for prefix
                        self.input_buffer.push(c);
                    }
                    _ => {
                        // Only allow numeric input for other fields
                        if c.is_numeric() || c == '.' || c == '-' {
                            self.input_buffer.push(c);
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            _ => {}
        }
        ConfigAction::Continue
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
        SettingsWidget::new(&self.state).render(main_chunks[1], buf);

        // Preview panel
        PreviewWidget::new(self.state).render(main_chunks[2], buf);

        // Footer
        let footer_text = match self.state.focus {
            Focus::FileList => "↑/↓: Navigate | Space: Toggle | a: Toggle All | Tab: Switch Panel | q: Quit",
            Focus::Settings => "m: Toggle Manga | s: Toggle Split | c: Toggle Crop | p: Prefix | Enter: Process | Tab: Switch Panel",
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
    state: &'a ConfigState,
}

impl<'a> SettingsWidget<'a> {
    fn new(state: &'a ConfigState) -> Self {
        Self { state }
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
            Line::from(vec![
                Span::raw("Quality: "),
                Span::styled(
                    self.state.config.compression_quality.to_string(),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" [q]"),
            ]),
            Line::from(vec![
                Span::raw("Brightness: "),
                Span::styled(
                    self.state.config.brightness.unwrap_or(-10).to_string(),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" [b]"),
            ]),
            Line::from(vec![
                Span::raw("Contrast: "),
                Span::styled(
                    format!("{:.1}", self.state.config.contrast.unwrap_or(1.0)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" [t]"),
            ]),
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
            Line::from(vec![Span::styled(
                "Press Enter to start processing",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )]),
        ];

        let paragraph = Paragraph::new(settings_text);
        paragraph.render(inner, buf);

        // Render editing overlay if active
        if let Some(field) = self.state.editing_field {
            let popup_area = centered_rect(50, 20, area);
            Clear.render(popup_area, buf);

            let title = match field {
                EditingField::Quality => "Edit Quality (0-100)",
                EditingField::Brightness => "Edit Brightness (-100 to 100)",
                EditingField::Contrast => "Edit Contrast (0.5 to 2.0)",
                EditingField::Prefix => "Edit Title Prefix",
                _ => "Edit Value",
            };

            let popup = Paragraph::new(self.state.input_buffer.as_str()).block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .style(Style::default().fg(Color::Yellow)),
            );
            popup.render(popup_area, buf);
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
        // Update preview if needed
        state.update_preview();
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
        } else if let Some(ref mut protocol) = self.state.preview_state.protocol {
            // The image widget
            let render_start = Instant::now();
            // Try different resize options
            let image = StatefulImage::new().resize(Resize::Scale(None));
            // Render with the protocol state
            tracing::info!("Rendering image in area: {:?}", inner);
            StatefulWidget::render(image, inner, buf, protocol);
            tracing::info!("Image render took {:?}", render_start.elapsed());
        } else {
            let msg = Paragraph::new("No preview available\nSelect a file to preview")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            msg.render(inner, buf);
        }
        log::info!("PreviewWidget render took {:?}", start.elapsed());
    }
}
fn preview_worker(rx: mpsc::Receiver<PreviewRequest>, tx: mpsc::Sender<PreviewResult>) {
    let mut picker = Picker::from_query_stdio()
        .inspect(|p| log::info!("Auto Picker: {:?}", p))
        .unwrap_or_else(|_| Picker::from_fontsize((8, 16)));

    log::info!("Picker: {:?}", picker);

    while let Ok(request) = rx.recv() {
        // Clear any pending requests in the queue - we only care about the latest
        let mut latest_request = request;
        while let Ok(newer_request) = rx.try_recv() {
            log::info!("Dropping stale preview request");
            latest_request = newer_request;
        }

        match latest_request {
            PreviewRequest::LoadFile(path, config, file_idx) => {
                log::info!("Processing preview for file {}", file_idx);
                let load_start = Instant::now();
                let result = load_and_process_preview(&path, &config);
                log::info!("Load and process took {:?}", load_start.elapsed());

                match result {
                    Ok(img) => {
                        // Do the protocol encoding in the background thread
                        let encode_start = Instant::now();
                        log::info!("Image dimensions before protocol: {:?}", img.dimensions());
                        let protocol = picker.new_resize_protocol(img);
                        log::info!("Protocol encoding took {:?}", encode_start.elapsed());
                        let _ = tx.send(PreviewResult::Loaded(protocol));
                    }
                    Err(e) => {
                        let _ = tx.send(PreviewResult::Error(e.to_string()));
                    }
                }
            }
        }
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
