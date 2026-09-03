use anyhow::Context;
use std::path::Path;
use tracing::info;

/// Ensure PNG bytes are on disk.
///
/// Must run **before** the path is pasted: CLI agents (Claude Code / Codex / etc.)
/// check whether the file exists the instant they receive the pasted text, and only
/// then render it as an `[Image #n]` attachment. The old "paste the path first,
/// write the file in the background" order could leave a raw `/mnt/...` path in
/// the input box when the system was slow (issue #4).
///
/// Repeat pastes with an unchanged clipboard (cache hit) skip the rewrite if the
/// file already exists.
pub async fn ensure_saved(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create temp directory")?;
    }

    let tmp_path = path.with_extension("png.partial");
    tokio::fs::write(&tmp_path, data)
        .await
        .context("failed to write image file")?;
    tokio::fs::rename(&tmp_path, path)
        .await
        .context("failed to commit image file")?;

    info!("image saved: {}", path.display());
    Ok(())
}
