//! The find options, snippets, comments, fields and bookmarks, the engine events, and folder search
//! (listing, caps, cancel), on small PDFs built here.

use super::files::{list_pdfs, list_picked, search_file, FilesEvent, FolderSearches, Skip, PER_FILE, TOTAL};
use super::*;
use crate::engine::{DocId, Engine, SearchEvent};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

pub(super) fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("search-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A PDF from object bodies (object `n` is `objs[n - 1]`; object 1 is the catalog).
pub(super) fn pdf_of(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A string literal in WinAnsi: Latin-1 letters become octal escapes.
pub(super) fn lit(s: &str) -> String {
    let mut o = String::from("(");
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) > 127 => o.push_str(&format!("\\{:03o}", c as u32)),
            c => o.push(c),
        }
    }
    o.push(')');
    o
}

/// The content stream of a page: each string is one line of text, 16 points apart.
pub(super) fn content(lines: &[&str]) -> String {
    let mut s = String::from("BT /F1 12 Tf 72 700 Td 16 TL\n");
    for l in lines {
        s.push_str(&format!("{} Tj T*\n", lit(l)));
    }
    s.push_str("ET");
    format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len())
}

/// Extra catalog entries, page annotations (object numbers from `first_extra`) and extra objects.
pub(super) struct Extra {
    pub(super) catalog: String,
    pub(super) annots: String,
    pub(super) objs: Vec<String>,
}

pub(super) fn no_extra() -> Extra {
    Extra { catalog: String::new(), annots: String::new(), objs: Vec::new() }
}

/// Pages of text lines. Objects: 1 catalog, 2 pages, 3 font, then per page (page, content), then `extra.objs`.
pub(super) fn doc_with(pages: &[Vec<&str>], extra: Extra) -> Vec<u8> {
    let first_extra = 4 + pages.len() * 2;
    let kids: String = (0..pages.len()).map(|i| format!("{} 0 R ", 4 + i * 2)).collect();
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {} >>", extra.catalog),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    for (i, l) in pages.iter().enumerate() {
        let annots = if i == 0 && !extra.annots.is_empty() { format!("/Annots [{}]", extra.annots) } else { String::new() };
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R {annots} >>",
            5 + i * 2
        ));
        objs.push(content(l));
    }
    assert_eq!(objs.len() + 1, first_extra);
    objs.extend(extra.objs);
    pdf_of(&objs)
}

pub(super) fn plain(pages: &[Vec<&str>]) -> Vec<u8> {
    doc_with(pages, no_extra())
}

pub(super) fn write(d: &Path, name: &str, bytes: Vec<u8>) -> PathBuf {
    let p = d.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, bytes).unwrap();
    p
}

fn opts() -> SearchOpts {
    SearchOpts::default()
}

/// Hits of `query` on every page of `bytes`, through the same path the engine uses.
fn hits(bytes: &[u8], query: &str, o: SearchOpts) -> Vec<(u32, Hit)> {
    let doc = Document::from_bytes(bytes, "application/pdf").unwrap();
    let q = Query::new(query, o).expect("a query");
    let mut out = Vec::new();
    for p in 0..doc.page_count().unwrap() as u32 {
        for h in find_in_page(&doc, p, &q).unwrap() {
            out.push((p, h));
        }
    }
    if o.bookmarks {
        out.extend(bookmark_hits(&doc, &q).unwrap());
    }
    out
}

fn count(bytes: &[u8], query: &str, o: SearchOpts) -> usize {
    hits(bytes, query, o).len()
}

#[test]
fn a_plain_search_ignores_case_and_gives_the_words_around_the_hit() {
    let b = plain(&[vec!["The notice period is thirty days.", "Notice must be written."]]);
    let h = hits(&b, "notice", opts());
    assert_eq!(h.len(), 2);
    let (page, first) = &h[0];
    assert_eq!(*page, 0);
    assert_eq!((first.before.as_str(), first.text.as_str()), ("The ", "notice"));
    assert!(first.after.starts_with(" period is thirty days."), "{:?}", first.after);
    assert_eq!(first.source, Source::Text);
    assert_eq!(first.rects.len(), 1);
    let r = first.rects[0];
    assert!(r[2] > r[0] && r[3] > r[1] && r[2] <= 612.0 && r[3] <= 792.0, "{r:?}");
    // The second hit is on the next line, so lower on the page and capitalised as the page has it.
    assert_eq!(h[1].1.text, "Notice");
    assert!(h[1].1.rects[0][1] > r[1]);
}

#[test]
fn match_case_and_whole_words() {
    let b = plain(&[vec!["notice Notice noticed unnoticed", "a notice, (notice) notice-board"]]);
    assert_eq!(count(&b, "notice", opts()), 7);
    assert_eq!(count(&b, "notice", SearchOpts { case_sensitive: true, ..opts() }), 6);
    assert_eq!(count(&b, "Notice", SearchOpts { case_sensitive: true, ..opts() }), 1);
    // Whole words: "noticed" and "unnoticed" are not it; punctuation and a hyphen end a word.
    assert_eq!(count(&b, "notice", SearchOpts { whole_words: true, ..opts() }), 5);
    assert_eq!(count(&b, "notice", SearchOpts { whole_words: true, case_sensitive: true, ..opts() }), 4);
}

#[test]
fn ignore_accents_folds_marks_and_keeps_them_otherwise() {
    let b = plain(&[vec!["un café, un Café and a cafe", "naïve résumé"]]);
    assert_eq!(count(&b, "cafe", opts()), 1);
    assert_eq!(count(&b, "café", opts()), 2);
    assert_eq!(count(&b, "cafe", SearchOpts { ignore_accents: true, ..opts() }), 3);
    assert_eq!(count(&b, "café", SearchOpts { ignore_accents: true, ..opts() }), 3);
    assert_eq!(count(&b, "resume", SearchOpts { ignore_accents: true, ..opts() }), 1);
    assert_eq!(count(&b, "naive", opts()), 0);
    // Case still counts when asked for, accents ignored.
    let both = SearchOpts { ignore_accents: true, case_sensitive: true, ..opts() };
    assert_eq!(count(&b, "Cafe", both), 1);
}

#[test]
fn a_phrase_runs_over_a_line_break_and_covers_a_rect_per_line() {
    let b = plain(&[vec!["Either party may give written", "notice of at least thirty days."]]);
    let h = hits(&b, "written notice", opts());
    assert_eq!(h.len(), 1, "{h:?}");
    let hit = &h[0].1;
    assert_eq!(hit.text, "written notice");
    assert_eq!(hit.rects.len(), 2);
    assert!(hit.rects[1][1] > hit.rects[0][1], "second rect is a line lower");
    // Runs of white space in the query do not matter.
    assert_eq!(count(&b, "written    notice", opts()), 1);
}

#[test]
fn a_hyphen_at_a_line_end_joins_the_word() {
    let b = plain(&[vec!["both parties will co-", "operate fully with the audit"]]);
    assert_eq!(count(&b, "cooperate", opts()), 1);
    assert_eq!(count(&b, "operate fully", opts()), 1);
    // A hyphen inside a line is the author's.
    let c = plain(&[vec!["a well-known term"]]);
    assert_eq!(count(&c, "well-known", opts()), 1);
    assert_eq!(count(&c, "wellknown", opts()), 0);
}

#[test]
fn comments_form_fields_and_bookmarks_are_searched_only_when_asked() {
    let extra = Extra {
        catalog: "/Outlines 8 0 R /AcroForm << /Fields [7 0 R] /DA (/Helv 0 Tf 0 g) >>".into(),
        annots: "6 0 R 7 0 R".into(),
        objs: vec![
            // 6: a sticky note, 7: a text field with a value, 8: outline root, 9: its one item
            "<< /Type /Annot /Subtype /Text /Rect [300 600 320 620] /Contents (Call about the NOTICE period) /T (Reviewer) /F 4 >>".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Clause) /V (notice given) /Rect [72 400 300 420] /F 4 /DA (/Helv 12 Tf 0 g) >>".into(),
            "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>".into(),
            "<< /Title (Termination and notice) /Parent 8 0 R /Dest [4 0 R /XYZ 0 792 0] >>".into(),
        ],
    };
    let b = doc_with(&[vec!["Nothing relevant on the page."]], extra);
    assert_eq!(count(&b, "notice", opts()), 0, "text only by default");

    let c = hits(&b, "notice", SearchOpts { comments: true, ..opts() });
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].1.source, Source::Comment);
    assert_eq!(c[0].1.text, "NOTICE");
    assert_eq!(c[0].1.before, "Call about the ");
    // The note's rectangle, in page space with the origin at the top left.
    let r = c[0].1.rects[0];
    assert!((r[0] - 300.0).abs() < 1.0 && (r[1] - (792.0 - 620.0)).abs() < 1.0, "{r:?}");

    let f = hits(&b, "notice", SearchOpts { fields: true, ..opts() });
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].1.source, Source::Field);
    assert_eq!(f[0].1.after, " given");

    let m = hits(&b, "notice", SearchOpts { bookmarks: true, ..opts() });
    assert_eq!(m.len(), 1);
    assert_eq!((m[0].0, m[0].1.source), (0, Source::Bookmark));
    assert!(m[0].1.rects.is_empty());
    assert_eq!(m[0].1.before, "Termination and ");

    let all = SearchOpts { comments: true, fields: true, bookmarks: true, ..opts() };
    assert_eq!(count(&b, "notice", all), 3);
}

#[test]
fn queries_that_are_empty_or_huge_are_safe() {
    assert!(Query::new("", opts()).is_none());
    assert!(Query::new("   \t ", opts()).is_none());
    let b = plain(&[vec!["short page"]]);
    let long = "word ".repeat(5000);
    assert_eq!(count(&b, &long, opts()), 0);
    // A bare accent with accents ignored folds to nothing: nothing to look for.
    assert!(Query::new("\u{301}", SearchOpts { ignore_accents: true, ..opts() }).is_none());
}

#[test]
fn a_page_lists_at_most_a_bounded_number_of_hits() {
    let line = "a ".repeat(40);
    let lines: Vec<&str> = (0..60).map(|_| line.as_str()).collect();
    let b = plain(&[lines]);
    // 60 lines of 40 hits would be 2400; the page stops at its cap.
    assert_eq!(count(&b, "a", opts()), 2000);
}

#[test]
fn the_engine_streams_hits_with_snippets_and_options_per_page() {
    let d = dir("engine");
    let p = write(&d, "a.pdf", plain(&[vec!["First page, no match."], vec!["The NOTICE and the notice."], vec!["noticed"]]));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;

    let run = |query: &str, o: SearchOpts| -> (Vec<(u32, Vec<Hit>)>, u32) {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        eng.search_with(id, query.into(), o, Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }))
        .unwrap();
        let mut pages = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
                SearchEvent::Hits { page, hits, .. } => pages.push((page, hits)),
                SearchEvent::Progress { .. } => {}
                SearchEvent::Done { hits, .. } => {
                    pages.sort_by_key(|p| p.0);
                    return (pages, hits);
                }
            }
        }
    };
    let (pages, total) = run("notice", opts());
    assert_eq!(total, 3);
    assert_eq!(pages.iter().map(|p| p.0).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(pages[0].1.len(), 2);
    assert_eq!(pages[0].1[0].before, "The ");

    let (_, total) = run("notice", SearchOpts { case_sensitive: true, whole_words: true, ..opts() });
    assert_eq!(total, 1);
    let (pages, total) = run("   ", opts());
    assert_eq!((pages.len(), total), (0, 0));
}

#[test]
fn the_engine_search_of_bookmarks_reports_the_page_they_go_to() {
    let extra = Extra {
        catalog: "/Outlines 8 0 R".into(),
        annots: String::new(),
        objs: vec![
            // 7 is unused padding so the outline objects land at 8 and 9 (3 pages = objects 4..9)
            "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>".into(),
            "<< /Title (Notice of termination) /Parent 8 0 R /Dest [6 0 R /XYZ 0 792 0] >>".into(),
        ],
    };
    // pages are objects 4,6 (+ contents 5,7); outline root is 8, item 9.
    let b = doc_with(&[vec!["one"], vec!["two"]], extra);
    let d = dir("bookmarks");
    let p = write(&d, "a.pdf", b);
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    eng.search_with(id, "notice".into(), SearchOpts { bookmarks: true, ..opts() }, Arc::new(move |e| {
        let _ = tx.lock().unwrap().send(e);
    }))
    .unwrap();
    let mut got = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            SearchEvent::Hits { page, hits, .. } => got.push((page, hits[0].source)),
            SearchEvent::Progress { .. } => {}
            SearchEvent::Done { hits, .. } => {
                assert_eq!(hits, 1);
                break;
            }
        }
    }
    assert_eq!(got, vec![(1, Source::Bookmark)]);
}

// ---- search across files -------------------------------------------------------------------------

/// The window label the folder searches in these tests belong to.
const WIN: &str = "main";

/// Runs a folder search to its `Done` and returns every event.
fn run_files(files: Vec<PathBuf>, query: &str, o: SearchOpts) -> Vec<FilesEvent> {
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

fn tree(test: &str) -> PathBuf {
    let d = dir(test);
    write(&d, "a.pdf", plain(&[vec!["notice one", "and notice two"], vec!["nothing"]]));
    write(&d, "b.PDF", plain(&[vec!["no match here"]]));
    write(&d, "notes.txt", b"notice in a text file".to_vec());
    write(&d, "sub/c.pdf", plain(&[vec!["a Notice in a sub-folder"]]));
    write(&d, "sub/deeper/d.pdf", plain(&[vec!["notice notice notice"]]));
    write(&d, "broken.pdf", b"this is not a pdf".to_vec());
    d
}

#[test]
fn a_folder_lists_its_pdfs_with_or_without_sub_folders() {
    let d = tree("list");
    let names = |v: Vec<PathBuf>| v.iter().map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/")).collect::<Vec<_>>();
    let go = AtomicBool::new(false);
    let flat = list_pdfs(&d, false, &go);
    assert!(!flat.capped && !flat.cancelled && flat.cloud_only == 0);
    assert_eq!(names(flat.files), vec!["a.pdf", "b.PDF", "broken.pdf"]);
    let deep = list_pdfs(&d, true, &go);
    assert_eq!(names(deep.files), vec!["a.pdf", "b.PDF", "broken.pdf", "sub/c.pdf", "sub/deeper/d.pdf"]);
}

#[test]
fn a_folder_search_groups_hits_by_file_and_skips_what_it_cannot_read() {
    let d = tree("run");
    let files = list_pdfs(&d, true, &AtomicBool::new(false)).files;
    let ev = run_files(files, "notice", opts());

    let mut found: Vec<(String, u32, u32, Vec<u32>)> = ev
        .iter()
        .filter_map(|e| match e {
            FilesEvent::File { name, hits, pages, items, .. } => Some((name.clone(), *hits, *pages, items.iter().map(|i| i.page).collect())),
            _ => None,
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            ("a.pdf".to_string(), 2, 1, vec![0, 0]),
            ("c.pdf".to_string(), 1, 1, vec![0]),
            ("d.pdf".to_string(), 3, 1, vec![0, 0, 0]),
        ]
    );
    let Some(FilesEvent::Done { files, matched, hits, skipped, cancelled, capped, .. }) = ev.last() else { panic!("no Done") };
    assert_eq!((*files, *matched, *hits, *skipped, *cancelled, *capped), (5, 3, 6, 1, false, false));
    // Progress ends at the total and never goes backwards.
    let scanned: Vec<u32> = ev.iter().filter_map(|e| if let FilesEvent::Progress { scanned, .. } = e { Some(*scanned) } else { None }).collect();
    assert_eq!(scanned.len(), 5);
    assert_eq!(*scanned.iter().max().unwrap(), 5);
}

#[test]
fn the_options_reach_the_files() {
    let d = tree("opts");
    let files = list_pdfs(&d, true, &AtomicBool::new(false)).files;
    let ev = run_files(files, "Notice", SearchOpts { case_sensitive: true, ..opts() });
    let Some(FilesEvent::Done { matched, hits, .. }) = ev.last() else { panic!("no Done") };
    assert_eq!((*matched, *hits), (1, 1));
}

#[test]
fn a_file_lists_a_bounded_number_of_hits_and_counts_the_rest() {
    let d = dir("cap");
    let line = "needle ".repeat(10);
    let lines: Vec<&str> = (0..15).map(|_| line.as_str()).collect();
    let p = write(&d, "many.pdf", plain(&[lines]));
    let q = Query::new("needle", opts()).unwrap();
    let r = search_file(&p, &q, &AtomicBool::new(false), usize::MAX).unwrap();
    assert_eq!(r.hits, 150);
    assert_eq!(r.items.len(), PER_FILE);
    assert!(r.truncated);
    // The search-wide budget can be smaller than the per-file one.
    let r = search_file(&p, &q, &AtomicBool::new(false), 7).unwrap();
    assert_eq!((r.hits, r.items.len()), (150, 7));
}

#[test]
fn an_encrypted_or_damaged_file_is_skipped_not_fatal() {
    let d = dir("skip");
    let p = write(&d, "x.pdf", b"%PDF-1.4\nnot really".to_vec());
    let q = Query::new("a", opts()).unwrap();
    assert_eq!(search_file(&p, &q, &AtomicBool::new(false), 10).unwrap_err(), Skip::Unreadable);
    assert_eq!(search_file(&d.join("missing.pdf"), &q, &AtomicBool::new(false), 10).unwrap_err(), Skip::Unreadable);
}

#[test]
fn cancel_stops_a_folder_search_between_pages() {
    let d = dir("cancel");
    let page: Vec<&str> = vec!["needle here"];
    let pages: Vec<Vec<&str>> = (0..30).map(|_| page.clone()).collect();
    let bytes = plain(&pages);
    let mut files = Vec::new();
    for i in 0..60 {
        files.push(write(&d, &format!("f{i:02}.pdf"), bytes.clone()));
    }
    let searches = Arc::new(FolderSearches::default());
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let id = searches.start(WIN, files, false, "needle", opts(), Arc::new(move |e| {
        let _ = tx.lock().unwrap().send(e);
    }));
    // Wait for the first file, then cancel.
    loop {
        if let FilesEvent::Progress { .. } = rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            break;
        }
    }
    searches.cancel(WIN, id);
    loop {
        match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            FilesEvent::Done { cancelled, matched, files, .. } => {
                assert!(cancelled);
                assert!(matched < files, "stopped before the end: {matched} of {files}");
                break;
            }
            _ => {}
        }
    }
}

#[test]
fn what_a_form_field_holds_is_found_after_save_and_reopen() {
    use crate::forms::{add_one, set_value_one, FieldKind, FieldValue, NewField};
    use crate::engine::edit::EditSource;
    let d = dir("roundtrip");
    let p = write(&d, "a.pdf", plain(&[vec!["A page with nothing to find."]]));
    let eng = Engine::new(2, 64 << 20);
    let id: DocId = eng.open(&p.to_string_lossy()).unwrap().doc;
    let what = NewField { kind: FieldKind::Text, rect: [72.0, 100.0, 272.0, 120.0], name: Some("Clause".into()), group: None, export: None, caption: None, options: None };
    let (_, xref) = add_one(&eng, id, 0, &what, EditSource::User).unwrap();
    set_value_one(&eng, id, 0, xref, &FieldValue::Text("written NOTICE of thirty days".into()), EditSource::User).unwrap();
    eng.save(id, None).unwrap();

    let again = Engine::new(2, 64 << 20);
    let id2 = again.open(&p.to_string_lossy()).unwrap().doc;
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    again
        .search_with(id2, "notice".into(), SearchOpts { fields: true, ..opts() }, Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }))
        .unwrap();
    let mut got = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            SearchEvent::Hits { hits, .. } => got.extend(hits),
            SearchEvent::Progress { .. } => {}
            SearchEvent::Done { .. } => break,
        }
    }
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!((got[0].source, got[0].text.as_str(), got[0].before.as_str()), (Source::Field, "NOTICE", "written "));
}

// ---- matching: what the reviewer asked for -----------------------------------------------------------------

fn text_hits(text: &str, query: &str, o: SearchOpts) -> usize {
    PageChars::of_text(text).find(&Query::new(query, o).expect("a query"), Source::Text).len()
}

#[test]
fn a_soft_hyphen_is_never_part_of_the_text() {
    // Searching for the word finds it however the writer broke it, and the query's own soft hyphen is ignored.
    assert_eq!(text_hits("co\u{ad}operate fully", "cooperate", opts()), 1);
    assert_eq!(text_hits("cooperate fully", "co\u{ad}operate", opts()), 1);
    assert_eq!(text_hits("caf\u{e9}\u{ad}teria", "cafeteria",SearchOpts { ignore_accents: true, ..opts() }), 1);
}

#[test]
fn composed_and_decomposed_letters_are_the_same_text() {
    let composed = "un caf\u{e9} noir";
    let decomposed = "un cafe\u{301} noir";
    for text in [composed, decomposed] {
        assert_eq!(text_hits(text, "caf\u{e9}", opts()), 1, "{text:?} for the composed query");
        assert_eq!(text_hits(text, "cafe\u{301}", opts()), 1, "{text:?} for the decomposed query");
        // Without ignoring accents "cafe" is not "café", composed or not; with it, it is.
        assert_eq!(text_hits(text, "cafe", opts()), 0, "{text:?}");
        assert_eq!(text_hits(text, "cafe", SearchOpts { ignore_accents: true, ..opts() }), 1, "{text:?}");
        assert_eq!(text_hits(text, "cafe", SearchOpts { whole_words: true, ..opts() }), 0, "{text:?}");
    }
}

#[test]
fn case_folding_is_full() {
    assert_eq!(text_hits("Stra\u{df}e", "STRASSE", opts()), 1);
    assert_eq!(text_hits("STRASSE", "stra\u{df}e", opts()), 1);
    // A final sigma and a medial one are the same letter in capitals and lower case.
    assert_eq!(text_hits("\u{3bf}\u{3b4}\u{3cc}\u{3c2}", "\u{39f}\u{394}\u{38c}\u{3a3}", opts()), 1);
    assert_eq!(text_hits("\u{3a3}\u{3bf}\u{3c6}\u{3cc}\u{3c2}", "\u{3c3}\u{3bf}\u{3c6}\u{3cc}\u{3c3}", opts()), 1);
    // Case counts when asked for.
    assert_eq!(text_hits("Stra\u{df}e", "STRASSE", SearchOpts { case_sensitive: true, ..opts() }), 0);
}

#[test]
fn a_hyphen_that_ends_a_line_is_found_both_ways() {
    let b = plain(&[vec!["it is a well-", "known fact and we co-", "operate fully"]]);
    // Kept: the author's hyphen. Dropped: the writer's.
    assert_eq!(count(&b, "well-known", opts()), 1);
    assert_eq!(count(&b, "wellknown", opts()), 1);
    assert_eq!(count(&b, "co-operate", opts()), 1);
    assert_eq!(count(&b, "cooperate", opts()), 1);
    // The same place is one result, not two.
    assert_eq!(count(&b, "known", opts()), 1);
    assert_eq!(count(&b, "well", opts()), 1);
}

#[test]
fn text_comments_and_fields_share_one_page_budget() {
    // 1990 hits in the text, then a sticky note and a field of 30 hits each: the page lists 2000 in all.
    let line = "a ".repeat(40);
    let lines: Vec<&str> = (0..50).map(|_| line.as_str()).collect();
    let mut stretch: Vec<&str> = lines;
    stretch.truncate(49); // 1960 hits
    let ten = "a ".repeat(30);
    let note = format!("<< /Type /Annot /Subtype /Text /Rect [300 600 320 620] /Contents ({ten}) /F 4 >>");
    let field = format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (F) /V ({ten}) /Rect [72 400 300 420] /F 4 /DA (/Helv 12 Tf 0 g) >>");
    let extra = Extra {
        catalog: "/AcroForm << /Fields [7 0 R] /DA (/Helv 0 Tf 0 g) >>".into(),
        annots: "6 0 R 7 0 R".into(),
        objs: vec![note, field],
    };
    let b = doc_with(&[stretch], extra);
    let all = SearchOpts { comments: true, fields: true, ..opts() };
    let got = hits(&b, "a", all);
    assert_eq!(got.len(), 2000, "text + comments + fields on one page");
    assert_eq!(got.iter().filter(|h| h.1.source == Source::Text).count(), 1960);
    assert_eq!(got.iter().filter(|h| h.1.source == Source::Comment).count(), 30);
    assert_eq!(got.iter().filter(|h| h.1.source == Source::Field).count(), 10, "the field gets what is left");
}

fn engine_search(eng: &Arc<Engine>, id: DocId, query: &str, o: SearchOpts) -> (u32, u32, bool) {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    eng.search_with(id, query.into(), o, Arc::new(move |e| {
        let _ = tx.lock().unwrap().send(e);
    }))
    .unwrap();
    let mut listed = 0u32;
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).expect("a Done event") {
            SearchEvent::Hits { hits, .. } => listed += hits.len() as u32,
            SearchEvent::Progress { .. } => {}
            SearchEvent::Done { hits, capped, .. } => return (listed, hits, capped),
        }
    }
}

#[test]
fn a_document_lists_at_most_the_document_cap_and_says_so() {
    let d = dir("doccap");
    // 30 pages of 400 hits: 12 000 in all, the search stops at 5000.
    let line = "a ".repeat(40);
    let page: Vec<&str> = (0..10).map(|_| line.as_str()).collect();
    let pages: Vec<Vec<&str>> = (0..30).map(|_| page.clone()).collect();
    let p = write(&d, "big.pdf", plain(&pages));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    let (listed, hits, capped) = engine_search(&eng, id, "a", opts());
    assert_eq!((listed, hits, capped), (DOC_CAP as u32, DOC_CAP as u32, true));
    // A search that fits says nothing of the kind.
    let (listed, hits, capped) = engine_search(&eng, id, "a a a a a", opts());
    assert!(!capped && listed == hits && hits > 0, "{listed} {hits} {capped}");
}

// ---- search across files: the review's findings -----------------------------------------------------------

#[test]
fn an_in_document_search_finishes_while_a_long_folder_search_runs() {
    let d = dir("starve");
    // 150 files of 150 pages: seconds of work for the folder search, even on a fast machine.
    let pages: Vec<Vec<&str>> = (0..150).map(|i| if i % 50 == 0 { vec!["the needle is here"] } else { vec!["nothing at all on this page"] }).collect();
    let bytes = plain(&pages);
    let mut files = Vec::new();
    for i in 0..150 {
        files.push(write(&d, &format!("f{i:02}.pdf"), bytes.clone()));
    }
    let open = write(&d, "open.pdf", plain(&[vec!["a needle in the open document"], vec!["another needle"]]));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&open.to_string_lossy()).unwrap().doc;

    let searches = Arc::new(FolderSearches::default());
    let folder_done = Arc::new(AtomicBool::new(false));
    let fd = folder_done.clone();
    let folder = searches.start(WIN, files, false, "needle", opts(), Arc::new(move |e| {
        if matches!(e, FilesEvent::Done { .. }) {
            fd.store(true, Ordering::SeqCst);
        }
    }));
    // Let the folder search get going, then search the open document: it must not wait for the folder.
    std::thread::sleep(Duration::from_millis(200));
    let t0 = std::time::Instant::now();
    let (listed, hits, _) = engine_search(&eng, id, "needle", opts());
    let took = t0.elapsed();
    assert_eq!((listed, hits), (2, 2));
    assert!(!folder_done.load(Ordering::SeqCst), "the folder search was already over ({took:?}): make it longer");
    assert!(took < Duration::from_secs(5), "took {took:?}");
    searches.cancel(WIN, folder);
}

#[test]
fn listing_stops_when_cancelled_and_leaves_out_cloud_only_files() {
    let d = tree("listcancel");
    let stop = AtomicBool::new(true);
    let l = list_pdfs(&d, true, &stop);
    assert!(l.cancelled && l.files.is_empty(), "{l:?}");

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
        let cloud = d.join("cloud.pdf");
        std::fs::write(&cloud, plain(&[vec!["notice in the cloud"]])).unwrap();
        let wide: Vec<u16> = cloud.as_os_str().encode_wide().chain(Some(0)).collect();
        // FILE_ATTRIBUTE_OFFLINE: what a placeholder that is not on this PC looks like.
        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), 0x1000) }, 0, "could not mark the file");
        let go = AtomicBool::new(false);
        let l = list_pdfs(&d, true, &go);
        assert_eq!(l.cloud_only, 1);
        assert!(!l.files.iter().any(|p| p.ends_with("cloud.pdf")));
        let picked = list_picked(vec![cloud.clone(), d.join("a.pdf")]);
        assert_eq!((picked.cloud_only, picked.files.len()), (1, 1));
    }
}

#[test]
fn the_listing_room_is_shared_by_the_workers() {
    let d = dir("room");
    // Each file has 120 hits (100 listed); 60 files would list 6000 without the shared cap.
    let line = "needle ".repeat(10);
    let page: Vec<&str> = (0..12).map(|_| line.as_str()).collect();
    let bytes = plain(&[page]);
    let mut files = Vec::new();
    for i in 0..60 {
        files.push(write(&d, &format!("f{i:02}.pdf"), bytes.clone()));
    }
    let ev = run_files(files, "needle", opts());
    let listed: usize = ev.iter().map(|e| if let FilesEvent::File { items, .. } = e { items.len() } else { 0 }).sum();
    assert_eq!(listed, TOTAL, "listed exactly the cap");
    let Some(FilesEvent::Done { matched, hits, .. }) = ev.last() else { panic!("no Done") };
    assert_eq!((*matched, *hits), (60, 60 * 120), "every hit is still counted");
}

#[test]
fn a_result_is_opened_through_its_search_and_only_the_latest_search_answers() {
    let d = tree("lazy");
    let files = list_pdfs(&d, true, &AtomicBool::new(false)).files;
    let searches = Arc::new(FolderSearches::default());
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let first = searches.start(WIN, files.clone(), false, "notice", opts(), Arc::new(move |e| {
        let _ = tx.lock().unwrap().send(e);
    }));
    let mut found = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            FilesEvent::File { index, path, .. } => found.push((index, path)),
            FilesEvent::Done { .. } => break,
            _ => {}
        }
    }
    assert!(!found.is_empty());
    for (index, path) in &found {
        assert_eq!(searches.path_of(WIN, first, *index).unwrap().to_string_lossy(), *path);
    }
    assert!(searches.path_of(WIN, first, 999).is_none());
    // A newer search forgets the older one's files.
    let second = searches.start(WIN, files, false, "zzz-nothing", opts(), Arc::new(|_| {}));
    assert!(searches.path_of(WIN, first, found[0].0).is_none());
    assert!(searches.path_of(WIN, second, found[0].0).is_none());
}

#[test]
fn a_result_is_only_reachable_from_the_window_that_searched() {
    let d = tree("window");
    let files = list_pdfs(&d, true, &AtomicBool::new(false)).files;
    let searches = Arc::new(FolderSearches::default());
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let id = searches.start(WIN, files, false, "notice", opts(), Arc::new(move |e| {
        let _ = tx.lock().unwrap().send(e);
    }));
    let mut index = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            FilesEvent::File { index: i, .. } => index = Some(i),
            FilesEvent::Done { .. } => break,
            _ => {}
        }
    }
    let index = index.expect("a file with hits");
    assert!(searches.path_of(WIN, id, index).is_some());
    assert!(searches.path_of("other", id, index).is_none(), "another window cannot open it");
    // ...nor cancel it.
    let (second, flag) = searches.begin(WIN);
    let (_third, other_flag) = searches.begin("other");
    searches.cancel("other", second);
    assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!other_flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn starting_a_search_ends_the_windows_previous_one_and_its_late_files_are_not_listed() {
    let d = tree("supersede");
    let files = list_pdfs(&d, true, &AtomicBool::new(false)).files;
    let searches = Arc::new(FolderSearches::default());
    let (first, first_flag) = searches.begin(WIN);
    let (_elsewhere, elsewhere_flag) = searches.begin("other");
    assert!(!first_flag.load(std::sync::atomic::Ordering::SeqCst));
    let (second, second_flag) = searches.begin(WIN);
    assert!(first_flag.load(std::sync::atomic::Ordering::SeqCst), "the window's previous search is cancelled");
    assert!(!second_flag.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!elsewhere_flag.load(std::sync::atomic::Ordering::SeqCst), "another window's search keeps going");
    // The old search finishing a file after the new one began must not write into the new one's list.
    searches.record(WIN, first, 0, files[0].clone());
    assert!(searches.path_of(WIN, second, 0).is_none());
    assert!(searches.path_of(WIN, first, 0).is_none());
    searches.record(WIN, second, 0, files[0].clone());
    assert_eq!(searches.path_of(WIN, second, 0), Some(files[0].clone()));
}
