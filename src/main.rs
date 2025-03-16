use clap::Parser;
use std::path::PathBuf;

mod comic_archive;
mod epub_builder;
mod image_processor;
mod mobi_converter;

#[derive(Parser, Debug)]
#[command(
    name = "comically",
    about = "A simple converter for comic book files to Kindle MOBI format",
    version
)]
struct Cli {
    /// Input file path (CBZ format)
    #[arg(required = true)]
    input: PathBuf,

    /// Output file path (if not specified, will use input filename with .mobi extension)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Keep temporary files (useful for debugging)
    #[arg(long)]
    keep_temp: bool,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    // Create default output path if not provided
    let output = match cli.output {
        Some(path) => path,
        None => {
            let mut path = cli.input.clone();
            path.set_extension("mobi");
            path
        }
    };

    println!("Converting {} to {}", cli.input.display(), output.display());

    // Extract CBZ to temporary directory
    let temp_dir = tempfile::tempdir()?;
    let extract_path = time_it("Extracting CBZ", || {
        comic_archive::extract_cbz(&cli.input, temp_dir.path())
    })?;

    // Process images
    let processed_path = time_it("Processing Images", || {
        image_processor::process_images(extract_path)
    })?;

    // Create EPUB
    let epub_path = time_it("Creating EPUB", || epub_builder::build_epub(processed_path))?;

    // Convert to MOBI
    time_it("Creating MOBI", || {
        mobi_converter::create_mobi(&epub_path, &output)
    })?;

    // Clean up unless we want to keep temp files
    if !cli.keep_temp {
        temp_dir.close()?;
    }

    println!("Conversion completed successfully!");
    Ok(())
}

fn time_it<F, T>(label: &str, func: F) -> T
where
    F: FnOnce() -> T,
{
    let start = std::time::Instant::now();
    let result = func();
    let duration = start.elapsed();
    log::info!("{}: {:?}", label, duration.as_millis());
    result
}
