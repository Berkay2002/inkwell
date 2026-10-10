//! Disk side of the start screen: file stats for the recent list and the list's own storage.
//! The list lives in `<data dir>/recents.json`; `INKWELL_DATA_DIR` moves it (benchmarks and
//! screenshot runs set it so they never touch the user's recents).

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tauri::Manager;

const FILE: &str = "recents.json";

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    /// Canonical path with the on-disk casing, or the input path when the file is missing.
    pub path: String,
    pub exists: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentsFile {
    /// Raw JSON text, `None` when nothing was saved yet.
    pub json: Option<String>,
    /// True when the data dir came from `INKWELL_DATA_DIR`: the UI then skips importing the old
    /// localStorage list, so an isolated run starts empty.
    pub isolated: bool,
}

/// `canonicalize` on Windows returns `\\?\C:\x` and `\\?\UNC\srv\x`; the UI wants the plain form.
pub fn strip_verbatim(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

pub fn stat_one(path: &str) -> FileStat {
    let missing = || FileStat { path: path.to_string(), exists: false, size: 0, modified: 0 };
    let p = Path::new(path);
    let Ok(meta) = std::fs::metadata(p) else { return missing() };
    if !meta.is_file() {
        return missing();
    }
    let canon = std::fs::canonicalize(p).map(|c| strip_verbatim(&c.to_string_lossy())).unwrap_or_else(|_| path.to_string());
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
    FileStat { path: canon, exists: true, size: meta.len(), modified }
}

fn data_dir(app: &tauri::AppHandle) -> Result<(PathBuf, bool), String> {
    if let Some(d) = std::env::var_os("INKWELL_DATA_DIR").filter(|d| !d.is_empty()) {
        return Ok((PathBuf::from(d), true));
    }
    app.path().app_data_dir().map(|d| (d, false)).map_err(|e| e.to_string())
}

pub fn read_json(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(FILE)).ok()
}

/// Write to a temp file, then rename over the old one: a crash never leaves half a list.
pub fn write_json(dir: &Path, json: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, dir.join(FILE))
}

/// Blocking filesystem work off the UI thread: a dead network share can stall `metadata` for seconds.
async fn blocking<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stat_files(paths: Vec<String>) -> Result<Vec<FileStat>, String> {
    blocking(move || paths.iter().map(|p| stat_one(p)).collect()).await
}

#[tauri::command]
pub async fn recents_load(app: tauri::AppHandle) -> Result<RecentsFile, String> {
    let (dir, isolated) = data_dir(&app)?;
    blocking(move || RecentsFile {
        json: read_json(&dir),
        isolated,
    })
    .await
}

#[tauri::command]
pub async fn recents_save(app: tauri::AppHandle, json: String) -> Result<(), String> {
    let (dir, _) = data_dir(&app)?;
    blocking(move || write_json(&dir, &json)).await?.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_verbatim_prefixes() {
        assert_eq!(strip_verbatim(r"\\?\C:\Docs\a.pdf"), r"C:\Docs\a.pdf");
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share\a.pdf"), r"\\srv\share\a.pdf");
        assert_eq!(strip_verbatim(r"C:\Docs\a.pdf"), r"C:\Docs\a.pdf");
    }

    #[test]
    fn stat_reports_missing_and_present() {
        let dir = std::env::temp_dir().join("inkwell-tests").join("recents");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Case.pdf");
        std::fs::write(&f, b"%PDF-1.4").unwrap();

        let s = stat_one(&f.to_string_lossy());
        assert!(s.exists && s.size == 8 && s.modified > 0);
        assert!(!s.path.starts_with(r"\\?\"));

        // a lower-cased spelling resolves to the on-disk casing: one dedupe key per file
        let lower = stat_one(&f.to_string_lossy().to_lowercase());
        assert_eq!(lower.path, s.path);

        let gone = stat_one(&dir.join("nope.pdf").to_string_lossy());
        assert!(!gone.exists);
    }

    #[test]
    fn json_roundtrip_is_atomic() {
        let dir = std::env::temp_dir().join("inkwell-tests").join("recents-store");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_json(&dir), None);
        write_json(&dir, "[1]").unwrap();
        write_json(&dir, "[1,2]").unwrap();
        assert_eq!(read_json(&dir).as_deref(), Some("[1,2]"));
        assert!(!dir.join("recents.json.tmp").exists());
    }
}
