//! Files named on a command line: the first launch (`initial_files`) and a later launch that the
//! single-instance plugin forwards to the running app (its argv and working directory).

use std::path::{Path, PathBuf};

/// The existing PDF files in `args` (argv, program name first); relative paths are taken from `cwd`.
/// An argument is a file when it names one, so a flag never matches and a file called `-x.pdf` does.
pub fn pdfs_in(args: &[String], cwd: &Path) -> Vec<PathBuf> {
    args.iter()
        .skip(1)
        .map(|a| cwd.join(a)) // an absolute argument replaces `cwd`
        .filter(|p| p.is_file() && is_pdf(p))
        .collect()
}

pub fn is_pdf(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// A multi-select "Open with" of a whole folder is a few hundred files; more than this is not a launch.
const MAX_WAITING: usize = 1000;

/// Files forwarded by later launches. A multi-select "Open with" starts one process per file within a
/// few milliseconds, long before the first process's window has loaded: those files wait here until the
/// UI asks for its launch files, then go straight to the window.
#[derive(Default)]
pub struct Forwarded {
    state: std::sync::Mutex<(bool, Vec<PathBuf>)>, // (UI has asked for its launch files, files waiting)
}

impl Forwarded {
    /// Files from a later launch: `Some` to send to the window now, `None` when they were kept for `ui_ready`.
    pub fn forward(&self, files: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
        let mut s = self.state.lock().unwrap();
        if s.0 {
            return Some(files);
        }
        let room = MAX_WAITING.saturating_sub(s.1.len());
        s.1.extend(files.into_iter().take(room));
        None
    }

    /// The UI asked for its launch files: the ones forwarded so far, and later ones are no longer kept.
    pub fn ui_ready(&self) -> Vec<PathBuf> {
        let mut s = self.state.lock().unwrap();
        s.0 = true;
        std::mem::take(&mut s.1)
    }
}

/// Whether this process takes part in single-instance forwarding. Runs with a scratch data dir
/// (`INKWELL_DATA_DIR`: tests, benchmarks, screenshot scripts) and debug builds stay independent, so
/// several can run at once next to an installed Inkwell; `INKWELL_SINGLE_INSTANCE=1` turns forwarding
/// back on for them (the installer check does).
pub fn single_instance() -> bool {
    let set = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    takes_part(cfg!(debug_assertions), set("INKWELL_DATA_DIR"), std::env::var_os("INKWELL_SINGLE_INSTANCE").is_some_and(|v| v == "1"))
}

fn takes_part(debug_build: bool, scratch_data_dir: bool, forced: bool) -> bool {
    forced || !(debug_build || scratch_data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("inkwell-launch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn absolute_and_relative_files_are_found_and_the_rest_is_skipped() {
        let d = scratch("a");
        std::fs::write(d.join("one.pdf"), b"%PDF").unwrap();
        std::fs::write(d.join("two words.pdf"), b"%PDF").unwrap();
        std::fs::write(d.join("-dash.pdf"), b"%PDF").unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(d.join("folder.pdf")).unwrap();
        let got = pdfs_in(
            &args(&[
                "inkwell.exe",
                "--flag",
                d.join("one.pdf").to_str().unwrap(),
                "two words.pdf",
                "-dash.pdf",
                "notes.txt",
                "missing.pdf",
                "folder.pdf",
            ]),
            &d,
        );
        assert_eq!(got, vec![d.join("one.pdf"), d.join("two words.pdf"), d.join("-dash.pdf")]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn files_forwarded_before_the_ui_is_ready_wait_for_it() {
        let f = Forwarded::default();
        assert_eq!(f.forward(vec!["a.pdf".into()]), None);
        assert_eq!(f.forward(vec!["b.pdf".into(), "c.pdf".into()]), None);
        let got = f.ui_ready();
        assert_eq!(got, vec![PathBuf::from("a.pdf"), "b.pdf".into(), "c.pdf".into()]);
        assert_eq!(f.forward(vec!["d.pdf".into()]), Some(vec!["d.pdf".into()]));
        assert!(f.ui_ready().is_empty());
    }

    #[test]
    fn the_waiting_queue_is_capped() {
        let f = Forwarded::default();
        for i in 0..MAX_WAITING + 50 {
            assert_eq!(f.forward(vec![format!("{i}.pdf").into()]), None);
        }
        let got = f.ui_ready();
        assert_eq!(got.len(), MAX_WAITING);
        assert_eq!(got[0], PathBuf::from("0.pdf"));
    }

    #[test]
    fn only_pdf_extensions_count() {
        assert!(is_pdf(Path::new("C:\\x\\Report.PDF")));
        assert!(!is_pdf(Path::new("notes.txt")));
        assert!(!is_pdf(Path::new("pdf")));
    }

    #[test]
    fn the_program_name_is_never_a_file() {
        let d = scratch("b");
        std::fs::write(d.join("inkwell.exe"), b"MZ").unwrap();
        assert!(pdfs_in(&args(&["inkwell.exe"]), &d).is_empty());
        assert!(pdfs_in(&[], &d).is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn debug_builds_and_scratch_data_dirs_stay_independent_unless_forced() {
        assert!(takes_part(false, false, false)); // an installed release build
        assert!(!takes_part(true, false, false)); // a dev build next to the installed app
        assert!(!takes_part(false, true, false)); // a scratch data dir
        assert!(takes_part(true, true, true)); // INKWELL_SINGLE_INSTANCE=1
    }
}
