use std::time::{Duration, Instant};

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

pub fn stage_weight(format: comically::OutputFormat, stage: ComicStage) -> f64 {
    match (format, stage) {
        // MOBI format weights
        (comically::OutputFormat::Mobi, ComicStage::Process) => 0.5,
        (comically::OutputFormat::Mobi, ComicStage::Package) => 0.05, // EPUB building
        (comically::OutputFormat::Mobi, ComicStage::Convert) => 0.4,  // EPUB to MOBI conversion

        // EPUB format weights
        (comically::OutputFormat::Epub, ComicStage::Process) => 0.8,
        (comically::OutputFormat::Epub, ComicStage::Package) => 0.1, // EPUB building
        (comically::OutputFormat::Epub, ComicStage::Convert) => 0.0, // Not used

        // CBZ format weights
        (comically::OutputFormat::Cbz, ComicStage::Process) => 0.85,
        (comically::OutputFormat::Cbz, ComicStage::Package) => 0.05, // CBZ building
        (comically::OutputFormat::Cbz, ComicStage::Convert) => 0.0,  // Not used
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
