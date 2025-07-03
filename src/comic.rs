use crate::Event;
use std::{
    fs,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub enum ComicStage {
    Extract,
    Process,
    Epub,
    Mobi,
}

impl std::fmt::Display for ComicStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComicStage::Extract => write!(f, "extract"),
            ComicStage::Process => write!(f, "process"),
            ComicStage::Epub => write!(f, "epub"),
            ComicStage::Mobi => write!(f, "mobi"),
        }
    }
}

#[derive(Debug)]
pub enum ComicStatus {
    Waiting,
    Progress {
        stage: ComicStage,
        progress: f64,
        start: Instant,
    },
    ImageProcessingStart {
        total_images: usize,
        start: Instant,
    },
    ImageProcessed,
    ImageProcessingComplete {
        duration: Duration,
    },
    StageCompleted {
        stage: ComicStage,
        duration: Duration,
    },
    Success,
    Failed {
        error: anyhow::Error,
    },
}

pub enum ProgressEvent {
    RegisterComic { id: usize, file_name: String },
    ComicUpdate { id: usize, status: ComicStatus },
    ProcessingComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SplitStrategy {
    None,
    Split,
    Rotate,
    RotateAndSplit,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComicConfig {
    pub device_dimensions: (u32, u32),
    pub right_to_left: bool,
    pub split: SplitStrategy,
    pub auto_crop: bool,
    pub compression_quality: u8,
    pub brightness: i32,
    // Gamma correction: 0.0-3.0
    pub gamma: f32,
}

impl Default for ComicConfig {
    fn default() -> Self {
        Self {
            device_dimensions: (1236, 1648),
            right_to_left: true,
            split: SplitStrategy::RotateAndSplit,
            auto_crop: true,
            compression_quality: 85,
            brightness: -10,
            gamma: 1.8,
        }
    }
}

impl ComicConfig {
    const CONFIG_PATH: &str = ".comically.config";

    pub fn load() -> Option<Self> {
        let config_path = std::env::home_dir()?.join(Self::CONFIG_PATH);

        fs::read_to_string(&config_path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
    }

    pub fn save(&self) -> Option<()> {
        let config_path = std::env::home_dir()?.join(Self::CONFIG_PATH);

        serde_json::to_string_pretty(self)
            .ok()
            .and_then(|json| fs::write(&config_path, json).ok())
    }
}

#[derive(Debug, Clone)]
pub struct ProcessedImage {
    pub path: PathBuf,
    pub dimensions: (u32, u32),
}

pub struct Comic {
    pub id: usize,
    pub tx: mpsc::Sender<Event>,
    pub temp_dir: tempfile::TempDir,
    pub processed_dir: PathBuf,
    pub processed_files: Vec<ProcessedImage>,
    pub title: String,
    pub prefix: Option<String>,
    pub input: PathBuf,
    pub config: ComicConfig,
}

impl Drop for Comic {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

impl Comic {
    pub fn new(
        id: usize,
        file: PathBuf,
        title_prefix: Option<&str>,
        title: String,
        mut config: ComicConfig,
        tx: mpsc::Sender<Event>,
    ) -> anyhow::Result<Self> {
        config.compression_quality = config.compression_quality.clamp(0, 100);

        let title_prefix = title_prefix
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from);

        let full_title = match &title_prefix {
            Some(prefix) => format!("{} {}", prefix, title),
            _ => title,
        };

        let temp_dir = tempfile::tempdir()?;

        let comic = Comic {
            id,
            tx,
            processed_dir: temp_dir.path().join("Processed"),
            temp_dir,
            processed_files: Vec::new(),
            title: full_title,
            prefix: title_prefix,
            input: file,
            config,
        };

        std::fs::create_dir_all(comic.processed_dir())?;

        Ok(comic)
    }

    pub fn with_try<F, T>(&mut self, f: F) -> Option<T>
    where
        F: FnOnce(&mut Comic) -> anyhow::Result<T>,
    {
        let result = f(self);
        match result {
            Ok(t) => Some(t),
            Err(e) => {
                log::error!("Error in comic: {} {e}", self.title);
                self.failed(e);
                None
            }
        }
    }

    pub fn processed_dir(&self) -> &std::path::Path {
        &self.processed_dir
    }

    pub fn epub_dir(&self) -> PathBuf {
        self.temp_dir.path().join("EPUB")
    }

    pub fn epub_file(&self) -> PathBuf {
        self.epub_dir().join("book.epub")
    }

    pub fn output_mobi(&self) -> PathBuf {
        let mut path = self.input.clone();
        if let Some(prefix) = &self.prefix {
            path.set_file_name(format!(
                "{}_{}",
                prefix,
                path.file_stem().unwrap().to_string_lossy()
            ));
        }
        path.set_extension("mobi");
        path
    }

    pub fn update_status(&self, stage: ComicStage, progress: f64) -> Instant {
        let start = Instant::now();
        self.notify(ProgressEvent::ComicUpdate {
            id: self.id,
            status: ComicStatus::Progress {
                stage,
                progress,
                start,
            },
        });
        start
    }

    pub fn stage_completed(&self, stage: ComicStage, duration: Duration) {
        self.notify(ProgressEvent::ComicUpdate {
            id: self.id,
            status: ComicStatus::StageCompleted { stage, duration },
        });
    }

    pub fn success(&self) {
        self.notify(ProgressEvent::ComicUpdate {
            id: self.id,
            status: ComicStatus::Success,
        });
    }

    pub fn failed(&self, error: anyhow::Error) {
        self.notify(ProgressEvent::ComicUpdate {
            id: self.id,
            status: ComicStatus::Failed { error },
        });
    }

    pub fn image_processing_start(&self, total_images: usize) -> Instant {
        let start = Instant::now();
        self.notify(ProgressEvent::ComicUpdate {
            id: self.id,
            status: ComicStatus::ImageProcessingStart {
                total_images,
                start,
            },
        });
        start
    }

    pub fn image_processing_complete(&self, duration: Duration) {
        self.notify(ProgressEvent::ComicUpdate {
            id: self.id,
            status: ComicStatus::ImageProcessingComplete { duration },
        });
    }

    fn notify(&self, event: ProgressEvent) {
        let _ = self.tx.send(Event::Progress(event));
    }
}
