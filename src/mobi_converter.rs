use anyhow::{Context, Result};
use std::fs;
use std::process::Command;

use crate::Comic;

/// Converts an EPUB file to MOBI using Amazon's KindleGen
pub fn create_mobi(comic: &Comic) -> Result<()> {
    let epub_path = comic.epub_file();
    if !epub_path.exists() {
        anyhow::bail!("EPUB file does not exist: {}", epub_path.display());
    }

    let output_path = comic.output_mobi();

    // Check if kindlegen is available
    if !is_kindlegen_available() {
        anyhow::bail!("KindleGen is not found in PATH. Please install KindleGen and make sure it's in your PATH.");
    }

    // Run kindlegen
    let output = Command::new("kindlegen")
        .arg("-dont_append_source")
        .arg("-c1")
        .arg("-locale")
        .arg("en")
        .arg(&epub_path)
        .output()
        .context("Failed to execute KindleGen")?;

    // Check output
    let output_str = String::from_utf8_lossy(&output.stdout);

    let has_error_output = output_str.lines().any(|line| line.starts_with("Error("));

    if !output.status.success() || has_error_output {
        let code = output.status.code();
        anyhow::bail!("KindleGen failed with code {:?}: {}", code, output_str);
    }

    // KindleGen creates the mobi file in the same directory as the epub
    let mobi_path = epub_path.with_extension("mobi");

    if mobi_path != output_path {
        fs::rename(&mobi_path, &output_path).context(format!(
            "Failed to move MOBI file from {} to {}",
            mobi_path.display(),
            output_path.display()
        ))?;
    }

    log::debug!("MOBI creation successful: {}", output_path.display());

    Ok(())
}

/// Checks if KindleGen is available in the PATH
fn is_kindlegen_available() -> bool {
    match Command::new("kindlegen").arg("-version").output() {
        Ok(_) => true,
        Err(_) => false,
    }
}
