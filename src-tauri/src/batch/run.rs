//! The runner: one file at a time, every step on a private working copy, nothing visible until the
//! file is done.
//!
//! Per file: the source is only ever read. Steps write into a hidden staging folder in the folder the
//! result will land in (so the final move is a rename), the finished files are given names that do
//! not exist yet (a name is claimed with `create_new`, then the staged file is renamed over the empty
//! claim), and a cancel or a failure drops the staging folder with everything in it. A cancel between
//! files keeps the files already finished: each of those is whole.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;

use super::action::{Action, Issue, OnError, OutputRule, Step};
use super::steps::{Cx, Env, Outcome, Product, StepError};
use crate::convert::Staging;
use crate::engine::edit::open_source;
use crate::engine::EngineError;
use crate::ocr;
use crate::protect;

#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    /// The Action cannot run; the issues say why (`batch_validate` returns the same list).
    #[error("{}", .0.first().map_or("The action is not valid.", |i| i.message.as_str()))]
    Invalid(Vec<Issue>),
    #[error("{0}")]
    Input(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Every step ran and the result is in place.
    Ok,
    /// Left alone on purpose, with the reason: open in a tab, signed, needs a password, restricted,
    /// cancelled, or after a file that stopped the run.
    Skipped,
    /// A step failed. Nothing of this file was written.
    Error,
}

/// A file the run made. `token` is filled in by the command layer (an open token for the webview).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputFile {
    pub path: String,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReport {
    pub index: usize,
    /// The source, as the caller named it (display only).
    pub input: String,
    pub name: String,
    pub status: Status,
    pub reason: Option<String>,
    pub size_before: u64,
    /// The PDF the run made, or the exports when the Action makes no PDF.
    pub size_after: Option<u64>,
    pub outputs: Vec<OutputFile>,
    pub notes: Vec<String>,
    /// Things to look at in the result although the file was processed (pages a search could not read).
    pub warnings: Vec<String>,
    pub steps_done: usize,
    pub ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub action: String,
    pub files: Vec<FileReport>,
    pub ok: usize,
    pub skipped: usize,
    pub errors: usize,
    /// The person cancelled the run.
    pub cancelled: bool,
    /// A file failed and the Action says stop.
    pub stopped: bool,
}

/// Progress for the UI, one event per thing that happens.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    Started { files: usize },
    FileStarted { index: usize, total: usize, name: String },
    Step { index: usize, step: usize, steps: usize, label: String },
    FileDone { index: usize, report: FileReport },
    Finished { ok: usize, skipped: usize, errors: usize, cancelled: bool },
}

/// Runs `action` over `files` (paths the command resolved from open tokens). `folder` is the folder
/// of a [`OutputRule::Folder`] Action.
pub fn run(
    action: &Action,
    files: &[PathBuf],
    folder: Option<&Path>,
    env: &Env,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Event),
) -> Result<Report, BatchError> {
    let issues = action.validate(env);
    if !issues.is_empty() {
        return Err(BatchError::Invalid(issues));
    }
    if files.is_empty() {
        return Err(BatchError::Input("Add at least one file.".into()));
    }
    if action.output.needs_folder() {
        match folder {
            Some(f) if f.is_dir() => {}
            Some(f) => return Err(BatchError::Input(format!("{} is not a folder.", f.display()))),
            None => return Err(BatchError::Input("Choose the folder the results go in.".into())),
        }
    }

    let total = files.len();
    emit(Event::Started { files: total });
    let mut reports: Vec<FileReport> = Vec::with_capacity(total);
    let mut bates_next: HashMap<usize, u64> = HashMap::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut halt: Option<String> = None;
    let mut cancelled = false;

    for (index, src) in files.iter().enumerate() {
        let name = src.file_name().map_or_else(|| src.display().to_string(), |n| n.to_string_lossy().into_owned());
        emit(Event::FileStarted { index, total, name: name.clone() });
        let mut r = FileReport {
            index,
            input: src.to_string_lossy().into_owned(),
            name: name.clone(),
            status: Status::Skipped,
            reason: None,
            size_before: fs::metadata(src).map_or(0, |m| m.len()),
            size_after: None,
            outputs: Vec::new(),
            notes: Vec::new(),
            warnings: Vec::new(),
            steps_done: 0,
            ms: 0,
        };
        let canon = fs::canonicalize(src).unwrap_or_else(|_| src.clone());
        if let Some(why) = &halt {
            r.reason = Some(why.clone());
        } else if cancel.load(Ordering::Relaxed) {
            r.reason = Some("cancelled".into());
            cancelled = true;
        } else if seen.contains(&canon) {
            r.reason = Some("listed more than once".into());
        } else {
            seen.push(canon);
            let t0 = Instant::now();
            let job = Job { action, folder, env, cancel, bates_next: &bates_next, done: std::cell::Cell::new(0) };
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process(&job, index, src, emit)));
            let verdict = done.unwrap_or_else(|_| Verdict::Error("a step crashed".into()));
            r.ms = t0.elapsed().as_millis() as u64;
            // How far it got: for a file that stopped, the steps that had finished.
            r.steps_done = job.done.get();
            match verdict {
                Verdict::Done(d) => {
                    r.status = Status::Ok;
                    r.size_after = d.size_after;
                    r.outputs = d.outputs.into_iter().map(|p| OutputFile { path: p.to_string_lossy().into_owned(), token: None }).collect();
                    r.notes = d.notes;
                    r.warnings = d.warnings;
                    r.steps_done = action.steps.len();
                    for (step, next) in d.bates_used {
                        bates_next.insert(step, next);
                    }
                }
                Verdict::Skip(why) => r.reason = Some(why),
                Verdict::Cancelled => {
                    r.reason = Some("cancelled".into());
                    cancelled = true;
                }
                Verdict::Error(why) => {
                    r.status = Status::Error;
                    r.reason = Some(why);
                    if action.on_error == OnError::Stop {
                        halt = Some(format!("not started: the run stopped after an error in {name}"));
                    }
                }
            }
        }
        emit(Event::FileDone { index, report: r.clone() });
        reports.push(r);
    }

    let count = |s: Status| reports.iter().filter(|r| r.status == s).count();
    let (ok, skipped, errors) = (count(Status::Ok), count(Status::Skipped), count(Status::Error));
    emit(Event::Finished { ok, skipped, errors, cancelled });
    Ok(Report { action: action.name.clone(), files: reports, ok, skipped, errors, cancelled, stopped: halt.is_some() })
}

// ---------------------------------------------------------------- one file

struct Done {
    outputs: Vec<PathBuf>,
    size_after: Option<u64>,
    notes: Vec<String>,
    warnings: Vec<String>,
    bates_used: Vec<(usize, u64)>,
}

enum Verdict {
    Done(Done),
    Skip(String),
    Error(String),
    Cancelled,
}

/// What every file of a run shares.
struct Job<'a> {
    action: &'a Action,
    folder: Option<&'a Path>,
    env: &'a Env,
    cancel: &'a AtomicBool,
    bates_next: &'a HashMap<usize, u64>,
    /// Steps of the current file that have finished.
    done: std::cell::Cell<usize>,
}

fn process(job: &Job, index: usize, src: &Path, emit: &mut dyn FnMut(Event)) -> Verdict {
    let Job { action, folder, env, cancel, bates_next, .. } = *job;
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    // 1. Whether this file may be touched at all.
    if !src.is_file() {
        return Verdict::Error("the file is not there any more".into());
    }
    if (env.is_open)(src) {
        return Verdict::Skip("open in Inkwell: save and close it first, so the result has its changes".into());
    }
    let (signed, sec) = match open_source(&src.to_string_lossy(), None) {
        Ok(doc) => match protect::read_security(&doc, None) {
            Ok(sec) => (is_signed(&doc), sec),
            Err(e) => return Verdict::Error(e.to_string()),
        },
        Err(EngineError::NeedsPassword | EngineError::BadPassword) => return Verdict::Skip("needs a password".into()),
        Err(e) => return Verdict::Error(e.to_string()),
    };
    if !sec.rights.may_extract() {
        return Verdict::Skip("its owner restricted copying or page changes, and a batch cannot lift that".into());
    }
    // The owner's password is not something a batch has. Without it the file's restrictions are the
    // owner's to change: a result keeps them, and a step that would replace them is refused.
    if sec.encrypted && !sec.unlocked {
        if action.steps.iter().any(|s| matches!(s, Step::Protect(_))) {
            return Verdict::Skip("its owner password guards its restrictions, and Protect would replace them".into());
        }
        if !sec.rights.modify && action.first_signature_breaker().is_some() {
            return Verdict::Skip("its owner does not allow changes to the content".into());
        }
    }
    if let Some(step) = action.first_signature_breaker() {
        match signed {
            Some(false) => {}
            Some(true) => return Verdict::Skip(format!("signed: \"{}\" would invalidate the signature", step.label())),
            None => return Verdict::Skip(format!("could not check whether the file is signed: \"{}\" might invalidate a signature", step.label())),
        }
    }

    // 2. A private working folder where the result will land.
    let out_dir = match (&action.output, folder) {
        (OutputRule::Folder { .. }, Some(f)) => f.to_path_buf(),
        _ => src.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf),
    };
    if let Some(j) = &env.journal {
        remember_dir(j, &out_dir, action.output.suffix());
    }
    let stage = match Staging::new(&out_dir) {
        Ok(s) => s,
        Err(e) => return Verdict::Error(format!("cannot write in {}: {e}", out_dir.display())),
    };
    let stem = src.file_stem().map_or_else(|| "document".into(), |s| s.to_string_lossy().into_owned());

    // 3. The steps, each on the file the one before wrote.
    let mut cx = Cx {
        env,
        cancel,
        stage: &stage,
        stem: &stem,
        index: 0,
        bates_next,
        bates_used: Vec::new(),
        products: Vec::new(),
        notes: Vec::new(),
        warnings: Vec::new(),
    };
    let mut current = src.to_path_buf();
    let mut wrote = false;
    for (i, step) in action.steps.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Verdict::Cancelled;
        }
        emit(Event::Step { index, step: i, steps: action.steps.len(), label: step.label() });
        cx.index = i;
        let output = stage.path(&format!("step-{i}.pdf"));
        let note_from = cx.notes.len();
        match step.imp().run(&mut cx, &current, &output) {
            Ok(Outcome::Wrote) => {
                job.done.set(i + 1);
                if wrote {
                    let _ = fs::remove_file(&current);
                }
                current = output;
                wrote = true;
            }
            Ok(Outcome::Unchanged) => {
                job.done.set(i + 1);
                let _ = fs::remove_file(&output);
            }
            Err(StepError::Cancelled) => return Verdict::Cancelled,
            Err(StepError::Refused(why)) => return Verdict::Skip(why),
            Err(StepError::Failed(why)) => return Verdict::Error(format!("{}: {why}", step.label())),
        }
        for n in &mut cx.notes[note_from..] {
            *n = format!("{}: {n}", step.label());
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Verdict::Cancelled;
    }

    // 4. The PDF the Action makes (a copy of the source when no step changed it).
    let pdf = if action.writes_pdf() {
        if !wrote {
            let copy = stage.path("copy.pdf");
            if let Err(e) = fs::copy(src, &copy) {
                return Verdict::Error(format!("cannot copy {name}: {e}"));
            }
            current = copy;
        }
        Some(current)
    } else {
        None
    };

    // 5. Names that are free, then the move into place. Not cancellable from here: it is quick, and
    // a half-moved result is the one thing a cancel must not leave.
    let suffix = action.output.suffix();
    let base = format!("{stem}{suffix}");
    let mut claimed: Vec<Claim> = Vec::new();
    // Every claim is written down inside the staging folder first: after a crash the sweep finds the
    // empty claims there and removes them.
    let log = |mark: char, p: &Path| {
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(stage.path(CLAIMS)) {
            let _ = writeln!(f, "{mark}{}", p.display());
        }
    };
    let published = (|| -> std::io::Result<()> {
        if let Some(p) = &pdf {
            let to = claim_file(&out_dir, &base, "pdf")?;
            log('+', &to);
            claimed.push(Claim { to, from: p.clone(), is_dir: false });
        }
        for product in &cx.products {
            match product {
                Product::File { staged, ext } => {
                    let to = claim_file(&out_dir, &base, ext)?;
                    log('+', &to);
                    claimed.push(Claim { to, from: staged.clone(), is_dir: false });
                }
                Product::Folder { staged, label } => {
                    // The folder appears whole or not at all: the staged folder itself is renamed into place.
                    let to = publish_dir(&out_dir, &format!("{base} ({label})"), staged)?;
                    log('=', &to);
                    claimed.push(Claim { to, from: staged.clone(), is_dir: true });
                }
            }
        }
        for c in &claimed {
            if !c.is_dir {
                fs::rename(&c.from, &c.to)?;
                log('=', &c.to);
            }
        }
        Ok(())
    })();
    if let Err(e) = published {
        for c in &claimed {
            if c.is_dir {
                let _ = fs::remove_dir_all(&c.to);
            } else {
                let _ = fs::remove_file(&c.to);
            }
        }
        return Verdict::Error(format!("cannot write the result: {e}"));
    }

    let outputs: Vec<PathBuf> = claimed.iter().map(|c| c.to.clone()).collect();
    let size_after = match claimed.iter().find(|c| !c.is_dir && c.to.extension().is_some_and(|e| e == "pdf")) {
        Some(c) => fs::metadata(&c.to).ok().map(|m| m.len()),
        None => Some(claimed.iter().map(|c| tree_size(&c.to)).sum()),
    };
    Verdict::Done(Done { outputs, size_after, notes: cx.notes, warnings: cx.warnings, bates_used: cx.bates_used })
}

struct Claim {
    to: PathBuf,
    from: PathBuf,
    is_dir: bool,
}

/// `<base>.<ext>`, then `<base> (2).<ext>`...: the first name nobody has, made as an empty file so no
/// one else can take it before the staged file is renamed over it.
fn claim_file(dir: &Path, base: &str, ext: &str) -> std::io::Result<PathBuf> {
    use std::io::ErrorKind::AlreadyExists;
    for n in 1..10_000u32 {
        let name = if n == 1 { format!("{base}.{ext}") } else { format!("{base} ({n}).{ext}") };
        let path = dir.join(name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free file name"))
}

/// Renames `staged` to `<base>`, then `<base> (2)`...: the first name that is free. A folder that
/// exists is never entered or replaced (a rename onto a non-empty folder fails; one onto an empty folder
/// is avoided by looking first).
fn publish_dir(dir: &Path, base: &str, staged: &Path) -> std::io::Result<PathBuf> {
    for n in 1..10_000u32 {
        let path = dir.join(if n == 1 { base.to_string() } else { format!("{base} ({n})") });
        if fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        match fs::rename(staged, &path) {
            Ok(()) => return Ok(path),
            // Taken since we looked.
            Err(_) if fs::symlink_metadata(&path).is_ok() => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free folder name"))
}

fn tree_size(p: &Path) -> u64 {
    match fs::metadata(p) {
        Ok(m) if m.is_dir() => fs::read_dir(p).into_iter().flatten().flatten().map(|e| tree_size(&e.path())).sum(),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

// ---------------------------------------------------------------- signatures

/// A signature the file carries: the "signatures exist" flag, a certification (DocMDP), or a signature
/// field that has a value. `None` when the form cannot be listed (too large to scan): the caller
/// treats that as signed.
fn is_signed(doc: &mupdf::pdf::PdfDocument) -> Option<bool> {
    if ocr::is_signed(doc) {
        return Some(true);
    }
    if crate::outline::certified(doc).map_or(true, |c| c.is_some()) {
        return Some(true);
    }
    let form = crate::forms::list(doc).ok()?;
    Some(form.fields.iter().any(|f| {
        f.kind == crate::forms::FieldKind::Signature
            && doc
                .xref_object(f.xref)
                .ok()
                .flatten()
                .and_then(|w| w.get_dict_inheritable("V").ok().flatten())
                .is_some_and(|v| !v.is_null().unwrap_or(true))
    }))
}

// ---------------------------------------------------------------- what a crash leaves behind

/// The file inside a staging folder that lists the names claimed for the result.
const CLAIMS: &str = "claims.txt";
const MAX_DIRS: usize = 200;
/// A staging folder this old belongs to a run that is gone.
const STALE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Runs `f` holding the journal's lock file (`create_new`; one that is a minute old is a dead holder's).
/// Without the lock after a couple of seconds `f` is skipped: the journal only helps the sweep.
fn with_journal_lock(journal: &Path, f: impl FnOnce()) {
    let lock = journal.with_extension("lock");
    if let Some(parent) = journal.parent() {
        let _ = fs::create_dir_all(parent);
    }
    for _ in 0..200 {
        match fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(_) => {
                f();
                let _ = fs::remove_file(&lock);
                return;
            }
            Err(_) => {
                let dead = fs::metadata(&lock).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e.as_secs() > 60);
                if dead {
                    let _ = fs::remove_file(&lock);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }
}

fn journal_lines(journal: &Path) -> Vec<String> {
    fs::read_to_string(journal).map(|t| t.lines().filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default()
}

/// Writes the folder a run works in, and the ending its results carry, into the journal
/// (`<folder>\t<ending>` per line, the newest last, at most [`MAX_DIRS`]).
fn remember_dir(journal: &Path, dir: &Path, suffix: &str) {
    let line = format!("{}\t{suffix}", dir.to_string_lossy());
    if suffix.contains(['\t', '\n', '\r']) {
        return;
    }
    with_journal_lock(journal, || {
        let mut lines = journal_lines(journal);
        if lines.last() == Some(&line) {
            return;
        }
        lines.retain(|l| *l != line);
        lines.push(line);
        let skip = lines.len().saturating_sub(MAX_DIRS);
        let _ = fs::write(journal, lines[skip..].join("\n") + "\n");
    });
}

/// Removes what runs that died left in the folders they worked in: staging folders older than a day and
/// the empty files and folders they had claimed for a result. Folders that are gone drop out of the
/// journal. Returns how many staging folders went.
pub fn sweep(journal: &Path) -> usize {
    sweep_older_than(journal, STALE)
}

/// `.inkwell-convert-<pid>-<n>-<hex>`: the name `Staging` gives its folders.
fn staging_name(name: &str) -> bool {
    name.strip_prefix(".inkwell-convert-").is_some_and(|r| {
        let parts: Vec<&str> = r.split('-').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric()))
    })
}

/// Whether `rest` (a claimed name after the source's name and the ending) is one of the shapes a result
/// has: `.pdf`, `.txt`, ` (images)`, each with an optional ` (n)` before the extension or at the end.
fn result_shape(rest: &str) -> bool {
    let rest = rest.strip_suffix(".pdf").or_else(|| rest.strip_suffix(".txt")).unwrap_or(rest);
    let rest = rest.strip_suffix(')').and_then(|r| r.rsplit_once(" (")).map_or(rest, |(head, n)| {
        if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) { head } else { rest }
    });
    let rest = rest.strip_suffix(" (images)").unwrap_or(rest);
    rest.is_empty()
}

/// A claim the sweep may remove: a path directly inside the folder the journal named, called after a PDF
/// that is in that folder plus an ending the journal recorded for it. Anything else in a `claims.txt`
/// (a list anyone with write access to the folder could have made) is ignored.
fn claim_ok(dir: &Path, endings: &[&str], stems: &std::collections::HashSet<String>, claim: &Path) -> bool {
    if !claim.is_absolute() || claim.parent() != Some(dir) {
        return false;
    }
    let Some(name) = claim.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    stems.iter().any(|stem| {
        endings.iter().any(|e| name.strip_prefix(&format!("{stem}{e}")).is_some_and(result_shape))
    })
}

fn sweep_older_than(journal: &Path, age: std::time::Duration) -> usize {
    let old = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e >= age);
    // The folders with the endings recorded for each.
    let mut folders: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for line in journal_lines(journal) {
        let (dir, ending) = line.split_once('\t').unwrap_or((&line, ""));
        let dir = PathBuf::from(dir);
        match folders.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, e)) => e.push(ending.to_string()),
            None => folders.push((dir, vec![ending.to_string()])),
        }
    }
    let mut swept = 0;
    for (dir, endings) in &folders {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        let entries: Vec<_> = entries.flatten().collect();
        let stems: std::collections::HashSet<String> = entries
            .iter()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) && p.is_file()).then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))?
            })
            .collect();
        let endings: Vec<&str> = endings.iter().map(String::as_str).collect();
        for e in entries {
            let (path, name) = (e.path(), e.file_name().to_string_lossy().into_owned());
            if !staging_name(&name) || !path.is_dir() || !old(&path) {
                continue;
            }
            let text = fs::read_to_string(path.join(CLAIMS)).unwrap_or_default();
            let published: std::collections::HashSet<&str> = text.lines().filter_map(|l| l.strip_prefix('=')).collect();
            for claim in text.lines().filter_map(|l| l.strip_prefix('+')).filter(|c| !published.contains(c)) {
                let claim = Path::new(claim);
                if !claim_ok(dir, &endings, &stems, claim) {
                    continue;
                }
                // Only what is still empty: a result that was moved into place is not touched.
                match fs::symlink_metadata(claim) {
                    Ok(m) if m.is_file() && m.len() == 0 => drop(fs::remove_file(claim)),
                    Ok(m) if m.is_dir() => drop(fs::remove_dir(claim)),
                    _ => {}
                }
            }
            if fs::remove_dir_all(&path).is_ok() {
                swept += 1;
            }
        }
    }
    // Forget folders that are gone, under the lock and from the journal as it is now: a run that started
    // meanwhile has added its own line.
    with_journal_lock(journal, || {
        let keep: Vec<String> = journal_lines(journal)
            .into_iter()
            .filter(|l| Path::new(l.split_once('\t').map_or(l.as_str(), |x| x.0)).is_dir())
            .collect();
        if journal.exists() {
            let _ = fs::write(journal, keep.join("\n") + if keep.is_empty() { "" } else { "\n" });
        }
    });
    swept
}

#[cfg(test)]
pub(super) fn sweep_for_test(journal: &Path, age: std::time::Duration) -> usize {
    sweep_older_than(journal, age)
}
