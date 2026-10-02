//! Getting the embedding model's files into the cache: each file the cache
//! lacks (or holds damaged) is downloaded from the pinned revision, checked
//! against its pinned size and SHA-256, and moved into place whole. Only
//! the embedder calls this, in the background; a search never downloads.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chatgpt_embed::model::{self, FILES, ModelFile};

/// Per file. The model is 23 MB.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Where tests serve the files from instead of Hugging Face (debug builds
/// only).
const BASE_URL_ENV: &str = "CHATGPT_MODEL_BASE_URL";

/// The pinned revision's directory in the model cache.
pub fn dir() -> Result<PathBuf, String> {
    model::cache_root()
        .map(|root| model::revision_dir(&root))
        .ok_or_else(|| "no home directory to keep the embedding model in".to_owned())
}

fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .filter(|url| cfg!(debug_assertions) && !url.is_empty())
        .unwrap_or_else(|| model::DOWNLOAD_BASE.to_owned())
}

/// Make sure every file is in `dir` and matches its checksum, downloading
/// what isn't. The error says why, without any file content.
pub async fn ensure(dir: &Path) -> Result<(), String> {
    for file in FILES {
        let check_dir = dir.to_path_buf();
        let verified = tokio::task::spawn_blocking(move || model::verify_file(&check_dir, &file))
            .await
            .map_err(|error| error.to_string())?;
        if verified.is_ok() {
            continue;
        }
        download(dir, &file).await?;
    }
    Ok(())
}

async fn download(dir: &Path, file: &ModelFile) -> Result<(), String> {
    let url = file.url(&base_url());
    tracing::info!(file = file.path, "downloading the embedding model");
    let bytes = chatgpt::http::download(&url, DOWNLOAD_TIMEOUT)
        .await
        .map_err(|error| format!("couldn't download {}: {error}", file.path))?;
    if !model::matches(file, &bytes) {
        return Err(format!(
            "the downloaded {} doesn't match its pinned checksum",
            file.path
        ));
    }
    let target = dir.join(file.path);
    let partial = target.with_extension(format!("partial-{}", std::process::id()));
    // On disk in full before it takes the real name.
    let written = target
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::File::create(&partial))
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, &bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&partial, &target));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("couldn't save {}: {error}", target.display()));
    }
    tracing::info!(file = file.path, "downloaded the embedding model");
    Ok(())
}
