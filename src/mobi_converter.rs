use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Converts an EPUB file to MOBI using Amazon's KindleGen
pub fn create_mobi(epub_path: &Path, output_path: &Path) -> Result<()> {
    info!(
        "Converting EPUB to MOBI: {} -> {}",
        epub_path.display(),
        output_path.display()
    );

    // Check if kindlegen is available
    if !is_kindlegen_available() {
        anyhow::bail!("KindleGen is not found in PATH. Please install KindleGen and make sure it's in your PATH.");
    }

    // Run kindlegen
    let output = Command::new("kindlegen")
        .arg("-dont_append_source")
        .arg("-locale")
        .arg("en")
        .arg(epub_path)
        .output()
        .context("Failed to execute KindleGen")?;

    // Check output
    let output_str = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() && !output_str.contains("Warnings") {
        let code = output.status.code();
        anyhow::bail!("KindleGen failed with code {:?}: {}", code, output_str);
    }

    // KindleGen creates the mobi file in the same directory as the epub
    let mobi_path = epub_path.with_extension("mobi");

    // Rename to the desired output path if needed
    if mobi_path != output_path {
        fs::rename(&mobi_path, output_path).context(format!(
            "Failed to move MOBI file from {} to {}",
            mobi_path.display(),
            output_path.display()
        ))?;
    }

    info!("MOBI creation successful: {}", output_path.display());

    Ok(())
}

/// Checks if KindleGen is available in the PATH
fn is_kindlegen_available() -> bool {
    match Command::new("kindlegen").arg("-version").output() {
        Ok(_) => true,
        Err(_) => false,
    }
}
