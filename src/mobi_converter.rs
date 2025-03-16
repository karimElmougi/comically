use anyhow::{Context, Result};
use log::info;
use std::process::Command;

use crate::Comic;

/// Converts an EPUB file to MOBI using Amazon's KindleGen
pub fn create_mobi(comic: &Comic) -> Result<()> {
    let epub_path = comic.epub_file();
    let output_path = comic.output_mobi();

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
