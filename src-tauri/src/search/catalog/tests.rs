//! Catalogs on generated PDFs in temp folders: the same hits as a folder search under every option set,
//! refresh that reads only what changed, skips, cancel, a corrupted index, and the embedded index.

use super::*;
use crate::engine::edit::EditSource;
use crate::engine::{Engine, EngineError};
use crate::search::files::FolderSearches;
use crate::search::tests::{dir, doc_with, plain, write, Extra};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Duration;

const WIN: &str = "main";

/// A worker that is handed this file dies (a panic outside the guarded read), as a failed spawn or an
/// allocation failure would: the build must count the file, not lose it.
pub(super) fn worker_dies_on(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "worker-dies.pdf")
}

thread_local! {
    /// Runs once, while `forget_deferred` holds a catalog (after it read the list, before it lets go).
    static WHILE_HELD: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn while_held(_dir: &Path) {
    if let Some(f) = WHILE_HELD.with(|h| h.borrow_mut().take()) {
        f();
    }
}

/// Replaces the file with other bytes just before the build opens it (a file that grew after it was listed).
static BEFORE_OPEN: Mutex<Option<(PathBuf, Vec<u8>)>> = Mutex::new(None);

pub(super) fn before_open(path: &Path) {
    let mut g = BEFORE_OPEN.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|(p, _)| p == path) {
        let (p, bytes) = g.take().unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

/// Like `BEFORE_OPEN`, but just after the open handle's size was taken (a file that grew while it was being read).
static AFTER_LEN: Mutex<Option<(PathBuf, Vec<u8>)>> = Mutex::new(None);

pub(super) fn after_len(path: &Path) {
    let mut g = AFTER_LEN.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|(p, _)| p == path) {
        let (p, bytes) = g.take().unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

/// The files whose bytes were read while holding the big-file lock.
static READ_ALONE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub(super) fn read_alone(path: &Path) {
    READ_ALONE.lock().unwrap_or_else(|e| e.into_inner()).push(path.to_path_buf());
}

fn no_progress() -> impl Fn(u32, u32) + Sync {
    |_, _| {}
}

/// A catalog root and a folder of about thirty PDFs that exercise the matcher.
fn corpus(test: &str) -> (PathBuf, PathBuf) {
    let base = dir(&format!("catalog-{test}"));
    let folder = base.join("docs");
    let words = ["notice", "Notice", "NOTICE", "termination", "café", "Cafe", "naïve", "Straße", "strasse", "thirty days"];
    for i in 0..24 {
        let a = words[i % words.len()];
        let b = words[(i * 3 + 1) % words.len()];
        let lines = vec![format!("File {i} says {a} here and {b} there."), format!("Another line about {b} number {i}.")];
        let second = vec![format!("Second page {i}: nothing but {a}.")];
        let (l, s) = (lines.iter().map(String::as_str).collect::<Vec<_>>(), second.iter().map(String::as_str).collect::<Vec<_>>());
        write(&folder, &format!("f{i:02}.pdf"), plain(&[l, s]));
    }
    write(&folder, "hyphen.pdf", plain(&[vec!["we will co-", "operate fully with the well-", "known firm"]]));
    write(&folder, "phrase.pdf", plain(&[vec!["written notice of thirty", "days is required"], vec!["a longer paragraph with no match"]]));
    write(&folder, "sub/deep.pdf", plain(&[vec!["Notice given in a sub-folder", "cafe\u{e9} and CAFÉ"]]));
    let extra = Extra {
        catalog: "/Outlines 8 0 R /AcroForm << /Fields [7 0 R] /DA (/Helv 0 Tf 0 g) >>".into(),
        annots: "6 0 R 7 0 R".into(),
        objs: vec![
            "<< /Type /Annot /Subtype /Text /Rect [300 600 320 620] /Contents (Call about the NOTICE period) /T (Reviewer) /F 4 >>".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Clause) /V (notice given) /Rect [72 400 300 420] /F 4 /DA (/Helv 12 Tf 0 g) >>".into(),
            "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>".into(),
            "<< /Title (Termination and notice) /Parent 8 0 R /Dest [4 0 R /XYZ 0 792 0] >>".into(),
        ],
    };
    write(&folder, "extras.pdf", doc_with(&[vec!["Nothing relevant on the page."]], extra));
    write(&folder, "broken.pdf", b"this is not a pdf".to_vec());
    (base.join("catalogs"), folder)
}

fn make(root: &Path, folder: &Path) -> (Meta, PathBuf) {
    let m = create(root, "Test", &folder.to_string_lossy(), true).unwrap();
    let d = root.join(&m.id);
    (m, d)
}

fn built(test: &str) -> (PathBuf, PathBuf, PathBuf) {
    let (root, folder) = corpus(test);
    let (_, d) = make(&root, &folder);
    let s = build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(!s.cancelled);
    (root, folder, d)
}

/// What a result list comes to: per file index, its numbers and every item with the text around it.
type Flat = std::collections::BTreeMap<u32, (String, u32, u32, bool, Vec<(u32, String, String, String, String, usize)>)>;

fn flat(events: &[FilesEvent]) -> (Flat, (u32, u32, u32, u32)) {
    let mut out = Flat::new();
    let mut done = (0, 0, 0, 0);
    for e in events {
        match e {
            FilesEvent::File { index, name, hits, pages, truncated, items, .. } => {
                let items = items
                    .iter()
                    .map(|i| (i.page, format!("{:?}", i.hit.source), i.hit.before.clone(), i.hit.text.clone(), i.hit.after.clone(), i.hit.rects.len()))
                    .collect();
                out.insert(*index, (name.clone(), *hits, *pages, *truncated, items));
            }
            FilesEvent::Done { files, matched, hits, skipped, .. } => done = (*files, *matched, *hits, *skipped),
            _ => {}
        }
    }
    (out, done)
}

fn folder_search(folder: &Path, query: &str, o: SearchOpts) -> Vec<FilesEvent> {
    let files = list_pdfs(folder, true, &AtomicBool::new(false)).files;
    let searches = Arc::new(FolderSearches::default());
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    searches.start(WIN, files, false, query, o, Arc::new(move |e| {
        let _ = tx.lock().unwrap().send(e);
    }));
    let mut out = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(60)).expect("a Done event");
        let done = matches!(e, FilesEvent::Done { .. });
        out.push(e);
        if done {
            return out;
        }
    }
}

fn catalog_search(d: &Path, query: &str, o: SearchOpts) -> Vec<FilesEvent> {
    let searches = FolderSearches::default();
    let out = Mutex::new(Vec::new());
    search(d, query, o, &searches, WIN, &|e| out.lock().unwrap().push(e)).unwrap();
    out.into_inner().unwrap()
}

#[test]
fn a_query_gives_the_same_hits_per_page_as_a_folder_search_for_every_option_set() {
    let (_, folder, d) = built("parity");
    let queries = [
        "notice", "NOTICE", "written notice", "cooperate", "co-operate", "well-known", "wellknown", "cafe", "café", "thirty days", "days is", "tice",
        "otic", "strasse", "STRASSE", "straße", "a", "File 1", "nothing but", "and Cafe", "  notice  ", "-", "\u{301}",
    ];
    let mut compared = 0;
    for bits in 0..16u32 {
        let o = SearchOpts {
            case_sensitive: bits & 1 != 0,
            whole_words: bits & 2 != 0,
            ignore_accents: bits & 4 != 0,
            comments: bits & 8 != 0,
            fields: bits & 8 != 0,
            bookmarks: bits & 8 != 0,
        };
        for q in queries {
            let (want, want_done) = flat(&folder_search(&folder, q, o));
            let (got, got_done) = flat(&catalog_search(&d, q, o));
            assert_eq!(got, want, "query {q:?} with {o:?}");
            assert_eq!(got_done, want_done, "totals of {q:?} with {o:?}");
            compared += want.len();
        }
    }
    assert!(compared > 500, "the corpus should have produced real hits ({compared})");
}

#[test]
fn a_build_reports_what_it_found_and_skips_what_it_cannot_read() {
    let (_, folder, d) = built("skips");
    let m = load_meta(&d).unwrap();
    assert_eq!((m.generation, m.damaged, m.password), (1, 1, 0));
    assert_eq!(m.files, 24 + 3 + 1 + 1);
    assert_eq!(m.indexed, m.files - 1);
    assert_eq!(m.pages, 24 * 2 + 1 + 2 + 1 + 1);
    // An encrypted file is counted apart.
    let secret = folder.join("secret.pdf");
    {
        use mupdf::pdf::{Encryption, PdfDocument, PdfWriteOptions};
        let src = PdfDocument::from_bytes(&plain(&[vec!["hidden notice"]])).unwrap();
        let mut o = PdfWriteOptions::default();
        o.set_encryption(Encryption::Aes256).set_user_password("secret").set_owner_password("owner");
        src.save_with_options(&secret.to_string_lossy(), o).unwrap();
    }
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.damaged, s.password, s.added, s.extracted), (1, 1, 1, 0));
    // Neither is a hit, both are counted as skipped by the search.
    let (_, done) = flat(&catalog_search(&d, "notice", SearchOpts::default()));
    assert_eq!(done.3, 2);
}

#[test]
fn a_refresh_reads_only_the_files_that_changed() {
    let (root, folder, d) = built("refresh");
    let _ = root;
    // The damaged file is not kept: it is tried again at every refresh (and counted as neither new nor changed).
    let n = load_meta(&d).unwrap().indexed;
    assert_eq!(load_meta(&d).unwrap().damaged, 1);

    // Nothing changed: no indexed file is opened.
    assert_eq!(check(&d, &AtomicBool::new(false)).unwrap(), Stale::default());
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.extracted, s.kept, s.added, s.changed, s.removed), (0, n, 0, 0, 0));

    // One edited (longer text, so its size differs), one deleted, one new.
    write(&folder, "f03.pdf", plain(&[vec!["This file now says ZEBRA twice: zebra."]]));
    std::fs::remove_file(folder.join("f04.pdf")).unwrap();
    write(&folder, "new.pdf", plain(&[vec!["A brand new zebra document"]]));
    let stale = check(&d, &AtomicBool::new(false)).unwrap();
    assert_eq!((stale.added, stale.changed, stale.removed, stale.unusable), (1, 1, 1, false));
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.extracted, s.added, s.changed, s.removed), (2, 1, 1, 1));
    assert_eq!(s.kept, n - 2);
    assert_eq!(check(&d, &AtomicBool::new(false)).unwrap(), Stale::default());

    // The results are those of a search of the folder as it is now.
    for q in ["zebra", "notice", "File 4 says", "File 3 says"] {
        let (want, wd) = flat(&folder_search(&folder, q, SearchOpts::default()));
        let (got, gd) = flat(&catalog_search(&d, q, SearchOpts::default()));
        assert_eq!((got, gd), (want, wd), "{q}");
    }

    // A moved file is not read again.
    std::fs::create_dir_all(folder.join("moved")).unwrap();
    std::fs::rename(folder.join("f05.pdf"), folder.join("moved/f05.pdf")).unwrap();
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.extracted, s.added, s.removed, s.kept), (0, 0, 0, n));
}

#[test]
fn a_cancelled_build_leaves_the_old_index_usable() {
    let (_, folder, d) = built("cancel");
    let before = flat(&catalog_search(&d, "notice", SearchOpts::default()));
    // Change several files so the refresh has work, then cancel it after its first file.
    for i in 0..6 {
        write(&folder, &format!("f{i:02}.pdf"), plain(&[vec![&format!("changed {i} with a lot more text here to differ in size notice notice")]]));
    }
    let cancel = AtomicBool::new(false);
    let s = build(&d, false, &cancel, &|done, total| {
        if done + 5 >= total {
            cancel.store(true, Ordering::SeqCst);
        }
    })
    .unwrap();
    assert!(s.cancelled);
    assert_eq!(load_meta(&d).unwrap().generation, 1, "the live generation did not move");
    // The old index is what answers, for every file that is still the one it read; the rewritten ones (on disk
    // already) are left out and counted, not answered from old text.
    let ev = catalog_search(&d, "notice", SearchOpts::default());
    let (now, _) = flat(&ev);
    assert!(done_of(&ev).2 > 0 && now.len() < before.0.len());
    assert!(now.iter().all(|(k, v)| before.0.get(k) == Some(v)));
    // No blob of the cancelled run is left (every file id of the live generation is below its next_fid).
    let table = load_table(&d, 1).unwrap();
    let stray = std::fs::read_dir(text_dir(&d)).unwrap().flatten().filter(|e| {
        e.file_name().to_string_lossy().split('.').next().and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n >= table.next_fid)
    });
    assert_eq!(stray.count(), 0);
    // And the next refresh completes.
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(!s.cancelled && s.changed == 6);
}

#[test]
fn a_corrupted_index_is_never_trusted_and_is_rebuilt() {
    let (_, _, d) = built("corrupt");
    let want = flat(&catalog_search(&d, "notice", SearchOpts::default()));
    let p = index_path(&d, 1);
    let mut b = std::fs::read(&p).unwrap();
    let k = b.len() / 2;
    b[k] ^= 0xff;
    std::fs::write(&p, &b).unwrap();
    assert!(Index::load(&d, &load_meta(&d).unwrap()).is_none());
    let searches = FolderSearches::default();
    assert!(search(&d, "notice", SearchOpts::default(), &searches, WIN, &|_| {}).is_err());
    // A refresh reads everything again (it cannot keep what it cannot trust) and the catalog works.
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!(s.kept, 0);
    assert!(s.extracted > 20);
    assert_eq!(flat(&catalog_search(&d, "notice", SearchOpts::default())), want);
    // So is a table that is not JSON, and an index from another version.
    std::fs::write(table_path(&d, 2), b"{ nope").unwrap();
    assert!(Index::load(&d, &load_meta(&d).unwrap()).is_none());
}

#[test]
fn results_open_through_the_search_and_stay_under_the_folder() {
    let (_, folder, d) = built("tokens");
    let searches = FolderSearches::default();
    let id = search(&d, "zzzz-not-there", SearchOpts::default(), &searches, WIN, &|_| {}).unwrap();
    assert!(searches.path_of(WIN, id, 0).is_none());
    let id = search(&d, "thirty", SearchOpts::default(), &searches, WIN, &|_| {}).unwrap();
    let ev = catalog_search(&d, "thirty", SearchOpts::default());
    let (files, _) = flat(&ev);
    let first = *files.keys().next().expect("a hit");
    let p = searches.path_of(WIN, id, first).expect("recorded");
    assert!(p.starts_with(&folder) && p.is_file());
    // A path outside the folder (a tampered index) is neither listed nor recorded.
    let mut t = load_table(&d, 1).unwrap();
    let hit = t.files.iter().position(|f| f.path.ends_with("f00.pdf")).unwrap();
    assert!(flat(&catalog_search(&d, "notice", SearchOpts::default())).0.contains_key(&(hit as u32)));
    t.files[hit].path = "/etc/hosts".into();
    write_atomic(&table_path(&d, 1), &serde_json::to_vec(&t).unwrap()).unwrap();
    assert!(!flat(&catalog_search(&d, "notice", SearchOpts::default())).0.contains_key(&(hit as u32)));
    assert!(under(&folder, &folder.join("a.pdf")));
    assert!(!under(&folder, Path::new("/etc/hosts")));
    assert!(!under(&folder, &folder.join("..").join("x.pdf")));
}

#[test]
fn the_index_only_narrows_pages_and_a_query_of_marks_verifies_everything() {
    let (_, _, d) = built("narrow");
    let idx = Index::load(&d, &load_meta(&d).unwrap()).unwrap();
    assert!(idx.candidates(&query_words("\u{301}")).is_none());
    let c = idx.candidates(&query_words("termination")).unwrap();
    let all: usize = idx.table.files.iter().map(|f| f.pages as usize).sum();
    assert!(!c.is_empty() && c.values().map(Vec::len).sum::<usize>() < all);
    // Words are matched across a line break and in the widest fold.
    assert!(idx.candidates(&query_words("CAFÉ")).unwrap().len() >= 2);
    assert!(idx.candidates(&query_words("cooperate")).is_some_and(|c| !c.is_empty()));
}

#[test]
fn an_embedded_index_is_read_by_a_build_and_removing_it_is_one_undo_step() {
    let base = dir("catalog-embed");
    let folder = base.join("docs");
    let p = write(&folder, "a.pdf", plain(&[vec!["The embedded notice is on page one"], vec!["and a second page about cafe\u{e9}"]]));
    write(&folder, "b.pdf", plain(&[vec!["a plain notice"]]));
    let root = base.join("catalogs");
    let (_, d) = make(&root, &folder);

    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    assert!(!embed::embedded(&eng, id).unwrap().present);
    let st = embed::embed(&eng, id, EditSource::User).unwrap();
    assert_eq!(st.undo.as_deref(), Some("Embed search index"));
    let e = embed::embedded(&eng, id).unwrap();
    assert!(e.present && e.current && e.pages == 2, "{e:?}");
    // Nothing is written until Save.
    assert!(!std::fs::read(&p).unwrap().windows(9).any(|w| w == b"PieceInfo"));
    eng.save(id, None).unwrap();
    drop(eng);

    // Reopened, the document still has it; the catalog build takes it for this file and reads the other.
    let again = Engine::new(2, 64 << 20);
    let id = again.open(&p.to_string_lossy()).unwrap().doc;
    let e = embed::embedded(&again, id).unwrap();
    assert!(e.present && e.current, "the /ID must survive the save: {e:?}");
    let s = build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.from_embedded, s.extracted, s.files), (1, 1, 2));
    let (found, _) = flat(&catalog_search(&d, "notice", SearchOpts::default()));
    assert_eq!(found.len(), 2);
    let (found, _) = flat(&catalog_search(&d, "café", SearchOpts { ignore_accents: true, ..Default::default() }));
    assert_eq!(found.len(), 1);

    // Removing it is one step; undo brings it back; once saved the bytes are gone from the file.
    let st = embed::unembed(&again, id, EditSource::User).unwrap();
    assert_eq!(st.undo.as_deref(), Some("Remove embedded search index"));
    assert!(!embed::embedded(&again, id).unwrap().present);
    let st = again.undo(id).unwrap();
    assert_eq!(st.undo.as_deref(), None, "one undo step undoes the removal and nothing else (embedding was saved)");
    assert!(embed::embedded(&again, id).unwrap().present);
    embed::unembed(&again, id, EditSource::User).unwrap();
    again.save(id, None).unwrap();
    assert!(!embed::embedded(&again, id).unwrap().present);
    let bytes = std::fs::read(&p).unwrap();
    assert!(!bytes.windows(18).any(|w| w == b"InkwellSearchIndex"), "the removed index is not left in the file");
    let s = build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.from_embedded, s.extracted), (0, 2));
}

#[test]
fn a_stale_or_hostile_embedded_index_is_not_used() {
    // Wrong page count and wrong id: read() refuses both.
    let text = FileText { version: VERSION, id: "abcd".into(), pages: vec![StoredPage::default(); 2], bookmarks: vec![], cut: false, sig: String::new() };
    let bytes = text.encode().unwrap();
    assert_eq!(FileText::decode(&bytes).unwrap(), text);
    assert!(FileText::decode(&bytes[..bytes.len() / 2]).is_none());
    assert!(FileText::decode(b"not zstd").is_none());
    // Inflates past the bound: refused without being built in memory.
    let big = zstd::encode_all(vec![b' '; (MAX_BLOB + 1024) as usize].as_slice(), 1).unwrap();
    assert!(FileText::decode(&big).is_none());
    // A record that points outside itself does not panic.
    let st = StoredText { text: "ab".into(), synth: vec![9], skip: vec![7], lines: vec![(5, [0.0; 4]), (0, [1.0; 4])] };
    let _ = PageChars::from_stored(&st).terms(&mut Default::default());
}

#[test]
fn the_estimate_counts_files_and_bytes() {
    let (_, folder) = corpus("estimate");
    let e = estimate(&folder, true, &AtomicBool::new(false));
    assert_eq!(e.files, 24 + 3 + 1 + 1);
    assert!(e.bytes > 10_000 && e.secs >= 1 && e.index_bytes > 0 && !e.capped);
    let flat_only = estimate(&folder, false, &AtomicBool::new(false));
    assert_eq!(flat_only.files, e.files - 1);
}


#[test]
fn an_embedded_index_of_pages_that_changed_afterwards_is_not_used() {
    use mupdf::pdf::PdfDocument;
    let base = dir("catalog-stale-embed");
    let p = write(&base, "a.pdf", plain(&[vec!["the original notice"]]));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    embed::embed(&eng, id, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    drop(eng);
    let open = || PdfDocument::open(&*p.to_string_lossy()).unwrap();
    let before = open();
    let n = before.page_count().unwrap() as u32;
    let id_before = pdf_id(&before);
    assert!(embed::read(&before, n, &id_before).is_some());
    drop(before);
    // Something else rewrites the page's text and leaves /ID and the stream alone.
    let changed = open();
    let mut content = changed.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
    content.write_stream_string("BT /F1 12 Tf 72 700 Td (the ALTERED words) Tj ET").unwrap();
    let tmp = base.join("b.pdf");
    changed.save(&tmp.to_string_lossy()).unwrap();
    drop(changed);
    let after = PdfDocument::open(&*tmp.to_string_lossy()).unwrap();
    assert_eq!(pdf_id(&after), id_before);
    assert!(embed::status(&after).present);
    assert!(!embed::status(&after).current);
    assert!(embed::read(&after, n, &id_before).is_none());
}


// ---- review findings -------------------------------------------------------------------------------------

use mupdf::pdf::{PdfDocument, PdfObject};

fn stream_obj(dict: &str, body: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len())
}

/// A one-page file whose only text is drawn by a Form XObject (`/Fm0 Do`).
fn form_text_pdf(words: &str) -> Vec<u8> {
    crate::search::tests::pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /XObject << /Fm0 6 0 R >> >> >>".into(),
        stream_obj("", "/Fm0 Do"),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >>",
            &format!("BT /F1 12 Tf 72 700 Td ({words}) Tj ET"),
        ),
    ])
}

/// Embeds an index in the file at `p` through the engine and saves it.
fn embed_and_save(p: &Path) {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    embed::embed(&eng, id, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
}

fn current(p: &Path) -> bool {
    embed::status(&PdfDocument::open(&*p.to_string_lossy()).unwrap()).current
}

#[test]
fn text_only_in_a_form_xobject_is_part_of_the_fingerprint() {
    let base = dir("catalog-fp-form");
    let p = write(&base, "a.pdf", form_text_pdf("the form words"));
    embed_and_save(&p);
    assert!(current(&p));
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    let t = embed::read(&doc, 1, &pdf_id(&doc)).unwrap();
    assert!(t.pages[0].text.text.contains("the form words"), "the form's text is in the index");
    let mut fm = doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Fm0").unwrap().unwrap();
    fm.write_stream_string("BT /F1 12 Tf 72 700 Td (the CHANGED words) Tj ET").unwrap();
    let b = base.join("b.pdf");
    doc.save(&b.to_string_lossy()).unwrap();
    assert!(!current(&b), "a change that only the form's stream shows is seen");
    assert!(embed::read(&PdfDocument::open(&*b.to_string_lossy()).unwrap(), 1, &pdf_id(&doc)).is_none());
}

#[test]
fn a_font_encoding_change_is_part_of_the_fingerprint() {
    let base = dir("catalog-fp-encoding");
    let p = write(&base, "a.pdf", form_text_pdf("encoded words"));
    embed_and_save(&p);
    assert!(current(&p));
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    let mut font = doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Fm0").unwrap().unwrap()
        .get_dict("Resources").unwrap().unwrap().get_dict("Font").unwrap().unwrap().get_dict("F1").unwrap().unwrap();
    font.dict_put("Encoding", PdfObject::new_name("MacRomanEncoding").unwrap()).unwrap();
    let b = base.join("b.pdf");
    doc.save(&b.to_string_lossy()).unwrap();
    assert!(!current(&b), "the same codes read as other letters under another encoding");
}

#[test]
fn a_field_value_kept_on_the_parent_is_part_of_the_fingerprint() {
    let base = dir("catalog-fp-parent");
    let extra = Extra {
        catalog: "/AcroForm << /Fields [7 0 R] /DA (/Helv 0 Tf 0 g) >>".into(),
        annots: "6 0 R".into(),
        objs: vec![
            "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [72 400 300 420] /F 4 /DA (/Helv 12 Tf 0 g) >>".into(),
            "<< /FT /Tx /T (Clause) /V (original value) /Kids [6 0 R] >>".into(),
        ],
    };
    let p = write(&base, "a.pdf", doc_with(&[vec!["body"]], extra));
    embed_and_save(&p);
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    let t = embed::read(&doc, 1, &pdf_id(&doc)).unwrap();
    assert_eq!(t.pages[0].fields.first().map(|f| f.0.as_str()), Some("original value"));
    let mut parent = doc.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap().get_dict("Fields").unwrap().unwrap().get_array(0).unwrap().unwrap();
    parent.dict_put("V", PdfObject::new_string("changed value").unwrap()).unwrap();
    let b = base.join("b.pdf");
    doc.save(&b.to_string_lossy()).unwrap();
    assert!(!current(&b), "a /V changed on the parent field is seen");
}

#[test]
fn fingerprinting_is_bounded_and_the_dict_is_checked_before_the_blob() {
    let base = dir("catalog-fp-bound");
    let p = write(&base, "a.pdf", form_text_pdf("some words"));
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    assert!(embed::fingerprint_with(&doc, 1 << 20, 1 << 20).is_some());
    assert!(embed::fingerprint_with(&doc, 8, 1 << 20).is_none(), "a stream over its cap is not vouched for");
    assert!(embed::fingerprint_with(&doc, 1 << 20, 40).is_none(), "nor are streams over the file's budget");
    // A claim for another page count is "out of date" from the dict alone: the blob is garbage here and never read.
    drop(doc); // Windows can't save over a file that is still open
    embed_and_save(&p);
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    let mut s = doc.catalog().unwrap().get_dict("PieceInfo").unwrap().unwrap().get_dict("Inkwell").unwrap().unwrap().get_dict("Private").unwrap().unwrap();
    s.dict_put("Pages", PdfObject::new_int(99).unwrap()).unwrap();
    s.write_stream_string("not an index at all").unwrap();
    let e = embed::status(&doc);
    assert_eq!((e.present, e.current, e.pages), (true, false, 99));
    assert!(embed::read(&doc, 1, &pdf_id(&doc)).is_none());
}

#[test]
fn comment_and_field_text_is_charged_to_the_fingerprint_budget() {
    let base = dir("catalog-fp-text-budget");
    let long = "x".repeat(4000);
    let extra = Extra {
        catalog: String::new(),
        annots: "6 0 R".into(),
        objs: vec![format!("<< /Type /Annot /Subtype /Text /Rect [72 400 90 420] /Contents ({long}) >>")],
    };
    let p = write(&base, "a.pdf", doc_with(&[vec!["body"]], extra));
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    assert!(embed::fingerprint_with(&doc, 1 << 20, 1 << 20).is_some());
    assert!(embed::fingerprint_with(&doc, 1 << 20, 1000).is_none(), "a long comment is not free");
}

/// One page drawing "words" with font /F1 (object 3 = `font`); `extra` objects follow from number 6.
fn font_pdf(font: &str, page_res: &str, catalog: &str, extra: Vec<String>) -> Vec<u8> {
    let mut o = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        font.to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> {page_res} >> >>"),
        stream_obj("", "BT /F1 12 Tf 72 700 Td (words) Tj ET"),
    ];
    o.extend(extra);
    crate::search::tests::pdf_of(&o)
}

/// Embeds an index in `bytes`, applies `change`, saves, and says whether the index is still current.
fn current_after(name: &str, bytes: Vec<u8>, change: impl FnOnce(&PdfDocument)) -> bool {
    let base = dir(&format!("catalog-fp-{name}"));
    let p = write(&base, "a.pdf", bytes);
    embed_and_save(&p);
    assert!(current(&p), "{name}: the index is current before the change");
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    change(&doc);
    let b = base.join("b.pdf");
    doc.save(&b.to_string_lossy()).unwrap();
    current(&b)
}

fn font_of(doc: &PdfDocument) -> PdfObject {
    doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap().get_dict("Font").unwrap().unwrap().get_dict("F1").unwrap().unwrap()
}

#[test]
fn what_decides_how_codes_read_is_part_of_the_fingerprint() {
    let name = |s: &str| PdfObject::new_name(s).unwrap();

    // An encoding kept in its own object (and its /Differences through another).
    let enc = font_pdf(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding 6 0 R >>",
        "",
        "",
        vec!["<< /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences 7 0 R >>".into(), "[65 /B /C]".into()],
    );
    assert!(!current_after("enc-ref", enc.clone(), |d| font_of(d).get_dict("Encoding").unwrap().unwrap().dict_put("BaseEncoding", name("MacRomanEncoding")).unwrap()));
    assert!(!current_after("enc-diff", enc, |d| {
        let mut diff = font_of(d).get_dict("Encoding").unwrap().unwrap().get_dict("Differences").unwrap().unwrap();
        diff.array_put(1, name("Z")).unwrap();
    }));

    // No /ToUnicode: the font program and a descendant font's CIDSystemInfo decide.
    let prog = font_pdf(
        "<< /Type /Font /Subtype /TrueType /BaseFont /X /FirstChar 32 /LastChar 32 /Widths [500] /FontDescriptor 6 0 R >>",
        "",
        "",
        vec!["<< /Type /FontDescriptor /FontName /X /Flags 32 /FontFile2 7 0 R >>".into(), stream_obj("", "font program bytes")],
    );
    assert!(!current_after("fontfile", prog, |d| {
        let mut ff = font_of(d).get_dict("FontDescriptor").unwrap().unwrap().get_dict("FontFile2").unwrap().unwrap();
        ff.write_stream_string("other font program bytes").unwrap();
    }));
    let cid = font_pdf(
        "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [6 0 R] >>",
        "",
        "",
        vec!["<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> >>".into()],
    );
    assert!(!current_after("cid", cid, |d| {
        let mut sys = font_of(d).get_dict("DescendantFonts").unwrap().unwrap().get_array(0).unwrap().unwrap().get_dict("CIDSystemInfo").unwrap().unwrap();
        sys.dict_put("Ordering", PdfObject::new_string("Japan1").unwrap()).unwrap();
    }));

    // Marked content property lists (/ActualText) and the layers' default visibility.
    let props = font_pdf(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "/Properties << /P1 6 0 R >>",
        "",
        vec!["<< /ActualText (what it says) >>".into()],
    );
    assert!(!current_after("properties", props, |d| {
        let mut p = d.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap().get_dict("Properties").unwrap().unwrap().get_dict("P1").unwrap().unwrap();
        p.dict_put("ActualText", PdfObject::new_string("what it now says").unwrap()).unwrap();
    }));
    let layers = font_pdf(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "",
        "/OCProperties << /OCGs [6 0 R] /D << /Order [6 0 R] /BaseState /ON >> >>",
        vec!["<< /Type /OCG /Name (Notes) >>".into()],
    );
    assert!(!current_after("layers", layers, |d| {
        let mut dflt = d.catalog().unwrap().get_dict("OCProperties").unwrap().unwrap().get_dict("D").unwrap().unwrap();
        dflt.dict_put("BaseState", name("OFF")).unwrap();
    }));
}

#[test]
fn a_resources_dict_shared_by_many_pages_is_walked_once_and_a_huge_one_runs_the_budget_out() {
    // 3000 pages share one Resources (object 3) holding 20000 bare font entries.
    let pages = 3000;
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 4 + i)).collect();
    let fonts: String = (0..20_000).map(|i| format!("/F{i} << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> ")).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        format!("<< /Font << {fonts} >> >>"),
    ];
    objs.extend((0..pages).map(|_| "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources 3 0 R >>".to_string()));
    let base = dir("catalog-fp-shared");
    let p = write(&base, "a.pdf", crate::search::tests::pdf_of(&objs));
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    let t = std::time::Instant::now();
    assert!(embed::fingerprint_with(&doc, 1 << 20, 64 << 20).is_some(), "walked once, well inside the budget");
    assert!(embed::fingerprint_in(&doc, 1 << 20, 64 << 20, 10_000).is_none(), "20000 entries are not free: the walk runs out of steps");
    assert!(t.elapsed().as_secs() < 60, "{:?}", t.elapsed());
}

/// Entries that carry nothing (references to one empty stream, to nothing) still cost steps, however many pages share them.
#[test]
fn empty_content_parts_and_annotation_lists_shared_by_pages_cost_steps() {
    let pages = 50;
    let parts: String = (0..20_000).map(|_| "4 0 R ").collect();
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 6 + i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        format!("[{parts}]"),
        stream_obj("", ""),
        "[]".to_string(),
    ];
    objs.extend((0..pages).map(|_| "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 3 0 R /Annots 3 0 R >>".to_string()));
    let base = dir("catalog-fp-steps");
    let p = write(&base, "a.pdf", crate::search::tests::pdf_of(&objs));
    let doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    let t = std::time::Instant::now();
    assert!(embed::fingerprint_in(&doc, 1 << 20, 64 << 20, 100_000).is_none(), "a million empty parts are not free");
    assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
}

#[test]
fn a_damaged_or_old_catalog_entry_is_listed_and_can_be_rebuilt_or_deleted() {
    let (root, folder) = corpus("meta");
    let (m, d) = make(&root, &folder);
    build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    // Another version: the folder and name survive, the catalog is "not built" and a build brings it back.
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(meta_path(&d)).unwrap()).unwrap();
    v["version"] = 99.into();
    std::fs::write(meta_path(&d), serde_json::to_vec(&v).unwrap()).unwrap();
    let listed = list(&root);
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].id.as_str(), listed[0].name.as_str(), listed[0].built), (m.id.as_str(), "Test", 0));
    assert!(check(&d, &AtomicBool::new(false)).unwrap().unusable);
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(!s.cancelled && s.files > 20);
    assert!(load_meta(&d).unwrap().built > 0);
    assert!(!check(&d, &AtomicBool::new(false)).unwrap().any());
    // Not JSON at all: still listed (so it can be deleted), nothing to build from.
    std::fs::write(meta_path(&d), b"{ nope").unwrap();
    let listed = list(&root);
    assert_eq!((listed.len(), listed[0].id.as_str()), (1, m.id.as_str()));
    delete(&root, &m.id).unwrap();
    assert!(list(&root).is_empty());
}

#[test]
fn a_busy_catalog_cannot_be_deleted_and_stays_held_while_it_is() {
    use super::app::Jobs;
    let jobs = Jobs::default();
    let r: crate::engine::Result<()> = jobs.guarded("abc", || {
        // A build (or another delete) of the same catalog is refused while this one runs.
        assert!(jobs.guarded("abc", || Ok(())).is_err());
        assert!(jobs.guarded("other", || Ok(())).is_ok());
        Ok(())
    });
    assert!(r.is_ok());
    // Released afterwards, also when the work failed.
    assert!(jobs.guarded("abc", || Err::<(), _>(EngineError::Panic)).is_err());
    assert!(jobs.guarded("abc", || Ok(())).is_ok());
}

#[test]
fn a_worker_that_dies_does_not_lose_its_file() {
    let base = dir("catalog-worker-dies");
    let folder = base.join("docs");
    write(&folder, "a.pdf", plain(&[vec!["alpha notice"]]));
    write(&folder, "worker-dies.pdf", plain(&[vec!["beta notice"]]));
    write(&folder, "c.pdf", plain(&[vec!["gamma notice"]]));
    let (_, d) = make(&base.join("catalogs"), &folder);
    let s = build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(!s.cancelled);
    assert_eq!((s.files, s.damaged, s.extracted), (3, 1, 2), "{s:?}");
    let t = load_table(&d, 1).unwrap();
    let died = t.files.iter().find(|r| r.path.ends_with("worker-dies.pdf")).expect("a row for the file");
    assert_eq!(died.status, Status::Damaged);
}

#[test]
fn a_corrupt_index_shows_as_unusable_before_a_refresh_trusts_it() {
    let (_, _, d) = built("check-corrupt");
    assert!(!check(&d, &AtomicBool::new(false)).unwrap().any());
    let p = index_path(&d, 1);
    let mut b = std::fs::read(&p).unwrap();
    let k = b.len() / 2;
    b[k] ^= 0xff;
    std::fs::write(&p, &b).unwrap();
    let st = check(&d, &AtomicBool::new(false)).unwrap();
    assert!(st.unusable && st.any(), "{st:?}");
}

#[test]
fn embedded_bookmarks_that_name_missing_pages_are_dropped() {
    let base = dir("catalog-bookmark-range");
    let p = write(&base, "a.pdf", plain(&[vec!["only page"]]));
    let mut doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
    // (A file without /ID gets one when it is embedded; the index is matched by it.)
    embed::add(&mut doc).unwrap();
    let mut t = read_pages(&doc, &AtomicBool::new(false)).unwrap();
    t.bookmarks = vec![(0, "real".into()), (7, "ghost".into()), (u32::MAX, "worse".into())];
    t.sig = embed::fingerprint(&doc).unwrap();
    embed::install_for_test(&mut doc, &t);
    let got = embed::read(&doc, 1, &pdf_id(&doc)).unwrap();
    assert_eq!(got.bookmarks, vec![(0, "real".to_string())]);
}

// ---- fix round: signatures, redaction, retries, reading, caps ----------------------------------------------

fn done_of(events: &[FilesEvent]) -> (u32, u32, u32) {
    match events.iter().rev().find(|e| matches!(e, FilesEvent::Done { .. })) {
        Some(FilesEvent::Done { matched, skipped, stale, .. }) => (*matched, *skipped, *stale),
        _ => panic!("no Done"),
    }
}

const CERTIFIED: &str = "/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 2 >> >> ] >> >>";
const SIGNED: &str = "/AcroForm << /SigFlags 3 /Fields [] >>";

fn with_catalog(catalog: &str) -> Vec<u8> {
    doc_with(&[vec!["A signed page about notice"]], Extra { catalog: catalog.into(), annots: String::new(), objs: vec![] })
}

fn index_text() -> FileText {
    FileText { version: VERSION, id: "abcd".into(), pages: vec![StoredPage::default()], bookmarks: vec![], cut: false, sig: "x".into() }
}

#[test]
fn a_certified_or_signed_document_gets_no_index_written_updated_or_removed() {
    for (name, catalog, word) in [("certified", CERTIFIED, "certified"), ("signed", SIGNED, "signed")] {
        let base = dir(&format!("catalog-guard-{name}"));
        // Without an index: Embed is refused and nothing is left to save.
        let p = write(&base, "a.pdf", with_catalog(catalog));
        let eng = Engine::new(2, 64 << 20);
        let id = eng.open(&p.to_string_lossy()).unwrap().doc;
        let e = embed::embedded(&eng, id).unwrap();
        assert!(!e.present && e.blocked.as_deref().is_some_and(|w| w.contains(word)), "{name}: {e:?}");
        let err = embed::embed(&eng, id, EditSource::User).unwrap_err().to_string();
        assert!(err.contains(word), "{name}: {err}");
        assert!(eng.edit_state(id).unwrap().undo.is_none(), "{name}: a refusal is not an undo step");
        let before = std::fs::read(&p).unwrap();
        assert!(eng.save(id, None).is_ok());
        assert_eq!(std::fs::read(&p).unwrap(), before, "{name}: the file is as it was");

        // With one (made before it was signed): Update and Remove are refused, the index stays, no step is made.
        let mut doc = PdfDocument::open(&*p.to_string_lossy()).unwrap();
        embed::install_for_test(&mut doc, &index_text());
        let q = base.join("b.pdf");
        doc.save(&q.to_string_lossy()).unwrap();
        let eng = Engine::new(2, 64 << 20);
        let id = eng.open(&q.to_string_lossy()).unwrap().doc;
        let e = embed::embedded(&eng, id).unwrap();
        assert!(e.present && e.blocked.is_some(), "{name}: {e:?}");
        assert!(embed::embed(&eng, id, EditSource::User).unwrap_err().to_string().contains(word));
        assert!(embed::unembed(&eng, id, EditSource::User).unwrap_err().to_string().contains(word));
        assert!(embed::embedded(&eng, id).unwrap().present);
        assert!(eng.edit_state(id).unwrap().undo.is_none(), "{name}: no undo step");
    }
    // A document that is neither is not blocked.
    let base = dir("catalog-guard-open");
    let p = write(&base, "a.pdf", plain(&[vec!["plain"]]));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    assert_eq!(embed::embedded(&eng, id).unwrap().blocked, None);
}

#[test]
fn a_saved_redaction_is_gone_from_the_catalog_and_its_results() {
    use crate::redact::app::{apply, find, mark_hits, ApplyOutcome, FindQuery};
    use crate::redact::Sanitize;
    let base = dir("catalog-redact");
    let folder = base.join("docs");
    let secret = write(&folder, "secret.pdf", plain(&[vec!["The code ZEBRAXYZ is private", "and a line about notice"]]));
    write(&folder, "other.pdf", plain(&[vec!["ZEBRAXYZ is also named in this other file, notice"]]));
    let (_, d) = make(&base.join("catalogs"), &folder);
    build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    let found = |q: &str| flat(&catalog_search(&d, q, SearchOpts::default())).0.values().map(|f| f.0.clone()).collect::<Vec<_>>();
    assert_eq!(found("zebraxyz").len(), 2);
    let contains = |needle: &[u8]| {
        let mut all = std::fs::read(index_path(&d, load_meta(&d).unwrap().generation)).unwrap();
        for e in std::fs::read_dir(text_dir(&d)).unwrap().flatten() {
            // The blobs are compressed: look at what they hold.
            if let Some(t) = FileText::decode(&std::fs::read(e.path()).unwrap()) {
                all.extend(t.pages.iter().flat_map(|p| p.text.text.bytes()));
            }
        }
        all.windows(needle.len()).any(|w| w == needle)
    };
    assert!(contains(b"zebraxyz") || contains(b"ZEBRAXYZ"), "the fixture is indexed");
    assert!(contains(b"private"), "a word only secret.pdf has is in the postings");

    // The document is opened by its canonical path and the catalog lists it by the folder's own spelling.
    let eng = Engine::new(2, 64 << 20);
    let real = std::fs::canonicalize(&secret).unwrap();
    let id = eng.open(&real.to_string_lossy()).unwrap().doc;
    let hits = find(&eng, id, &FindQuery::Text { needle: "ZEBRAXYZ".into(), case_sensitive: true, whole_word: false }).unwrap();
    assert_eq!(hits.len(), 1);
    mark_hits(&eng, id, &hits, EditSource::User).unwrap();
    let out = apply(&eng, id, Sanitize::default(), None, EditSource::User).unwrap();
    assert!(matches!(out, ApplyOutcome::Applied { .. }), "{out:?}");
    let (state, saved) = eng.save_recorded(id, None).unwrap();
    assert!(saved.unwrap().redacts);

    // Until the catalog is told, a search still must not show the old text: the file is not the one that was read.
    let ev = catalog_search(&d, "zebraxyz", SearchOpts::default());
    assert_eq!(found("zebraxyz"), vec!["other.pdf".to_string()]);
    assert_eq!(done_of(&ev), (1, 0, 1), "one file left out as changed");

    // The save path forgets it: row, text and postings.
    let jobs = super::app::Jobs::default();
    assert_eq!(super::app::forget_all(&base.join("catalogs"), &jobs, Path::new(&state.path)), 1);
    assert!(!contains(b"The code"), "the old text of the file is not kept");
    assert!(!contains(b"private"), "its words are not left in the postings (only secret.pdf has them)");
    let ev = catalog_search(&d, "zebraxyz", SearchOpts::default());
    assert_eq!(done_of(&ev), (1, 0, 0));
    assert_eq!(found("notice"), vec!["other.pdf".to_string()]);
    let meta = load_meta(&d).unwrap();
    assert_eq!((meta.files, meta.generation), (1, 2));
    // It comes back, redacted, with the next refresh.
    assert_eq!(check(&d, &AtomicBool::new(false)).unwrap().added, 1);
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.added, s.extracted), (1, 1));
    assert_eq!(found("zebraxyz"), vec!["other.pdf".to_string()]);
    assert_eq!(found("notice").len(), 2);
    // Forgetting a file no catalog lists, or one that is being built, is harmless.
    assert_eq!(super::app::forget_all(&base.join("catalogs"), &jobs, &base.join("nothing.pdf")), 0);
}

#[test]
fn a_changed_file_is_left_out_of_a_catalog_search_and_counted() {
    let (_, folder, d) = built("stale-search");
    let (before, bd) = flat(&catalog_search(&d, "notice", SearchOpts::default()));
    assert!(before.values().any(|f| f.0 == "f00.pdf") && before.values().any(|f| f.0 == "f01.pdf"));
    assert_eq!(done_of(&catalog_search(&d, "notice", SearchOpts::default())).2, 0);
    // Replaced (another size), and touched (same bytes, another time): neither is answered from old text.
    write(&folder, "f00.pdf", plain(&[vec!["rewritten without the word, and a longer line to differ in size"]]));
    let p = folder.join("f01.pdf");
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(3600)).unwrap();
    drop(f);
    let ev = catalog_search(&d, "notice", SearchOpts::default());
    let (now, nd) = flat(&ev);
    let stale = done_of(&ev).2;
    assert!(stale >= 1, "{stale}");
    assert!(nd.1 < bd.1 && now.len() < before.len());
    let names: Vec<_> = now.values().map(|f| f.0.as_str()).collect();
    assert!(!names.contains(&"f00.pdf") && !names.contains(&"f01.pdf"), "{names:?}");
    // A refresh brings them back to date.
    build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!(done_of(&catalog_search(&d, "notice", SearchOpts::default())).2, 0);
}

#[test]
fn a_file_that_failed_is_read_again_by_a_refresh_even_when_it_looks_unchanged() {
    let base = dir("catalog-retry");
    let folder = base.join("docs");
    let good = plain(&[vec!["recovered notice text"]]);
    let p = write(&folder, "a.pdf", vec![b'x'; good.len()]);
    write(&folder, "b.pdf", plain(&[vec!["steady notice"]]));
    let (_, d) = make(&base.join("catalogs"), &folder);
    let s = build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.files, s.damaged, s.extracted), (2, 1, 1));
    // Same size, same modified time (a copy that finished, a lock that let go): only its status can say.
    let t = std::fs::metadata(&p).unwrap().modified().unwrap();
    std::fs::write(&p, &good).unwrap();
    std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_modified(t).unwrap();
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.extracted, s.kept, s.damaged, s.added, s.changed), (1, 1, 0, 0, 0), "{s:?}");
    let (found, _) = flat(&catalog_search(&d, "recovered", SearchOpts::default()));
    assert_eq!(found.len(), 1);
}

#[test]
fn a_file_is_indexed_the_same_read_whole_or_alone() {
    let base = dir("catalog-read");
    let folder = base.join("docs");
    // Over 128 KB, so the sample has a head and a tail.
    let big = doc_with(
        &[vec!["a big file with notice in it"]],
        Extra { catalog: String::new(), annots: String::new(), objs: vec![stream_obj("", &"x".repeat(200_000))] },
    );
    write(&folder, "big.pdf", big);
    write(&folder, "small.pdf", plain(&[vec!["a small notice"]]));
    write(&folder, "bad.pdf", b"nope".to_vec());
    let rows = |max: u64, name: &str| {
        let (_, d) = make(&base.join(name), &folder);
        let s = build_with(&d, true, &AtomicBool::new(false), &no_progress(), max, MAX_FILE).unwrap();
        assert_eq!((s.files, s.extracted, s.damaged), (3, 2, 1), "{s:?}");
        let t = load_table(&d, 1).unwrap();
        (t.files.iter().map(|r| (r.path.clone(), r.size, r.head_tail.clone(), r.pages, r.status)).collect::<Vec<_>>(), flat(&catalog_search(&d, "notice", SearchOpts::default())).0)
    };
    let whole = rows(MAX_READ, "c1");
    // Every file counts as big: read one at a time, the same bytes, so the same rows.
    let alone = rows(0, "c2");
    assert_eq!(whole, alone);
    assert_eq!(whole.1.len(), 2);
}

#[test]
fn a_file_over_the_size_limit_is_listed_as_too_large_and_never_opened() {
    let base = dir("catalog-too-large");
    let folder = base.join("docs");
    let big = doc_with(
        &[vec!["a big file with notice in it"]],
        Extra { catalog: String::new(), annots: String::new(), objs: vec![stream_obj("", &"x".repeat(200_000))] },
    );
    let limit = big.len() as u64 - 1;
    write(&folder, "big.pdf", big);
    write(&folder, "small.pdf", plain(&[vec!["a small notice"]]));
    let (m, d) = make(&base.join("catalogs"), &folder);
    let s = build_with(&d, true, &AtomicBool::new(false), &no_progress(), MAX_READ, limit).unwrap();
    assert_eq!((s.files, s.extracted, s.damaged, s.too_large), (2, 1, 0, 1), "{s:?}");
    let t = load_table(&d, 1).unwrap();
    let row = t.files.iter().find(|r| r.path.ends_with("big.pdf")).unwrap();
    assert_eq!((row.status, row.pages), (Status::TooLarge, 0));
    assert!(!row.head_tail.is_empty(), "a sample is kept, so a move is recognised");
    let meta = load_meta(&d).unwrap();
    assert_eq!((meta.too_large, meta.indexed), (1, 1));
    assert_eq!(flat(&catalog_search(&d, "notice", SearchOpts::default())).0.len(), 1);
    // An entry from before this field existed still loads.
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(meta_path(&d)).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("tooLarge");
    std::fs::write(meta_path(&d), serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(load_meta(&d).unwrap().id, m.id);
    // A refresh with the same limit finds nothing changed.
    let s = build_with(&d, false, &AtomicBool::new(false), &no_progress(), MAX_READ, limit).unwrap();
    assert_eq!((s.added, s.changed, s.removed, s.too_large), (0, 0, 0, 1), "{s:?}");
}

#[test]
fn a_record_that_would_not_decode_is_never_written() {
    let big = FileText {
        version: VERSION,
        id: "ab".into(),
        pages: vec![StoredPage { text: StoredText { text: "a".repeat(MAX_BLOB as usize), ..Default::default() }, ..Default::default() }],
        ..Default::default()
    };
    assert!(big.encode().is_none(), "what decode refuses is not encoded");
    assert!(index_text().encode().is_some_and(|b| FileText::decode(&b).is_some()));
}

#[test]
fn a_file_saved_while_its_catalog_is_busy_is_forgotten_when_the_build_ends() {
    let base = dir("catalog-forget-busy");
    let folder = base.join("docs");
    let p = write(&folder, "a.pdf", plain(&[vec!["the old words, secretive notice"]]));
    write(&folder, "b.pdf", plain(&[vec!["another notice"]]));
    let root = base.join("catalogs");
    let (m, d) = make(&root, &folder);
    build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    let jobs = super::app::Jobs::default();
    let r = jobs.guarded(&m.id, || {
        assert_eq!(super::app::forget_all(&root, &jobs, &p), 0, "nothing runs against a catalog that is held");
        Ok(())
    });
    assert!(r.is_ok());
    assert_eq!(load_table(&d, 1).unwrap().files.len(), 2);
    super::app::forget_deferred(&jobs, &m.id, &d);
    assert_eq!(load_meta(&d).unwrap().files, 1);
    assert_eq!(flat(&catalog_search(&d, "secretive", SearchOpts::default())).0.len(), 0);
    // Nothing is left to do the second time.
    super::app::forget_deferred(&jobs, &m.id, &d);
    assert_eq!(load_meta(&d).unwrap().generation, 2);

    // A save that lands while another is being handled is done by that one's forget_all, not left for a build.
    let c = write(&folder, "c.pdf", plain(&[vec!["a third file with a rare word: quokka"]]));
    build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    let r = jobs.guarded(&m.id, || {
        assert_eq!(super::app::forget_all(&root, &jobs, &c), 0);
        Ok(())
    });
    assert!(r.is_ok());
    assert_eq!(flat(&catalog_search(&d, "quokka", SearchOpts::default())).0.len(), 1);
    assert_eq!(super::app::forget_all(&root, &jobs, &base.join("nothing.pdf")), 0, "nothing of its own to drop");
    assert_eq!(flat(&catalog_search(&d, "quokka", SearchOpts::default())).0.len(), 0, "the deferred one ran with the next forget");
}

#[test]
fn a_catalog_that_cannot_be_rewritten_drops_its_index_rather_than_keep_a_forgotten_files_words() {
    let (_, folder, d) = built("forget-closed");
    let p = folder.join("f03.pdf");
    let idx = index_path(&d, 1);
    let mut b = std::fs::read(&idx).unwrap();
    let k = b.len() / 2;
    b[k] ^= 0xff;
    std::fs::write(&idx, &b).unwrap();
    assert!(forget(&d, &p).is_err());
    assert!(!idx.exists(), "the postings are not left on disk");
    assert!(check(&d, &AtomicBool::new(false)).unwrap().unusable);
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(!s.cancelled && s.extracted > 20);
    assert!(!flat(&catalog_search(&d, "notice", SearchOpts::default())).0.is_empty());
}

#[test]
fn a_catalog_whose_entry_is_unreadable_keeps_no_stored_text_when_a_file_is_forgotten() {
    let (_, folder, d) = built("forget-meta");
    assert!(std::fs::read_dir(text_dir(&d)).unwrap().count() > 20);
    std::fs::write(meta_path(&d), b"{ nope").unwrap();
    assert!(load_meta(&d).is_err());
    assert_eq!(forget(&d, &folder.join("f03.pdf")).unwrap(), 0);
    assert_eq!(std::fs::read_dir(text_dir(&d)).unwrap().count(), 0, "no blob is left");
    assert!(!index_path(&d, 1).exists(), "nor the postings");
    // It is still listed (so it can be deleted); an entry that is another version's can be built again.
    assert_eq!(list(d.parent().unwrap()).len(), 1);
}

#[test]
fn a_forget_waiting_for_a_busy_catalog_survives_a_restart() {
    let base = dir("catalog-forget-restart");
    let folder = base.join("docs");
    let p = write(&folder, "a.pdf", plain(&[vec!["pre-redaction words: quokka"]]));
    write(&folder, "b.pdf", plain(&[vec!["another notice"]]));
    let root = base.join("catalogs");
    let (m, d) = make(&root, &folder);
    build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    let jobs = super::app::Jobs::default();
    jobs.guarded(&m.id, || {
        assert_eq!(super::app::forget_all(&root, &jobs, &p), 0);
        assert_eq!(pending(&d), Waiting::Paths(vec![p.clone()]), "the path is on disk, in the catalog's folder");
        Ok(())
    })
    .unwrap();
    // The app quit and started again: a new Jobs, the same folder. Nothing in memory remembers.
    drop(jobs);
    let again = super::app::Jobs::default();
    super::app::replay_pending(&root, &again);
    assert_eq!(pending(&d), Waiting::Paths(vec![]));
    assert_eq!(load_meta(&d).unwrap().files, 1);
    assert_eq!(flat(&catalog_search(&d, "quokka", SearchOpts::default())).0.len(), 0);
    // A failed forget stays on the list for the next hold (the entry here is not a catalog's).
    defer_forget(&d, &base.join("c.pdf")).unwrap();
    settle_pending(&d, &[]).unwrap();
    assert_eq!(pending(&d), Waiting::Paths(vec![base.join("c.pdf")]));
    settle_pending(&d, &[base.join("c.pdf")]).unwrap();
    assert!(!pending_path(&d).exists());
}

#[test]
fn a_file_that_grew_after_it_was_listed_is_checked_against_the_limit_by_what_is_read() {
    let base = dir("catalog-grew");
    let folder = base.join("docs");
    let small = plain(&[vec!["a small notice"]]);
    let limit = small.len() as u64 + 10;
    let grown = doc_with(
        &[vec!["a grown notice"]],
        Extra { catalog: String::new(), annots: String::new(), objs: vec![stream_obj("", &"x".repeat(200_000))] },
    );
    let p = write(&folder, "a.pdf", small);
    write(&folder, "b.pdf", plain(&[vec!["another notice"]]));
    let (_, d) = make(&base.join("catalogs"), &folder);
    *BEFORE_OPEN.lock().unwrap() = Some((p.clone(), grown));
    let s = build_with(&d, true, &AtomicBool::new(false), &no_progress(), MAX_READ, limit).unwrap();
    assert!(BEFORE_OPEN.lock().unwrap().is_none(), "the file was replaced after the listing");
    assert_eq!((s.files, s.extracted, s.too_large, s.damaged), (2, 1, 1, 0), "{s:?}");
    assert_eq!(flat(&catalog_search(&d, "grown", SearchOpts::default())).0.len(), 0);
}

#[test]
fn a_file_that_grows_while_it_is_read_is_read_again_under_the_big_file_lock() {
    let base = dir("catalog-grows-reading");
    let folder = base.join("docs");
    let small = plain(&[vec!["a small notice"]]);
    let grown = doc_with(
        &[vec!["a grown notice"]],
        Extra { catalog: String::new(), annots: String::new(), objs: vec![stream_obj("", &"x".repeat(200_000))] },
    );
    let (max_read, max_file) = (small.len() as u64 + 10, grown.len() as u64 + 10);
    let p = write(&folder, "a.pdf", small);
    let q = write(&folder, "b.pdf", plain(&[vec!["another notice"]]));
    let (_, d) = make(&base.join("catalogs"), &folder);
    *AFTER_LEN.lock().unwrap() = Some((p.clone(), grown));
    let s = build_with(&d, true, &AtomicBool::new(false), &no_progress(), max_read, max_file).unwrap();
    assert!(AFTER_LEN.lock().unwrap().is_none(), "the file was replaced after its size was taken");
    assert_eq!((s.files, s.extracted, s.too_large, s.damaged), (2, 2, 0, 0), "{s:?}");
    // The whole grown file was read, so its text is stored (a search leaves it out: it changed since the listing);
    // the one that did not grow never took the lock.
    let row = load_table(&d, 1).unwrap().files.into_iter().find(|r| r.path.ends_with("a.pdf")).unwrap();
    let text = FileText::decode(&std::fs::read(text_path(&d, row.fid)).unwrap()).unwrap();
    assert!(serde_json::to_string(&text).unwrap().contains("grown"));
    let alone = READ_ALONE.lock().unwrap();
    assert!(alone.contains(&p) && !alone.contains(&q), "{alone:?}");
}

#[test]
fn a_file_that_grows_past_the_limit_while_it_is_read_is_too_large() {
    let base = dir("catalog-grows-past");
    let folder = base.join("docs");
    let small = plain(&[vec!["a small notice"]]);
    let grown = doc_with(
        &[vec!["a grown notice"]],
        Extra { catalog: String::new(), annots: String::new(), objs: vec![stream_obj("", &"x".repeat(200_000))] },
    );
    let limit = small.len() as u64 + 10;
    let p = write(&folder, "a.pdf", small);
    let (_, d) = make(&base.join("catalogs"), &folder);
    *AFTER_LEN.lock().unwrap() = Some((p, grown));
    let s = build_with(&d, true, &AtomicBool::new(false), &no_progress(), limit, limit).unwrap();
    assert_eq!((s.files, s.extracted, s.too_large, s.damaged), (1, 0, 1, 0), "{s:?}");
}

#[test]
fn a_forget_asked_for_just_after_the_holder_read_the_list_is_done_when_it_lets_go() {
    let base = dir("catalog-forget-wakeup");
    let folder = base.join("docs");
    let a = write(&folder, "a.pdf", plain(&[vec!["first words: quokka"]]));
    let b = write(&folder, "b.pdf", plain(&[vec!["second words: wombat"]]));
    write(&folder, "c.pdf", plain(&[vec!["another notice"]]));
    let root = base.join("catalogs");
    let (m, d) = make(&root, &folder);
    build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    let jobs = Arc::new(super::app::Jobs::default());
    defer_forget(&d, &a).unwrap();
    // While the holder has the catalog (it has read a.pdf), a save of b.pdf asks to forget: it finds the catalog
    // held and writes the path down. Nothing else will look at that list until some later hold.
    let (j2, r2, b2) = (jobs.clone(), root.clone(), b.clone());
    WHILE_HELD.with(|h| *h.borrow_mut() = Some(Box::new(move || assert_eq!(super::app::forget_all(&r2, &j2, &b2), 0))));
    super::app::forget_deferred(&jobs, &m.id, &d);
    assert_eq!(pending(&d), Waiting::Paths(vec![]));
    assert_eq!(load_meta(&d).unwrap().files, 1);
    assert_eq!(flat(&catalog_search(&d, "quokka", SearchOpts::default())).0.len(), 0);
    assert_eq!(flat(&catalog_search(&d, "wombat", SearchOpts::default())).0.len(), 0);
}

#[test]
fn a_wipe_kept_only_in_memory_is_done_when_the_catalogs_are_listed() {
    let base = dir("catalog-replay-mem");
    let folder = base.join("docs");
    write(&folder, "a.pdf", plain(&[vec!["words that must go: quokka"]]));
    let root = base.join("catalogs");
    let (_, d) = make(&root, &folder);
    build(&d, true, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(blobs(&d) > 0);
    // The marker file could not be written: only this process knows, and no list or marker is on disk.
    WIPE_MEM.lock().unwrap().push(d.clone());
    assert!(!marker(&d).exists() && !pending_path(&d).exists());
    super::app::replay_pending(&root, &super::app::Jobs::default());
    assert_eq!(blobs(&d), 0);
    assert!(!WIPE_MEM.lock().unwrap().iter().any(|p| p == &d));
}

fn blobs(d: &Path) -> usize {
    std::fs::read_dir(text_dir(d)).unwrap().count()
}

fn marker(d: &Path) -> PathBuf {
    d.join("forget-wipe")
}

#[test]
fn a_full_pending_list_asks_for_a_wipe_instead_of_dropping_a_path() {
    let (_, folder, d) = built("pending-full");
    assert!(blobs(&d) > 20);
    defer_forget_max(&d, &folder.join("a.pdf"), 2).unwrap();
    defer_forget_max(&d, &folder.join("b.pdf"), 2).unwrap();
    assert!(matches!(pending(&d), Waiting::Paths(l) if l.len() == 2));
    defer_forget_max(&d, &folder.join("c.pdf"), 2).unwrap();
    // Not wiped here: the catalog is held by someone, who would write it back.
    assert_eq!(pending(&d), Waiting::Wipe);
    assert!(blobs(&d) > 20 && index_path(&d, 1).exists());
    // Whoever frees the catalog does it.
    wipe_pending(&d).unwrap();
    assert_eq!((blobs(&d), pending(&d)), (0, Waiting::Paths(vec![])));
    assert!(!index_path(&d, 1).exists() && !marker(&d).exists() && !pending_path(&d).exists());
    // A refresh reads every file again (the rows' text is gone) and the catalog answers.
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(s.extracted > 20 && s.kept == 0, "{s:?}");
    assert!(!flat(&catalog_search(&d, "notice", SearchOpts::default())).0.is_empty());
}

#[test]
fn an_unreadable_pending_list_is_not_taken_for_an_empty_one() {
    // Read: it asks for a wipe, and nothing is wiped by the read.
    let (_, folder, d) = built("pending-corrupt");
    std::fs::write(pending_path(&d), b"{ nope").unwrap();
    assert_eq!(pending(&d), Waiting::Wipe);
    assert!(blobs(&d) > 20);
    // Added to or settled: the earlier entries it held are not overwritten, the wipe is asked for.
    defer_forget(&d, &folder.join("a.pdf")).unwrap();
    settle_pending(&d, &[]).unwrap();
    assert!(marker(&d).exists() && blobs(&d) > 20);
    wipe_pending(&d).unwrap();
    assert_eq!((blobs(&d), pending(&d)), (0, Waiting::Paths(vec![])));
    assert!(!pending_path(&d).exists() && !marker(&d).exists());

    // A missing file is simply empty.
    let (_, _, d) = built("pending-missing");
    assert_eq!(pending(&d), Waiting::Paths(vec![]));
    assert!(blobs(&d) > 20);
}

#[test]
fn a_pending_list_that_cannot_be_written_asks_for_a_wipe() {
    // The list's name is taken by a folder: it can be neither read nor written.
    let (_, folder, d) = built("pending-unwritable");
    std::fs::create_dir_all(pending_path(&d)).unwrap();
    defer_forget(&d, &folder.join("a.pdf")).unwrap();
    assert_eq!(pending(&d), Waiting::Wipe);
    assert!(blobs(&d) > 20);
    wipe_pending(&d).unwrap();
    assert_eq!((blobs(&d), pending(&d)), (0, Waiting::Paths(vec![])));
}

#[cfg(unix)]
#[test]
fn a_wipe_that_cannot_be_asked_for_on_disk_is_kept_in_memory_and_one_that_fails_stays_asked_for() {
    use std::os::unix::fs::PermissionsExt;
    let (_, folder, d) = built("pending-readonly");
    let mode = |m| std::fs::set_permissions(&d, std::fs::Permissions::from_mode(m)).unwrap();
    // Nothing can be written in the catalog's folder: the marker is kept in memory and the error is reported.
    mode(0o555);
    let r = defer_forget(&d, &folder.join("a.pdf"));
    // The same folder cannot be wiped either: the wipe fails, and what asked for it stays.
    let w = wipe_pending(&d);
    mode(0o755);
    assert!(r.is_err(), "not even the marker could be written");
    assert!(w.is_err(), "a wipe that failed is not done");
    assert!(index_path(&d, 1).exists());
    assert_eq!(pending(&d), Waiting::Wipe, "still asked for");
    wipe_pending(&d).unwrap();
    assert_eq!((blobs(&d), pending(&d)), (0, Waiting::Paths(vec![])));

    // A catalog with no readable entry that cannot be wiped is an error, not "0 rows dropped".
    let (_, folder, d) = built("forget-wipe-fails");
    std::fs::write(meta_path(&d), b"{ nope").unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
    let f = forget(&d, &folder.join("a.pdf"));
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(f.is_err());
}

#[test]
fn a_wipe_asked_for_while_a_build_holds_the_catalog_runs_after_the_build() {
    let (root, folder, d) = built("pending-wipe-after-build");
    let id = load_meta(&d).unwrap().id;
    let jobs = super::app::Jobs::default();
    // A file that changes, so the build in the middle writes a generation with the kept files' words in it.
    write(&folder, "f00.pdf", plain(&[vec!["a changed file, quokka"]]));
    jobs.guarded(&id, || {
        std::fs::write(pending_path(&d), b"{ nope").unwrap();
        assert_eq!(super::app::forget_all(&root, &jobs, &folder.join("f01.pdf")), 0);
        assert!(blobs(&d) > 20 && index_path(&d, 1).exists(), "nothing is wiped under someone else's hold");
        let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
        assert!(s.kept > 20, "{s:?}");
        Ok(())
    })
    .unwrap();
    assert!(index_path(&d, 2).exists(), "the build wrote the old words back");
    assert!(marker(&d).exists(), "and the wipe is still asked for");
    super::app::forget_deferred(&jobs, &id, &d);
    assert_eq!(blobs(&d), 0);
    assert!(!index_path(&d, 2).exists() && !marker(&d).exists() && !pending_path(&d).exists());
    assert!(check(&d, &AtomicBool::new(false)).unwrap().unusable);
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert!(s.extracted > 20, "{s:?}");
    assert!(!flat(&catalog_search(&d, "quokka", SearchOpts::default())).0.is_empty());
}

#[test]
fn a_kept_row_whose_stored_text_is_gone_is_read_again() {
    let (_, _, d) = built("missing-blob");
    let t = load_table(&d, 1).unwrap();
    let r = t.files.iter().find(|r| r.status == Status::Indexed).unwrap();
    std::fs::remove_file(text_path(&d, r.fid)).unwrap();
    let s = build(&d, false, &AtomicBool::new(false), &no_progress()).unwrap();
    assert_eq!((s.extracted, s.added), (1, 0), "{s:?}");
    assert!(read_text(&d, load_table(&d, 2).unwrap().files.iter().find(|x| x.path == r.path).unwrap().fid).is_some());
}
