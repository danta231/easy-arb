use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{RuntimeError, RuntimeResult};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn write_utf8(path: PathBuf, contents: &str) -> RuntimeResult<()> {
    fs::write(&path, contents).map_err(|error| RuntimeError::Io {
        path,
        message: error.to_string(),
    })
}

pub(super) fn ensure_dir(path: &Path) -> RuntimeResult<()> {
    fs::create_dir_all(path).map_err(|error| RuntimeError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

pub(super) fn write_utf8_in_dir(
    dir: &Path,
    file_name: &str,
    contents: &str,
) -> RuntimeResult<PathBuf> {
    ensure_dir(dir)?;
    let path = dir.join(file_name);
    write_utf8(path.clone(), contents)?;
    Ok(path)
}

#[cfg(feature = "live-exec")]
pub(super) fn write_utf8_with_parent(path: PathBuf, contents: &str) -> RuntimeResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_dir(parent)?;
        }
    }
    write_utf8(path, contents)
}

pub(super) fn read_utf8(path: &Path) -> RuntimeResult<String> {
    fs::read_to_string(path).map_err(|error| RuntimeError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

pub(super) struct RuntimeTempDir {
    path: PathBuf,
}

impl RuntimeTempDir {
    pub(super) fn new() -> RuntimeResult<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("arb-runtime-s9-{}-{counter}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| RuntimeError::Io {
                path: path.clone(),
                message: error.to_string(),
            })?;
        }
        fs::create_dir_all(&path).map_err(|error| RuntimeError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RuntimeTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
