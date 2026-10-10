//! The only way a file path gets from the user to a command. The webview never hands Rust a path:
//! it gets an opaque token for a file the user chose (dialog, drag-drop, command line, a recent
//! file) and passes the token back. See docs/engine-rules.md, "File paths".
//!
//! - Open tokens name a file to read. They work for the window they were issued to, as often as needed.
//! - Save tokens name a file to write. They come only from a save dialog, work once, for that window.
//! - Known paths: every file the user chose, plus the stored recent list. `known_file` turns one of
//!   those back into an open token, and the recent list only ever keeps known paths.

use crate::engine::EngineError;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A file the UI may name: the token for commands, the path for display only.
#[derive(Serialize, Clone, Debug)]
pub struct FileRef {
    pub token: String,
    pub path: String,
}

struct Entry {
    path: String,
    window: String,
    save: bool,
}

#[derive(Default)]
pub struct Paths {
    tokens: Mutex<HashMap<String, Entry>>,
    known: Mutex<HashSet<String>>,
}

/// Identity of a path on Windows, without touching the disk (same rules as the UI's `keyOf`).
fn key(path: &str) -> String {
    crate::recents::strip_verbatim(&path.replace('/', "\\")).trim_end_matches('\\').to_lowercase()
}

fn bad() -> EngineError {
    EngineError::Invalid("unknown or used file token".into())
}

impl Paths {
    fn issue(&self, window: &str, path: String, save: bool) -> FileRef {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).expect("system random source");
        let token: String = b.iter().map(|x| format!("{x:02x}")).collect();
        self.learn(&path);
        let e = Entry { path: path.clone(), window: window.to_string(), save };
        self.tokens.lock().unwrap().insert(token.clone(), e);
        FileRef { token, path }
    }

    /// A file the user chose to read, for `window`.
    pub fn issue_open(&self, window: &str, path: String) -> FileRef {
        self.issue(window, path, false)
    }

    /// A file the user chose to write in a save dialog, for `window`, good for one write.
    pub fn issue_save(&self, window: &str, path: String) -> FileRef {
        self.issue(window, path, true)
    }

    /// Path of an open token issued to `window`.
    pub fn open(&self, window: &str, token: &str) -> Result<String, EngineError> {
        match self.tokens.lock().unwrap().get(token) {
            Some(e) if !e.save && e.window == window => Ok(e.path.clone()),
            _ => Err(bad()),
        }
    }

    /// Path of a save token issued to `window`; the token is spent.
    pub fn take_save(&self, window: &str, token: &str) -> Result<String, EngineError> {
        let mut t = self.tokens.lock().unwrap();
        match t.get(token) {
            Some(e) if e.save && e.window == window => Ok(t.remove(token).unwrap().path),
            _ => Err(bad()),
        }
    }

    /// Adds a path the user chose (or that the stored recent list has) to the known ones.
    pub fn learn(&self, path: &str) {
        self.known.lock().unwrap().insert(key(path));
    }

    pub fn known(&self, path: &str) -> bool {
        self.known.lock().unwrap().contains(&key(path))
    }

    /// An open token for a known path that is a file today (recent list, Locate, Home, Library).
    pub fn known_file(&self, window: &str, path: String) -> Result<FileRef, EngineError> {
        if !self.known(&path) {
            return Err(EngineError::Invalid("not a file Inkwell was given".into()));
        }
        if !std::path::Path::new(&path).is_file() {
            return Err(EngineError::Io(format!("{path} is not there any more")));
        }
        Ok(self.issue_open(window, path))
    }

    /// The stored recent list's paths are known: they were the user's files when they were listed.
    pub fn learn_recents(&self, json: &str) {
        for p in recent_paths(json) {
            self.learn(&p);
        }
    }

    /// The recent list as the UI wants to store it, without entries for paths nobody chose.
    pub fn filter_recents(&self, json: &str) -> Result<String, String> {
        let mut items: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        items.retain(|v| v["path"].as_str().is_some_and(|p| self.known(p)));
        serde_json::to_string(&items).map_err(|e| e.to_string())
    }
}

fn recent_paths(json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v["path"].as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::edit::EditOp;
    use crate::engine::Engine;

    const W: &str = "main";

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join("inkwell-tests").join("paths").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(d: &std::path::Path, name: &str, pages: usize) -> String {
        let p = d.join(name);
        let f = crate::engine::fixture::Fixture { pages, ..Default::default() };
        std::fs::write(&p, crate::engine::fixture::build(&f)).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn raw_paths_unknown_and_spent_tokens_are_refused() {
        let d = dir("refuse");
        let f = sample(&d, "a.pdf", 1);
        let p = Paths::default();
        assert!(p.open(W, &f).is_err(), "a raw path is not a token");
        assert!(p.take_save(W, &f).is_err());
        assert!(p.open(W, "00112233445566778899aabbccddeeff").is_err());

        let o = p.issue_open(W, f.clone());
        assert_eq!(p.open(W, &o.token).unwrap(), f);
        assert_eq!(p.open(W, &o.token).unwrap(), f, "open tokens can be used again");
        assert!(p.open("other", &o.token).is_err(), "tokens belong to one window");
        assert!(p.take_save(W, &o.token).is_err(), "an open token never names a write");

        let s = p.issue_save(W, d.join("out.pdf").to_string_lossy().into_owned());
        assert!(p.open(W, &s.token).is_err(), "a save token is not for reading");
        assert!(p.take_save("other", &s.token).is_err());
        assert!(p.take_save(W, &s.token).is_ok());
        assert!(p.take_save(W, &s.token).is_err(), "save tokens work once");
    }

    #[test]
    fn known_paths_come_from_choices_and_the_stored_list() {
        let d = dir("known");
        let f = sample(&d, "Doc.pdf", 1);
        let other = sample(&d, "other.pdf", 1);
        let p = Paths::default();
        assert!(p.known_file(W, f.clone()).is_err());
        p.learn_recents(&format!(r#"[{{"path":{}}}]"#, serde_json::to_string(&f).unwrap()));
        let r = p.known_file(W, f.to_uppercase().replace('\\', "/")).unwrap();
        assert!(p.open(W, &r.token).is_ok(), "any spelling of a known path");

        let json = serde_json::json!([{ "path": f, "pinned": true }, { "path": other }]).to_string();
        let kept = p.filter_recents(&json).unwrap();
        assert!(kept.contains("pinned") && !kept.contains("other.pdf"), "{kept}");
        p.issue_open(W, other.clone());
        assert!(p.filter_recents(&json).unwrap().contains("other.pdf"));
    }

    /// Save, extract, merge and insert with tokens resolved the way the commands resolve them.
    #[test]
    fn edits_reach_the_files_through_tokens() {
        let d = dir("e2e");
        let a = sample(&d, "a.pdf", 3);
        let b = sample(&d, "b.pdf", 2);
        let p = Paths::default();
        let eng = Engine::new(1, 1 << 20);
        let ta = p.issue_open(W, a.clone()).token;
        let tb = p.issue_open(W, b.clone()).token;

        let doc = match eng.open_pending(&p.open(W, &ta).unwrap()) {
            (doc, Ok(_)) => doc,
            (_, Err(e)) => panic!("{e:?}"),
        };
        let mut op = EditOp::InsertFile { at: 0, path: tb.clone(), pages: None, password: None };
        crate::resolve_op(&p, W, &mut op).unwrap();
        assert_eq!(eng.apply(doc, op).unwrap().page_count, 5);

        let out = d.join("copy.pdf").to_string_lossy().into_owned();
        let s = p.issue_save(W, out.clone()).token;
        eng.save(doc, Some(&p.take_save(W, &s).unwrap())).unwrap();
        assert!(p.take_save(W, &s).is_err());

        let ex = d.join("ex.pdf").to_string_lossy().into_owned();
        let s = p.issue_save(W, ex.clone()).token;
        eng.extract(doc, &[0], &p.take_save(W, &s).unwrap(), false).unwrap();

        let m = d.join("m.pdf").to_string_lossy().into_owned();
        let s = p.issue_save(W, m.clone()).token;
        let srcs = [ta.clone(), tb.clone()].iter().map(|t| p.open(W, t)).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(eng.merge(&srcs, &p.take_save(W, &s).unwrap()).unwrap(), 5);
        for f in [&out, &ex, &m] {
            assert!(std::fs::metadata(f).unwrap().len() > 0, "{f}");
        }

        let mut raw = EditOp::InsertFile { at: 0, path: b.clone(), pages: None, password: None };
        assert!(crate::resolve_op(&p, W, &mut raw).is_err(), "insert refuses a raw path");
        eng.close(doc);
    }
}
