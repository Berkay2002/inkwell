//! Every file path the compress and OCR commands use that is not a document's own comes from here:
//! a dialog the Rust side shows, or the stored Tesseract location. Nothing is taken from the webview.
//! The one place to switch to the shared path-token registry (`src-tauri/src/paths.rs`).

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use super::configured_tesseract;

/// Asks where to write a PDF (blocking: call off the UI thread). `None` = dismissed.
pub fn choose_output(app: &AppHandle, default: &Path) -> Option<PathBuf> {
    let mut d = app.dialog().file().add_filter("PDF documents", &["pdf"]);
    if let Some(dir) = default.parent().filter(|d| d.is_dir()) {
        d = d.set_directory(dir);
    }
    if let Some(name) = default.file_name() {
        d = d.set_file_name(name.to_string_lossy());
    }
    d.blocking_save_file().and_then(|p| p.into_path().ok())
}

/// Asks for `tesseract.exe` (blocking). `None` = dismissed.
pub fn choose_tesseract(app: &AppHandle) -> Option<PathBuf> {
    app.dialog().file().add_filter("Tesseract", &["exe"]).blocking_pick_file().and_then(|p| p.into_path().ok())
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    tesseract: Option<PathBuf>,
}

/// Settings that hold a path, kept by the Rust side (`ocr.tesseractPath`).
pub struct ToolPaths {
    file: Option<PathBuf>,
    stored: Mutex<Stored>,
}

impl ToolPaths {
    pub fn load(app: &AppHandle) -> Self {
        let file = app.path().app_config_dir().ok().map(|d| d.join("tools.json"));
        let stored = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { file, stored: Mutex::new(stored) }
    }

    pub fn tesseract(&self) -> Option<PathBuf> {
        self.stored.lock().tesseract.clone()
    }

    /// Only something named `tesseract(.exe)` is accepted: the path is run as a program.
    pub fn set_tesseract(&self, path: Option<PathBuf>) -> Result<(), String> {
        if let Some(p) = &path {
            if configured_tesseract(Some(p)).is_none() {
                return Err("that is not tesseract.exe".into());
            }
        }
        let mut s = self.stored.lock();
        s.tesseract = path;
        if let Some(f) = &self.file {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(f, serde_json::to_vec(&*s).unwrap_or_default()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
