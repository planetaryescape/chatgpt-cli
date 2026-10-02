//! The model files: which ones, at which revision, with which checksums,
//! and where they're cached. The cache is the TS CLI's
//! (`src/search/embeddings.ts` @ 1b8c950, which lets Transformers.js lay it
//! out as `<cache>/chatgpt-cli/models/<model>/<revision>/<file>`), so a
//! machine where the TS CLI already ran `search-index` downloads nothing.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const MODEL_ID: &str = "Xenova/all-MiniLM-L6-v2";
pub const MODEL_REVISION: &str = "751bff37182d3f1213fa05d7196b954e230abad9";

/// Where the files come from: `<base>/<model>/resolve/<revision>/<file>`.
pub const DOWNLOAD_BASE: &str = "https://huggingface.co";

/// One file the embedder needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// Relative to the revision's directory, and to the download URL.
    pub path: &'static str,
    pub size: u64,
    /// Hex SHA-256. The ONNX file's is Hugging Face's LFS id for it; the
    /// tokenizer's was taken from the file the TS CLI downloaded.
    pub sha256: &'static str,
}

pub const MODEL_FILE: ModelFile = ModelFile {
    path: "onnx/model_quantized.onnx",
    size: 22_972_370,
    sha256: "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1",
};

pub const TOKENIZER_FILE: ModelFile = ModelFile {
    path: "tokenizer.json",
    size: 711_661,
    sha256: "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0",
};

pub const FILES: [ModelFile; 2] = [MODEL_FILE, TOKENIZER_FILE];

impl ModelFile {
    pub fn url(&self, base: &str) -> String {
        format!(
            "{}/{MODEL_ID}/resolve/{MODEL_REVISION}/{}",
            base.trim_end_matches('/'),
            self.path
        )
    }
}

/// `$XDG_CACHE_HOME/chatgpt-cli/models`, else `~/.cache/chatgpt-cli/models`,
/// as the TS CLI picks it.
pub fn cache_root() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cache")))?;
    Some(base.join("chatgpt-cli").join("models"))
}

/// The pinned revision's directory under `root`.
pub fn revision_dir(root: &Path) -> PathBuf {
    root.join(MODEL_ID).join(MODEL_REVISION)
}

/// Why a file can't be used.
#[derive(Debug, thiserror::Error)]
pub enum FileProblem {
    #[error("{path} is missing")]
    Missing { path: String },
    #[error("{path} can't be read: {source}")]
    Unreadable {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is {found} bytes, not the expected {expected}")]
    WrongSize {
        path: String,
        found: u64,
        expected: u64,
    },
    #[error("{path} doesn't match its pinned SHA-256")]
    WrongChecksum { path: String },
}

/// Whether every file is in `dir` at its expected size: the cheap check
/// before starting a worker, which verifies checksums when it loads.
pub fn present(dir: &Path) -> bool {
    FILES.iter().all(|file| {
        std::fs::metadata(dir.join(file.path)).is_ok_and(|meta| meta.len() == file.size)
    })
}

/// Check one file's size and checksum.
pub fn verify_file(dir: &Path, file: &ModelFile) -> Result<(), FileProblem> {
    let path = dir.join(file.path);
    let shown = path.display().to_string();
    let mut handle = match std::fs::File::open(&path) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(FileProblem::Missing { path: shown });
        }
        Err(source) => {
            return Err(FileProblem::Unreadable {
                path: shown,
                source,
            });
        }
    };
    let found = handle
        .metadata()
        .map_err(|source| FileProblem::Unreadable {
            path: shown.clone(),
            source,
        })?
        .len();
    if found != file.size {
        return Err(FileProblem::WrongSize {
            path: shown,
            found,
            expected: file.size,
        });
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = handle
            .read(&mut buffer)
            .map_err(|source| FileProblem::Unreadable {
                path: shown.clone(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hex(&hasher.finalize()) != file.sha256 {
        return Err(FileProblem::WrongChecksum { path: shown });
    }
    Ok(())
}

/// Check bytes against a file's pinned size and checksum, before they're
/// written to the cache.
pub fn matches(file: &ModelFile, bytes: &[u8]) -> bool {
    bytes.len() as u64 == file.size && hex(&Sha256::digest(bytes)) == file.sha256
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_point_at_the_pinned_revision() {
        assert_eq!(
            MODEL_FILE.url("https://huggingface.co/"),
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/751bff37182d3f1213fa05d7196b954e230abad9/onnx/model_quantized.onnx"
        );
    }

    #[test]
    fn files_are_checked_by_size_and_checksum() {
        let dir = tempfile::tempdir().expect("dir");
        let file = ModelFile {
            path: "a/b.bin",
            size: 3,
            // SHA-256 of "abc".
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        assert!(matches!(
            verify_file(dir.path(), &file),
            Err(FileProblem::Missing { .. })
        ));
        std::fs::create_dir_all(dir.path().join("a")).expect("mkdir");
        std::fs::write(dir.path().join("a/b.bin"), "abcd").expect("write");
        assert!(matches!(
            verify_file(dir.path(), &file),
            Err(FileProblem::WrongSize { .. })
        ));
        std::fs::write(dir.path().join("a/b.bin"), "abd").expect("write");
        assert!(matches!(
            verify_file(dir.path(), &file),
            Err(FileProblem::WrongChecksum { .. })
        ));
        std::fs::write(dir.path().join("a/b.bin"), "abc").expect("write");
        assert!(verify_file(dir.path(), &file).is_ok());
        assert!(matches(&file, b"abc"));
        assert!(!matches(&file, b"abd"));
    }
}
