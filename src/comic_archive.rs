use anyhow::Context;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;
use unrar::Archive;
use zip::ZipArchive;

#[derive(Debug)]
pub struct ArchiveFile {
    pub file_name: String,
    pub data: Vec<u8>,
}

pub trait ArchiveReader: Send {
    fn next_file(&mut self) -> anyhow::Result<Option<ArchiveFile>>;
}

pub struct ZipReader {
    index: usize,
    archive: ZipArchive<BufReader<File>>,
}

impl ZipReader {
    fn new(file: File) -> anyhow::Result<Self> {
        let reader = BufReader::new(file);
        let archive = ZipArchive::new(reader).context("Failed to parse file as zip archive")?;
        Ok(Self { index: 0, archive })
    }
}

impl ArchiveReader for ZipReader {
    fn next_file(&mut self) -> anyhow::Result<Option<ArchiveFile>> {
        while self.index < self.archive.len() {
            let current_index = self.index;
            self.index += 1;

            let mut file = match self.archive.by_index(current_index) {
                Ok(f) => f,
                Err(e) => return Err(e.into()),
            };

            if file.is_dir() {
                continue;
            }

            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };

            let file_name = match get_file_name(&outpath) {
                Some(name) => name,
                None => continue,
            };

            let mut data = Vec::new();
            io::Read::read_to_end(&mut file, &mut data).context("Failed to read file")?;

            return Ok(Some(ArchiveFile { file_name, data }));
        }

        Ok(None)
    }
}

pub struct RarReader {
    archive: Option<unrar::OpenArchive<unrar::Process, unrar::CursorBeforeHeader>>,
    finished: bool,
}

// whoops
unsafe impl Send for RarReader {}

impl RarReader {
    fn new(path: &Path) -> anyhow::Result<Self> {
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid path"))?;
        let archive = Archive::new(path_str)
            .open_for_processing()
            .context("Failed to open RAR file")?;
        Ok(Self {
            archive: Some(archive),
            finished: false,
        })
    }
}

impl ArchiveReader for RarReader {
    fn next_file(&mut self) -> anyhow::Result<Option<ArchiveFile>> {
        if self.finished {
            return Ok(None);
        }

        let archive = match self.archive.take() {
            Some(archive) => archive,
            None => {
                self.finished = true;
                return Ok(None);
            }
        };

        match archive.read_header()? {
            Some(header) => {
                let file_path = Path::new(&header.entry().filename);

                if header.entry().is_directory() {
                    self.archive = Some(header.skip()?);
                    return self.next_file();
                }

                let Some(file_name) = get_file_name(file_path) else {
                    self.archive = Some(header.skip()?);
                    return self.next_file();
                };

                let (data, new_archive) = header.read()?;
                self.archive = Some(new_archive);

                Ok(Some(ArchiveFile { file_name, data }))
            }
            None => {
                self.finished = true;
                Ok(None)
            }
        }
    }
}

pub struct ArchiveIter {
    reader: Box<dyn ArchiveReader>,
}

impl Iterator for ArchiveIter {
    type Item = anyhow::Result<ArchiveFile>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.next_file() {
            Ok(Some(file)) => Some(Ok(file)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

pub fn unarchive_comic_iter(
    comic_file: impl AsRef<Path>,
) -> anyhow::Result<impl Iterator<Item = anyhow::Result<ArchiveFile>> + Send> {
    let path = comic_file.as_ref();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let reader: Box<dyn ArchiveReader> = match ext.as_str() {
        "cbz" | "zip" => {
            let file = File::open(path).context("Failed to open zip file")?;
            Box::new(ZipReader::new(file)?)
        }
        "cbr" | "rar" => Box::new(RarReader::new(path)?),
        _ => anyhow::bail!("Unsupported archive format: {}", ext),
    };

    Ok(ArchiveIter { reader })
}

fn get_file_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|f| f.to_string_lossy())
        .filter(|f| !should_skip_file(&f))
        .filter(|_| has_image_extension(path))
        .map(|f| f.to_string())
}

fn should_skip_file(file_name: &str) -> bool {
    file_name.starts_with(".")
        || file_name.contains("__MACOSX")
        || file_name.contains("thumbs.db")
        || file_name.contains(".DS_Store")
}

fn has_image_extension(path: &Path) -> bool {
    static VALID_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        for valid_ext in VALID_EXTENSIONS {
            if valid_ext == &ext_str {
                return true;
            }
        }
    }
    false
}

#[ignore]
#[test]
fn test_unarchive_comic_iter() {
    let files = unarchive_comic_iter(std::path::PathBuf::from("v12.cbz"))
        .unwrap()
        .collect::<Vec<_>>();
    println!("{:?}", files.len());
}
