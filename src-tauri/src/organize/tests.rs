//! Organize-plus through the edit lane: every command is one undo step, kept by Save, and read back
//! from the saved file with a fresh MuPDF.

use super::attach::{AttachKind, NewFile};
use super::combine::{CombineOptions, FileState, SourceSpec};
use super::labels::{LabelSpec, LabelStyle};
use super::trans::{Motion, TransChange, TransStyle, Transition};
use super::*;
use crate::engine::edit::EditOp;
use crate::engine::fixture::{self, Fixture};
use std::path::PathBuf;
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("organize-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &std::path::Path, name: &str, pages: usize, chapter_every: usize) -> String {
    let p = d.join(name);
    std::fs::write(&p, fixture::build(&Fixture { pages, chapter_every, lines: 4, ..Default::default() })).unwrap();
    p.to_string_lossy().into_owned()
}

fn open(path: &str) -> (Arc<Engine>, DocId) {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(path).unwrap().doc;
    (eng, id)
}

/// Saves and opens the file again with a new engine.
fn save_reopen(eng: &Engine, id: DocId) -> (Arc<Engine>, DocId, String) {
    let s = eng.save(id, None).unwrap();
    let (e, i) = open(&s.path);
    (e, i, s.path)
}

fn user() -> EditSource {
    EditSource::User
}

fn spec(style: LabelStyle, prefix: &str, start: u32) -> LabelSpec {
    LabelSpec { style, prefix: prefix.into(), start, extend: false }
}

fn labels(eng: &Engine, id: DocId) -> Vec<String> {
    labels_get(eng, id).unwrap().labels
}

#[test]
fn page_labels_are_one_undo_step_and_survive_save() {
    let d = dir("labels");
    let (eng, id) = open(&write(&d, "a.pdf", 6, 0));
    assert!(labels(&eng, id).is_empty());
    let s = labels_apply(&eng, id, 0, 2, &spec(LabelStyle::LowerRoman, "", 1), user()).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Page labels on pp. 1-3"));
    assert_eq!(labels(&eng, id), ["i", "ii", "iii", "4", "5", "6"]);
    // asking for what is already there adds no step
    let rev = eng.edit_state(id).unwrap().rev;
    labels_apply(&eng, id, 0, 2, &spec(LabelStyle::LowerRoman, "", 1), user()).unwrap();
    assert_eq!(eng.edit_state(id).unwrap().rev, rev);

    let (e2, i2, _) = save_reopen(&eng, id);
    assert_eq!(labels(&e2, i2), ["i", "ii", "iii", "4", "5", "6"]);

    eng.undo(id).unwrap();
    assert!(labels(&eng, id).is_empty(), "one undo takes the whole dialog back");
    eng.redo(id).unwrap();
    assert_eq!(labels(&eng, id)[0], "i");

    // a prefix section over the last pages, numbered from 1
    labels_apply(&eng, id, 3, 5, &spec(LabelStyle::Decimal, "A-", 1), user()).unwrap();
    assert_eq!(labels(&eng, id), ["i", "ii", "iii", "A-1", "A-2", "A-3"]);
    labels_clear(&eng, id, user()).unwrap();
    assert!(labels(&eng, id).is_empty());
    eng.undo(id).unwrap();
    assert_eq!(labels(&eng, id)[3], "A-1");
}

#[test]
fn page_labels_follow_the_pages_when_pages_are_deleted_or_inserted() {
    let d = dir("labels-edit");
    let (eng, id) = open(&write(&d, "a.pdf", 6, 0));
    labels_apply(&eng, id, 0, 2, &spec(LabelStyle::LowerRoman, "", 1), user()).unwrap();
    assert_eq!(labels(&eng, id), ["i", "ii", "iii", "4", "5", "6"]);
    // a front page goes: the roman section closes up, the body keeps its numbers
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    assert_eq!(labels(&eng, id), ["i", "ii", "4", "5", "6"]);
    // a page put inside the roman section joins it
    eng.apply(id, EditOp::InsertBlank { at: 1, like: 0 }).unwrap();
    assert_eq!(labels(&eng, id), ["i", "ii", "iii", "4", "5", "6"]);
    // deleting the whole roman section leaves the body as it was
    eng.apply(id, EditOp::Delete { pages: vec![0, 1, 2] }).unwrap();
    assert_eq!(labels(&eng, id), ["4", "5", "6"]);
    // and moving pages changes nothing about the sections (they stay where they are)
    let n = labels(&eng, id).len();
    eng.apply(id, EditOp::Move { pages: vec![0], to: n as u32 }).unwrap();
    assert_eq!(labels(&eng, id), ["4", "5", "6"]);
}

#[test]
fn extend_numbering_and_bad_ranges() {
    let d = dir("labels-extend");
    let (eng, id) = open(&write(&d, "a.pdf", 5, 0));
    labels_apply(&eng, id, 0, 1, &spec(LabelStyle::UpperRoman, "", 1), user()).unwrap();
    let ext = LabelSpec { style: LabelStyle::Decimal, prefix: String::new(), start: 1, extend: true };
    labels_apply(&eng, id, 2, 2, &ext, user()).unwrap();
    assert_eq!(labels(&eng, id), ["I", "II", "III", "4", "5"]);
    assert!(labels_apply(&eng, id, 3, 9, &spec(LabelStyle::Decimal, "", 1), user()).is_err());
    assert!(labels_apply(&eng, id, 0, 0, &ext, user()).is_err(), "nothing before the first page to extend");
    assert!(labels_apply(&eng, id, 0, 0, &spec(LabelStyle::Decimal, "", 0), user()).is_err());
}

fn wipe() -> Transition {
    Transition { style: TransStyle::Wipe, duration: 0.5, dimension: None, motion: Some(Motion::I), direction: Some(90) }
}

#[test]
fn transitions_and_auto_advance_round_trip() {
    let d = dir("trans");
    let (eng, id) = open(&write(&d, "a.pdf", 3, 0));
    let change = TransChange { transition: Some(wipe()), auto_advance: Some(4.0) };
    let s = trans_set(&eng, id, &[0, 1], &change, user()).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Page transition on pp. 1-2"));
    let got = trans_get(&eng, id).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].auto_advance, Some(4.0));
    let t = got[1].transition.unwrap();
    assert_eq!((t.style, t.duration, t.direction), (TransStyle::Wipe, 0.5, Some(90)));
    // /M is not a Wipe key: it is not written, and reads back as none
    assert_eq!(t.motion, None);
    assert_eq!(got[2], trans::PageTrans::default());

    let (e2, i2, _) = save_reopen(&eng, id);
    assert_eq!(trans_get(&e2, i2).unwrap()[1], got[1]);

    // the same again is not a new step
    let rev = eng.edit_state(id).unwrap().rev;
    trans_set(&eng, id, &[0, 1], &change, user()).unwrap();
    assert_eq!(eng.edit_state(id).unwrap().rev, rev);

    eng.undo(id).unwrap();
    assert!(trans_get(&eng, id).unwrap().iter().all(|p| *p == trans::PageTrans::default()));
    eng.redo(id).unwrap();
    let off = TransChange { transition: None, auto_advance: None };
    let s = trans_set(&eng, id, &[0, 1], &off, user()).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Remove page transition on pp. 1-2"));
    assert!(trans_get(&eng, id).unwrap().iter().all(|p| *p == trans::PageTrans::default()));
    assert!(trans_set(&eng, id, &[7], &change, user()).is_err());
    let bad = TransChange { transition: Some(Transition { direction: Some(45), ..wipe() }), auto_advance: None };
    assert!(trans_set(&eng, id, &[0], &bad, user()).is_err());
}

/// Bytes nothing compresses: a leak of them into a file shows in its size.
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn file(name: &str, data: Vec<u8>, desc: &str) -> NewFile {
    NewFile { name: name.into(), data, description: desc.into() }
}

#[test]
fn attachments_add_save_describe_delete_and_leave_nothing_behind() {
    let d = dir("attach");
    let path = write(&d, "a.pdf", 2, 0);
    let original = std::fs::metadata(&path).unwrap().len();
    let (eng, id) = open(&path);
    let secret = noise(64 * 1024, 7);
    let s = attach_add(&eng, id, vec![file("notes.txt", b"hello".to_vec(), "my notes"), file("blob.bin", secret.clone(), "")], user()).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Attach 2 files"), "several files are one step");
    let list = attach_list(&eng, id).unwrap();
    assert_eq!(list.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["blob.bin", "notes.txt"], "listed in key order");
    let notes = list.iter().find(|a| a.name == "notes.txt").unwrap();
    assert_eq!((notes.description.as_str(), notes.size, notes.kind), ("my notes", 5, AttachKind::File));
    assert_eq!(notes.mime.as_deref(), Some("text/plain"));

    // a second file of the same name does not replace the first
    attach_add(&eng, id, vec![file("notes.txt", b"second".to_vec(), "")], user()).unwrap();
    let list = attach_list(&eng, id).unwrap();
    assert_eq!(list.len(), 3);
    let (name, bytes) = attach_data(&eng, id, "n:notes (2).txt").unwrap();
    assert_eq!((name.as_str(), bytes.as_slice()), ("notes (2).txt", b"second".as_slice()));
    assert_eq!(attach_data(&eng, id, "n:notes.txt").unwrap().1, b"hello");

    // saved out and read back after a save and a reopen
    let (e2, i2, saved) = save_reopen(&eng, id);
    assert_eq!(attach_data(&e2, i2, "n:blob.bin").unwrap().1, secret);
    assert!(std::fs::metadata(&saved).unwrap().len() > original + 60_000);
    drop(e2); // Windows: the next save replaces the file this engine has open

    attach_describe(&eng, id, "n:blob.bin", "random data", user()).unwrap();
    assert_eq!(attach_list(&eng, id).unwrap()[0].description, "random data");
    eng.undo(id).unwrap();
    assert_eq!(attach_list(&eng, id).unwrap()[0].description, "");

    // delete: one undo step, and after a save the bytes are not in the file
    let s = attach_remove(&eng, id, &["n:blob.bin".to_string()], user()).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Delete attachment blob.bin"));
    assert_eq!(attach_list(&eng, id).unwrap().len(), 2);
    let (e3, i3, saved) = save_reopen(&eng, id);
    assert_eq!(attach_list(&e3, i3).unwrap().len(), 2);
    let size = std::fs::metadata(&saved).unwrap().len();
    assert!(size < original + 20_000, "the deleted file's bytes are still in the saved file ({size} bytes)");
    let on_disk = std::fs::read(&saved).unwrap();
    assert!(!on_disk.windows(64).any(|w| w == &secret[1000..1064]), "deleted bytes found in the saved file");

    eng.undo(id).unwrap();
    assert_eq!(attach_list(&eng, id).unwrap().len(), 3, "undo brings it back");
}

#[test]
fn attachments_of_a_pages_file_attachment_annotation_are_listed_and_deleted() {
    use mupdf::pdf::{EmbeddedFileOptions, PdfDocument, PdfObject};
    let d = dir("attach-annot");
    let path = write(&d, "a.pdf", 2, 0);
    {
        // build the annotation with MuPDF itself, as another program would have
        let mut doc = PdfDocument::open(&path).unwrap();
        // the same file spec the document-level list has: a pin on page 2 points at it
        let made = doc.add_embedded_file("pin.txt", b"pinned", EmbeddedFileOptions::new("pin.txt")).unwrap();
        let fs = doc.new_indirect(made.xref, 0).unwrap();
        let mut a = doc.new_dict().unwrap();
        a.dict_put("Type", PdfObject::new_name("Annot").unwrap()).unwrap();
        a.dict_put("Subtype", PdfObject::new_name("FileAttachment").unwrap()).unwrap();
        let mut r = doc.new_array().unwrap();
        for v in [100.0, 100.0, 120.0, 120.0] {
            r.array_push(PdfObject::new_real(v).unwrap()).unwrap();
        }
        a.dict_put("Rect", r).unwrap();
        a.dict_put("FS", fs).unwrap();
        a.dict_put("Contents", PdfObject::new_string("see this").unwrap()).unwrap();
        let a = doc.add_object(&a).unwrap();
        let mut list = doc.new_array().unwrap();
        list.array_push(a).unwrap();
        let mut page = doc.find_page(1).unwrap();
        page.dict_put("Annots", list).unwrap();
        let out = d.join("annot.pdf");
        doc.save(out.to_str().unwrap()).unwrap();
    }
    let (eng, id) = open(d.join("annot.pdf").to_str().unwrap());
    let list = attach_list(&eng, id).unwrap();
    assert_eq!(list.len(), 2, "{list:?}");
    assert_eq!(list[0].kind, AttachKind::File);
    let a = &list[1];
    assert_eq!((a.kind, a.page, a.name.as_str(), a.description.as_str()), (AttachKind::Annotation, Some(1), "pin.txt", "see this"));
    assert_eq!(attach_data(&eng, id, &a.id).unwrap().1, b"pinned");
    // the pin goes; the document-level file it pointed at stays
    attach_remove(&eng, id, &[a.id.clone()], user()).unwrap();
    let left = attach_list(&eng, id).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].kind, AttachKind::File);
    let (e2, i2, _) = save_reopen(&eng, id);
    assert_eq!(attach_list(&e2, i2).unwrap().len(), 1);
}

// ---- combine ------------------------------------------------------------------------------------

fn src(path: &str, range: &str) -> SourceSpec {
    SourceSpec { path: path.into(), range: range.into(), title: None }
}

fn all_options() -> CombineOptions {
    CombineOptions { bookmarks: true, keep_outlines: true, contents_page: true }
}

fn count(items: &[crate::engine::OutlineItem]) -> u32 {
    items.iter().map(|i| 1 + count(&i.children)).sum()
}

#[test]
fn combine_writes_what_the_plan_said() {
    let d = dir("combine");
    let a = write(&d, "a.pdf", 3, 1);
    let b = write(&d, "b.pdf", 2, 0);
    let c = write(&d, "c.pdf", 4, 0);
    let specs = [src(&a, "1-2"), src(&b, ""), src(&c, "4-3")];
    let (eng, _) = open(&b);
    let plan = combine::inspect(&specs, all_options()).unwrap();
    assert_eq!(plan.summary, "Combines 3 files, 7 pages, 7 bookmarks, a contents page");
    assert_eq!((plan.pages, plan.contents_pages, plan.can_run), (7, 1, true));

    let out = d.join("out.pdf").to_string_lossy().into_owned();
    let report = combine::run(&eng, &specs, all_options(), &out).unwrap();
    assert_eq!(report.plan, plan, "the run is what was predicted");

    let (e2, i2) = open(&out);
    assert_eq!(e2.edit_state(i2).unwrap().page_count, plan.pages);
    let outline = e2.outline(i2).unwrap();
    assert_eq!(count(&outline), plan.bookmarks);
    let tops: Vec<(&str, Option<u32>)> = outline.iter().map(|o| (o.title.as_str(), o.page)).collect();
    assert_eq!(tops, [("Contents", Some(0)), ("a", Some(1)), ("b", Some(3)), ("c", Some(5))]);
    // a's own outline sits under it; the chapter whose page was left out keeps its title, no jump
    let kids: Vec<Option<u32>> = outline[1].children.iter().map(|o| o.page).collect();
    assert_eq!(kids, [Some(1), Some(2), None]);
    // the contents page links to the files' first pages
    let doc = mupdf::pdf::PdfDocument::open(&out).unwrap();
    let toc = doc.load_pdf_page(0).unwrap();
    assert_eq!(toc.links().unwrap().count(), 3, "one link per file");
    assert!(e2.page_png(i2, 0, 0, 200).is_ok(), "the contents page renders");
    assert!(labels(&e2, i2).is_empty(), "no page labels leak into the combined file");
    let text = toc.to_text_page(mupdf::TextPageFlags::empty()).unwrap().to_text().unwrap();
    assert!(text.contains("Contents") && text.contains("a") && text.contains('6'), "{text}");
}

#[test]
fn combine_options_off_give_a_plain_file() {
    let d = dir("combine-plain");
    let a = write(&d, "a.pdf", 2, 1);
    let b = write(&d, "b.pdf", 2, 0);
    let (eng, _) = open(&a);
    let specs = [src(&a, ""), src(&b, "")];
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false };
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!((plan.pages, plan.bookmarks, plan.contents_pages), (4, 0, 0));
    assert_eq!(plan.summary, "Combines 2 files, 4 pages");
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &specs, opt, &out).unwrap();
    let (e2, i2) = open(&out);
    assert!(e2.outline(i2).unwrap().is_empty());
    // outlines kept without a bookmark per file: the file's own items, flat
    let opt = CombineOptions { bookmarks: false, keep_outlines: true, contents_page: false };
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!(plan.bookmarks, 2);
    let out2 = d.join("out2.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &specs, opt, &out2).unwrap();
    let (e3, i3) = open(&out2);
    assert_eq!(count(&e3.outline(i3).unwrap()), 2);
}

#[test]
fn combine_leaves_out_what_cannot_be_taken_and_refuses_bad_ranges() {
    use mupdf::pdf::{Encryption, PdfDocument, PdfWriteOptions};
    let d = dir("combine-skip");
    let a = write(&d, "a.pdf", 2, 0);
    let locked = d.join("locked.pdf").to_string_lossy().into_owned();
    {
        let src_doc = PdfDocument::from_bytes(&fixture::build(&Fixture { pages: 2, ..Default::default() })).unwrap();
        let mut o = PdfWriteOptions::default();
        o.set_encryption(Encryption::Aes256).set_user_password("secret").set_owner_password("owner");
        src_doc.save_with_options(&locked, o).unwrap();
    }
    let junk = d.join("junk.pdf");
    std::fs::write(&junk, b"not a pdf at all").unwrap();
    let junk = junk.to_string_lossy().into_owned();
    let (eng, _) = open(&a);
    let opt = CombineOptions { bookmarks: true, keep_outlines: false, contents_page: false };
    let specs = [src(&a, ""), src(&locked, ""), src(&junk, "")];
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!(plan.files[1].state, FileState::Encrypted);
    assert!(plan.files[1].message.as_deref().unwrap().contains("locked.pdf is password protected"));
    assert_eq!(plan.files[2].state, FileState::Unreadable);
    assert_eq!((plan.left_out, plan.pages, plan.can_run), (2, 2, true));
    assert_eq!(plan.summary, "Combines 1 file, 2 pages, 1 bookmark. 2 files are left out");
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    let r = combine::run(&eng, &specs, opt, &out).unwrap();
    assert_eq!(r.plan.pages, 2);

    // nothing left to combine
    let only = [src(&locked, "")];
    assert!(!combine::inspect(&only, opt).unwrap().can_run);
    assert!(combine::run(&eng, &only, opt, &out).is_err());

    // a range outside the file stops the run and says which file and page
    let bad = [src(&a, "1-9")];
    let plan = combine::inspect(&bad, opt).unwrap();
    assert!(!plan.can_run);
    let e = combine::run(&eng, &bad, opt, &out).unwrap_err().to_string();
    assert!(e.contains("Page 9 is not in a.pdf"), "{e}");

    // an open document's own file is never the output
    assert!(eng.check_not_open(&a, None).is_err());
    assert!(combine::run(&eng, &[src(&a, "")], opt, &a).is_err());
}

#[test]
fn combine_keeps_form_fields_and_renames_the_ones_that_clash() {
    use crate::forms::{FieldKind, NewField, add_one, list_all};
    let d = dir("combine-forms");
    let mut paths = Vec::new();
    for n in ["f1.pdf", "f2.pdf"] {
        let p = write(&d, n, 1, 0);
        let (eng, id) = open(&p);
        let what = NewField { kind: FieldKind::Text, rect: [72.0, 100.0, 272.0, 120.0], name: Some("Name".into()), group: None, export: None, caption: None, options: None };
        add_one(&eng, id, 0, &what, user()).unwrap();
        eng.save(id, None).unwrap();
        paths.push(p);
    }
    let (eng, _) = open(&paths[0]);
    let specs = [src(&paths[0], ""), src(&paths[1], "")];
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false };
    assert!(combine::inspect(&specs, opt).unwrap().files.iter().all(|f| f.has_fields));
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &specs, opt, &out).unwrap();
    let (e2, i2) = open(&out);
    let mut names: Vec<(u32, String)> = list_all(&e2, i2).unwrap().fields.into_iter().map(|f| (f.page, f.name)).collect();
    names.sort();
    assert_eq!(names, [(0, "Name".to_string()), (1, "Name (2)".to_string())]);
}

#[test]
fn combine_and_insert_keep_page_transitions() {
    let d = dir("combine-trans");
    let a = write(&d, "a.pdf", 2, 0);
    let (eng, id) = open(&a);
    let change = TransChange { transition: Some(wipe()), auto_advance: Some(3.0) };
    trans_set(&eng, id, &[1], &change, user()).unwrap();
    eng.save(id, None).unwrap();
    let b = write(&d, "b.pdf", 1, 0);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false };
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &[src(&b, ""), src(&a, "")], opt, &out).unwrap();
    let (e2, i2) = open(&out);
    let t = trans_get(&e2, i2).unwrap();
    assert_eq!(t.len(), 3);
    assert_eq!(t[2].auto_advance, Some(3.0));
    assert_eq!(t[2].transition.unwrap().style, TransStyle::Wipe);
    assert_eq!(t[0], trans::PageTrans::default());
}

#[test]
fn a_file_without_pages_is_left_out_not_a_panic() {
    let d = dir("combine-empty");
    let a = write(&d, "a.pdf", 2, 0);
    let empty = d.join("empty.pdf").to_string_lossy().into_owned();
    mupdf::pdf::PdfDocument::new().save(&empty).unwrap();
    let (eng, _) = open(&a);
    let opt = all_options();
    let specs = [src(&empty, ""), src(&a, "")];
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!(plan.files[0].state, FileState::Unreadable);
    assert!(plan.files[0].message.as_deref().unwrap().contains("empty.pdf"));
    // one contents page for the one file that goes in
    assert_eq!((plan.pages, plan.contents_pages, plan.left_out, plan.can_run), (3, 1, 1, true));
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    let r = combine::run(&eng, &specs, opt, &out).unwrap();
    assert_eq!(r.plan, plan);
    let (e2, i2) = open(&out);
    assert_eq!(e2.edit_state(i2).unwrap().page_count, 3);
    // only the empty file: nothing to combine, and no panic
    let only = [src(&empty, "")];
    assert!(!combine::inspect(&only, opt).unwrap().can_run);
    assert!(combine::run(&eng, &only, opt, &out).is_err());
}

#[test]
fn combine_merges_the_default_resources_of_every_form() {
    use crate::forms::{FieldKind, NewField, add_one};
    use mupdf::pdf::PdfDocument;
    let d = dir("combine-dr");
    let mut paths = Vec::new();
    for n in ["f1.pdf", "f2.pdf"] {
        let p = write(&d, n, 1, 0);
        let (eng, id) = open(&p);
        let what = NewField { kind: FieldKind::Text, rect: [72.0, 100.0, 272.0, 120.0], name: Some(format!("Field {n}")), group: None, export: None, caption: None, options: None };
        add_one(&eng, id, 0, &what, user()).unwrap();
        eng.save(id, None).unwrap();
        paths.push(p);
    }
    // the second form has a font the first one does not
    let second = d.join("f2-fonts.pdf").to_string_lossy().into_owned();
    {
        let doc = PdfDocument::open(&paths[1]).unwrap();
        let form = doc.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap();
        let mut dr = form.get_dict("DR").unwrap().expect("a form made by Inkwell has /DR");
        if dr.get_dict("Font").unwrap().is_none() {
            let fonts = doc.new_dict().unwrap();
            dr.dict_put("Font", fonts).unwrap();
        }
        let mut fonts = dr.get_dict("Font").unwrap().unwrap();
        let extra = doc.new_dict().unwrap();
        fonts.dict_put("InkwellOnlyHere", extra).unwrap();
        doc.save(&second).unwrap();
    }
    let (eng, _) = open(&paths[0]);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false };
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &[src(&paths[0], ""), src(&second, "")], opt, &out).unwrap();
    let doc = PdfDocument::open(&out).unwrap();
    let form = doc.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap();
    let fonts = form.get_dict("DR").unwrap().unwrap().get_dict("Font").unwrap().unwrap();
    assert!(fonts.get_dict("InkwellOnlyHere").unwrap().is_some(), "the later file's font is in the merged /DR");
}

#[test]
fn saving_an_attachment_never_writes_over_an_open_file() {
    let source = include_str!("app.rs");
    let from = source.find("pub async fn attachment_save").unwrap();
    let body = &source[from..from + source[from..].find("pub async fn attachment_open").unwrap()];
    let guard = body.find("check_not_open").expect("attachment_save checks the target is not open");
    assert!(guard < body.find("write_plain").unwrap(), "the check comes before the write");
}
