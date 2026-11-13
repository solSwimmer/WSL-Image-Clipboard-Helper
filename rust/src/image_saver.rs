use anyhow::Context;
use std::path::Path;
use tracing::info;

/// Ensure the PNG data is written to disk.
///
/// Must be called **before** pasting the path: the moment a CLI agent (Claude Code / Codex, etc.) receives the pasted text
/// it checks whether the file exists, and only then renders it as an `[Image #n]` attachment;
/// the old "paste the path first, write the file in the background" order could let the file land after that check when the system was busy,
/// leaving the raw `/mnt/...` path in the input box (issue #4).
///
/// Repeat pastes with an unchanged clipboard (cache hit) already have the file and skip the rewrite.
pub async fn ensure_saved(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create temp directory")?;
    }

    let tmp_path = path.with_extension("png.partial");
    tokio::fs::write(&tmp_path, data)
        .await
        .context("Failed to write image file")?;
    tokio::fs::rename(&tmp_path, path)
        .await
        .context("Failed to commit image file")?;

    info!("Image saved: {}", path.display());
    Ok(())
}
