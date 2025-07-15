use std::{
    borrow::Cow,
    fs,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::Event;

#[derive(Debug, Clone, Copy)]
pub enum ComicStage {
    Process,
    Package, // Building the output format (EPUB/CBZ)
    Convert, // Converting EPUB to MOBI (only for MOBI output)
}

impl std::fmt::Display for ComicStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComicStage::Process => write!(f, "process"),
            ComicStage::Package => write!(f, "package"),
            ComicStage::Convert => write!(f, "convert"),
        }
    }
}

impl OutputFormat {
    pub fn stage_weight(&self, stage: ComicStage) -> f64 {
        match (self, stage) {
            // MOBI format weights
            (OutputFormat::Mobi, ComicStage::Process) => 0.5,
            (OutputFormat::Mobi, ComicStage::Package) => 0.05, // EPUB building
            (OutputFormat::Mobi, ComicStage::Convert) => 0.4,  // EPUB to MOBI conversion

            // EPUB format weights
            (OutputFormat::Epub, ComicStage::Process) => 0.8,
            (OutputFormat::Epub, ComicStage::Package) => 0.1, // EPUB building
            (OutputFormat::Epub, ComicStage::Convert) => 0.0, // Not used

            // CBZ format weights
            (OutputFormat::Cbz, ComicStage::Process) => 0.85,
            (OutputFormat::Cbz, ComicStage::Package) => 0.05, // CBZ building
            (OutputFormat::Cbz, ComicStage::Convert) => 0.0,  // Not used
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
pub enum OutputFormat {
    Mobi,
    Epub,
    Cbz,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PngCompression {
    Fast,
    Default,
    Best,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ImageFormat {
    Jpeg { quality: u8 },
    Png { compression: PngCompression },
    WebP { quality: u8 },
}

impl ImageFormat {
    pub fn cycle(&self) -> Self {
        match self {
            ImageFormat::Jpeg { .. } => ImageFormat::Png {
                compression: PngCompression::Default,
            },
            ImageFormat::Png { .. } => ImageFormat::WebP { quality: 85 },
            ImageFormat::WebP { .. } => ImageFormat::Jpeg { quality: 85 },
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg { .. } => "jpg",
            ImageFormat::Png { .. } => "png",
            ImageFormat::WebP { .. } => "webp",
        }
    }

    pub fn adjust_quality(&mut self, increase: bool, fine: bool) {
        let step = if fine { 1 } else { 5 };
        match self {
            ImageFormat::Jpeg { quality } | ImageFormat::WebP { quality } => {
                if increase {
                    *quality = (*quality + step).min(100);
                } else {
                    *quality = quality.saturating_sub(step);
                }
            }
            ImageFormat::Png { compression } => {
                *compression = if increase {
                    match compression {
                        PngCompression::Fast => PngCompression::Default,
                        PngCompression::Default => PngCompression::Best,
                        PngCompression::Best => PngCompression::Best,
                    }
                } else {
                    match compression {
                        PngCompression::Fast => PngCompression::Fast,
                        PngCompression::Default => PngCompression::Fast,
                        PngCompression::Best => PngCompression::Default,
                    }
                };
            }
        }
    }
}

impl PngCompression {
    pub fn cycle(&self) -> Self {
        match self {
            PngCompression::Fast => PngCompression::Default,
            PngCompression::Default => PngCompression::Best,
            PngCompression::Best => PngCompression::Fast,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComicConfig {
    pub device: DevicePreset,
    pub right_to_left: bool,
    pub split: SplitStrategy,
    pub auto_crop: bool,
    pub compression_quality: u8,
    pub brightness: i32,
    // Gamma correction: 0.0-3.0
    pub gamma: f32,
    pub output_format: OutputFormat,
    pub margin_color: Option<u8>,
    pub image_format: ImageFormat,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DevicePreset {
    pub name: Cow<'static, str>,
    pub dimensions: (u32, u32),
}

impl Default for ComicConfig {
    fn default() -> Self {
        Self {
            device: DevicePreset {
                name: Cow::Borrowed("Kindle PW 11"),
                dimensions: (1236, 1648),
            },
            right_to_left: true,
            split: SplitStrategy::RotateAndSplit,
            auto_crop: true,
            compression_quality: 85,
            brightness: -10,
            gamma: 1.8,
            output_format: OutputFormat::Mobi,
            margin_color: None,
            image_format: ImageFormat::Jpeg { quality: 85 },
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

    pub fn device_dimensions(&self) -> (u32, u32) {
        self.device.dimensions
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
    pub output_dir: PathBuf,
    pub input: PathBuf,
    pub config: ComicConfig,
}

impl std::fmt::Debug for Comic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Comic")
            .field("id", &self.id)
            .field("tx", &self.tx)
            .field("temp_dir", &self.temp_dir)
            .field("processed_dir", &self.processed_dir)
            .field("processed_files", &self.processed_files.len())
            .field("title", &self.title)
            .field("output_dir", &self.output_dir)
            .field("input", &self.input)
            .field("config", &self.config)
            .finish()
    }
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
        output_dir: PathBuf,
        title: String,
        mut config: ComicConfig,
        tx: mpsc::Sender<Event>,
    ) -> anyhow::Result<Self> {
        config.compression_quality = config.compression_quality.clamp(0, 100);

        let temp_dir = tempfile::tempdir()?;

        let comic = Comic {
            id,
            tx,
            processed_dir: temp_dir.path().join("Processed"),
            temp_dir,
            processed_files: Vec::new(),
            title,
            output_dir,
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

    pub fn output_path(&self) -> PathBuf {
        let filename = self.input.file_stem().unwrap().to_string_lossy();

        let extension = match self.config.output_format {
            OutputFormat::Mobi => "mobi",
            OutputFormat::Epub => "epub",
            OutputFormat::Cbz => "cbz",
        };

        // don't use .with_extension() bc it replaces everything after the first dot
        self.output_dir.join(format!("{}.{}", filename, extension))
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

#[test]
fn output_path_with_dots() {
    use std::sync::mpsc;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let output_dir = temp_dir.path().join("output");
    let (tx, _rx) = mpsc::channel();

    let mut config = ComicConfig::default();
    config.output_format = OutputFormat::Cbz;

    let comic = Comic::new(
        0,
        PathBuf::from("Dr. STONE v01 (2018) (Digital) (1r0n).cbz"),
        output_dir.clone(),
        "Dr. STONE v01 (2018) (Digital) (1r0n)".to_string(),
        config,
        tx,
    )
    .unwrap();

    let output_path = comic.output_path();
    assert_eq!(
        output_path,
        output_dir.join("Dr. STONE v01 (2018) (Digital) (1r0n).cbz")
    );

    assert_eq!(
        output_path.file_name().unwrap().to_str().unwrap(),
        "Dr. STONE v01 (2018) (Digital) (1r0n).cbz",
        "filename is preserved"
    );
}
