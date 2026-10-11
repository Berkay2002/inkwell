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
    let n = load_meta(&d).unwrap().files;

    // Nothing changed: nothing is opened.
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
    assert_eq!(flat(&catalog_search(&d, "notice", SearchOpts::default())), before);
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
    let bytes = text.encode();
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
    let open = || PdfDocument::open(&*p).unwrap();
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
    let after = PdfDocument::open(&*tmp).unwrap();
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
    embed::status(&PdfDocument::open(p).unwrap()).current
}

#[test]
fn text_only_in_a_form_xobject_is_part_of_the_fingerprint() {
    let base = dir("catalog-fp-form");
    let p = write(&base, "a.pdf", form_text_pdf("the form words"));
    embed_and_save(&p);
    assert!(current(&p));
    let doc = PdfDocument::open(&*p).unwrap();
    let t = embed::read(&doc, 1, &pdf_id(&doc)).unwrap();
    assert!(t.pages[0].text.text.contains("the form words"), "the form's text is in the index");
    let mut fm = doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Fm0").unwrap().unwrap();
    fm.write_stream_string("BT /F1 12 Tf 72 700 Td (the CHANGED words) Tj ET").unwrap();
    let b = base.join("b.pdf");
    doc.save(&b.to_string_lossy()).unwrap();
    assert!(!current(&b), "a change that only the form's stream shows is seen");
    assert!(embed::read(&PdfDocument::open(&*b).unwrap(), 1, &pdf_id(&doc)).is_none());
}

#[test]
fn a_font_encoding_change_is_part_of_the_fingerprint() {
    let base = dir("catalog-fp-encoding");
    let p = write(&base, "a.pdf", form_text_pdf("encoded words"));
    embed_and_save(&p);
    assert!(current(&p));
    let doc = PdfDocument::open(&*p).unwrap();
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
    let doc = PdfDocument::open(&*p).unwrap();
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
    let doc = PdfDocument::open(&*p).unwrap();
    assert!(embed::fingerprint_with(&doc, 1 << 20, 1 << 20).is_some());
    assert!(embed::fingerprint_with(&doc, 8, 1 << 20).is_none(), "a stream over its cap is not vouched for");
    assert!(embed::fingerprint_with(&doc, 1 << 20, 40).is_none(), "nor are streams over the file's budget");
    // A claim for another page count is "out of date" from the dict alone: the blob is garbage here and never read.
    embed_and_save(&p);
    let doc = PdfDocument::open(&*p).unwrap();
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
    let doc = PdfDocument::open(&*p).unwrap();
    assert!(embed::fingerprint_with(&doc, 1 << 20, 1 << 20).is_some());
    assert!(embed::fingerprint_with(&doc, 1 << 20, 1000).is_none(), "a long comment is not free");
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
    let mut doc = PdfDocument::open(&*p).unwrap();
    // (A file without /ID gets one when it is embedded; the index is matched by it.)
    embed::add(&mut doc).unwrap();
    let mut t = read_pages(&doc, &AtomicBool::new(false)).unwrap();
    t.bookmarks = vec![(0, "real".into()), (7, "ghost".into()), (u32::MAX, "worse".into())];
    t.sig = embed::fingerprint(&doc).unwrap();
    embed::install_for_test(&mut doc, &t);
    let got = embed::read(&doc, 1, &pdf_id(&doc)).unwrap();
    assert_eq!(got.bookmarks, vec![(0, "real".to_string())]);
}
