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
    NewFile { name: name.into(), data, description: desc.into(), modified: None }
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
    SourceSpec { path: path.into(), range: range.into(), title: None, password: None }
}

fn all_options() -> CombineOptions {
    CombineOptions { bookmarks: true, keep_outlines: true, contents_page: true, keep_labels: true }
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
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!((plan.pages, plan.bookmarks, plan.contents_pages), (4, 0, 0));
    assert_eq!(plan.summary, "Combines 2 files, 4 pages");
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &specs, opt, &out).unwrap();
    let (e2, i2) = open(&out);
    assert!(e2.outline(i2).unwrap().is_empty());
    // outlines kept without a bookmark per file: the file's own items, flat
    let opt = CombineOptions { bookmarks: false, keep_outlines: true, contents_page: false, keep_labels: true };
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
    let opt = CombineOptions { bookmarks: true, keep_outlines: false, contents_page: false, keep_labels: true };
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
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    assert!(combine::inspect(&specs, opt).unwrap().files.iter().all(|f| f.has_fields));
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &specs, opt, &out).unwrap();
    let (e2, i2) = open(&out);
    let mut names: Vec<(u32, String)> = list_all(&e2, i2).unwrap().fields.into_iter().map(|f| (f.page, f.name)).collect();
    names.sort();
    assert_eq!(names, [(0, "Name".to_string()), (1, "Name (2)".to_string())]);
}

#[test]
fn a_source_with_a_parent_loop_in_its_field_tree_combines_and_its_fields_are_listed() {
    use crate::outline::tests::raw_pdf;
    // widget 5 and field 6 are each other's /Parent; widget 7 is its own /Parent
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 7 0 R] >>".into(),
        "<< >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /Parent 6 0 R /Rect [10 10 100 50] /P 3 0 R >>".into(),
        "<< /T (Loop) /FT /Tx /Kids [5 0 R] /Parent 5 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Own) /Parent 7 0 R /Rect [10 60 100 100] /P 3 0 R >>".into(),
    ];
    let d = dir("combine-parent-loop");
    let a = write(&d, "a.pdf", 1, 0);
    let l = d.join("loop.pdf");
    std::fs::write(&l, raw_pdf(&objs)).unwrap();
    let l = l.to_string_lossy().into_owned();
    let (eng, _) = open(&a);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &[src(&a, ""), src(&l, "")], opt, &out).unwrap();
    let (e2, i2) = open(&out);
    let mut names: Vec<String> = crate::forms::list_all(&e2, i2).unwrap().fields.into_iter().map(|f| f.name).collect();
    names.sort();
    assert_eq!(names, ["Loop", "Own"]);
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
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
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
fn a_damaged_file_costs_only_its_own_place_in_the_plan() {
    let d = dir("combine-damaged");
    let a = write(&d, "a.pdf", 2, 0);
    let b = write(&d, "b.pdf", 3, 0);
    let bytes = std::fs::read(&b).unwrap();
    // cut off in the middle, and with the middle overwritten: MuPDF may repair them or not, but the
    // files around them must be planned either way
    let cut = d.join("cut.pdf");
    std::fs::write(&cut, &bytes[..bytes.len() * 6 / 10]).unwrap();
    let holed = d.join("holed.pdf");
    let mut h = bytes.clone();
    let (from, to) = (h.len() / 4, h.len() * 3 / 4);
    h[from..to].fill(0);
    std::fs::write(&holed, &h).unwrap();
    let (cut, holed) = (cut.to_string_lossy().into_owned(), holed.to_string_lossy().into_owned());
    let opt = CombineOptions { bookmarks: true, keep_outlines: false, contents_page: false, keep_labels: true };
    let specs = [src(&a, ""), src(&cut, ""), src(&holed, ""), src(&b, "")];
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!(plan.files.len(), 4);
    for i in [0, 3] {
        assert_eq!(plan.files[i].state, FileState::Ready, "{:?}", plan.files[i]);
    }
    assert_eq!((plan.files[0].pages, plan.files[3].pages), (2, 3));
    for f in &plan.files[1..3] {
        assert!(matches!(f.state, FileState::Ready | FileState::Unreadable), "{f:?}");
        if f.state == FileState::Unreadable {
            assert!(f.message.as_deref().unwrap().contains(&f.name), "{f:?}");
        }
    }
    // and a run writes what the plan said
    let (eng, _) = open(&a);
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    let r = combine::run(&eng, &specs, opt, &out).unwrap();
    assert_eq!(r.plan, plan);
    let (e2, i2) = open(&out);
    assert_eq!(e2.edit_state(i2).unwrap().page_count, plan.pages);
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
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
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

/// Raw key bytes of the /Names /EmbeddedFiles array of the file at `path`.
fn raw_attachment_keys(path: &str) -> Vec<Vec<u8>> {
    let d = mupdf::pdf::PdfDocument::open(path).unwrap();
    let names = d.catalog().unwrap().get_dict("Names").unwrap().unwrap();
    let arr = names.get_dict("EmbeddedFiles").unwrap().unwrap().get_dict("Names").unwrap().unwrap();
    (0..arr.len().unwrap()).step_by(2).map(|i| arr.get_array(i as i32).unwrap().unwrap().as_bytes().unwrap()).collect()
}

#[test]
fn attachment_keys_are_sorted_by_the_bytes_the_file_stores() {
    let d = dir("attach-order");
    let path = write(&d, "a.pdf", 1, 0);
    let (eng, id) = open(&path);
    // "räkning" sorts before "summary" as text, but is stored as UTF-16BE (FE FF ...), which sorts after it
    attach_add(&eng, id, vec![file("summary.pdf", b"s".to_vec(), ""), file("räkning.pdf", b"r".to_vec(), ""), file("alpha.txt", b"a".to_vec(), "")], user()).unwrap();
    let (e2, i2, saved) = save_reopen(&eng, id);
    let keys = raw_attachment_keys(&saved);
    assert_eq!(keys.len(), 3);
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "keys not ascending by stored bytes: {keys:?}");
    assert!(keys[2].starts_with(&[0xFE, 0xFF]));
    // and both are found by name after the reopen
    assert_eq!(attach_data(&e2, i2, "n:räkning.pdf").unwrap().1, b"r");
    assert_eq!(attach_data(&e2, i2, "n:summary.pdf").unwrap().1, b"s");
}

#[test]
fn a_name_tree_cut_short_by_a_cap_is_never_written_back_flat() {
    use crate::outline::tests::{open_bytes, raw_pdf};
    // 11 nested /Kids nodes: the walk stops at depth 8, so entries below were never seen
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles 5 0 R >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
        "<< >>".into(),
    ];
    let depth = 11;
    for i in 0..depth {
        let kids = if i + 1 < depth { format!("/Kids [{} 0 R]", 6 + i) } else { String::new() };
        objs.push(format!("<< {kids} /Names [(k{i:02}) {} 0 R] >>", 5 + depth + i));
    }
    objs.extend((0..depth).map(|i| format!("<< /Type /Filespec /F (k{i:02}) /UF (k{i:02}) >>")));
    let d = dir("attach-cut");
    let (eng, id, _) = open_bytes(&d, "a.pdf", &raw_pdf(&objs));
    let before = attach_list(&eng, id).unwrap().len();
    assert!(before < depth, "{before}");
    let r = attach_add(&eng, id, vec![file("new.txt", b"n".to_vec(), "")], user());
    assert!(r.is_err(), "an edit of a tree that was cut short must be refused");
    assert_eq!(attach_list(&eng, id).unwrap().len(), before);
    assert!(attach_remove(&eng, id, &["n:k00".to_string()], user()).is_err());
}

// ---- combine: source page labels and passwords --------------------------------------------------

/// A file of `pages` pages with `style`/`prefix` labels over all of them.
fn labelled(d: &std::path::Path, name: &str, pages: usize, style: LabelStyle, prefix: &str) -> String {
    let p = write(d, name, pages, 0);
    let (eng, id) = open(&p);
    labels_apply(&eng, id, 0, pages as u32 - 1, &spec(style, prefix, 1), user()).unwrap();
    eng.save(id, None).unwrap();
    p
}

#[test]
fn combine_carries_the_source_page_labels_to_the_pages_it_takes() {
    let d = dir("combine-labels");
    let a = labelled(&d, "a.pdf", 4, LabelStyle::LowerRoman, "");
    let b = labelled(&d, "b.pdf", 5, LabelStyle::Decimal, "A-");
    let c = write(&d, "c.pdf", 3, 0);
    let (eng, _) = open(&c);
    let specs = [src(&a, "2-3"), src(&b, "5-4, 1"), src(&c, "")];
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let run = |name: &str, opt: CombineOptions, specs: &[SourceSpec]| {
        let out = d.join(name).to_string_lossy().into_owned();
        combine::run(&eng, specs, opt, &out).unwrap();
        let (e2, i2) = open(&out);
        labels(&e2, i2)
    };

    // shifted to the page offset, limited to the picked pages (in the order picked); the file without
    // labels gets plain decimal numbers that carry on the combined page numbers
    assert_eq!(run("out.pdf", opt, &specs), ["ii", "iii", "A-5", "A-4", "A-1", "6", "7", "8"]);

    // a contents page in front is plain decimal and counted
    let toc = CombineOptions { contents_page: true, ..opt };
    assert_eq!(run("toc.pdf", toc, &specs), ["1", "ii", "iii", "A-5", "A-4", "A-1", "7", "8", "9"]);

    // unticked: the result has none
    let off = CombineOptions { keep_labels: false, ..opt };
    assert!(run("off.pdf", off, &specs).is_empty());

    // nothing labelled anywhere: a plain document stays plain
    assert!(run("plain.pdf", opt, &[src(&c, ""), src(&c, "1")]).is_empty());

    // a source that spells out plain decimal numbering (what many producers write) counts as unlabelled:
    // it carries on the combined numbers instead of starting again at 1
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /PageLabels << /Nums [0 << /S /D >>] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
    ];
    let explicit = d.join("explicit.pdf");
    std::fs::write(&explicit, crate::outline::tests::raw_pdf(&objs)).unwrap();
    let explicit = explicit.to_string_lossy().into_owned();
    assert!(run("explicit1.pdf", opt, &[src(&c, ""), src(&explicit, "")]).is_empty());
    assert_eq!(run("explicit2.pdf", opt, &[src(&b, "5"), src(&explicit, "")]), ["A-5", "2"]);
}

#[test]
fn a_hostile_page_label_tree_is_bounded_and_never_stops_a_combine() {
    use crate::outline::tests::{open_bytes, raw_pdf};
    // one node that is its own kid 200 000 times over: nothing is ever added to the result (the depth cap
    // alone leaves 200 000 ^ 8 visits), so only a cap on the work stops the walk
    let kids = "5 0 R ".repeat(200_000);
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /PageLabels 5 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
        "<< >>".into(),
        format!("<< /Kids [{kids}] >>"),
    ];
    let d = dir("combine-labels-hostile");
    let bytes = raw_pdf(&objs);
    let t = std::time::Instant::now();
    let (eng, id, path) = open_bytes(&d, "hostile.pdf", &bytes);
    assert!(labels(&eng, id).is_empty());
    let a = write(&d, "a.pdf", 2, 0);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    let specs = [src(&path, ""), src(&a, "")];
    let report = combine::run(&eng, &specs, opt, &out).unwrap();
    assert_eq!(report.plan.pages, 3);
    assert!(t.elapsed() < std::time::Duration::from_secs(20), "{:?}", t.elapsed());
}

fn locked_copy(d: &std::path::Path, name: &str, pages: usize, may_copy: bool) -> String {
    locked_with(d, name, pages, may_copy, "secret-pw")
}

fn locked_with(d: &std::path::Path, name: &str, pages: usize, may_copy: bool, user_pw: &str) -> String {
    use mupdf::pdf::{Encryption, PdfDocument, PdfWriteOptions, Permission};
    let path = d.join(name).to_string_lossy().into_owned();
    let doc = PdfDocument::from_bytes(&fixture::build(&Fixture { pages, ..Default::default() })).unwrap();
    let mut o = PdfWriteOptions::default();
    o.set_encryption(Encryption::Aes256).set_user_password(user_pw).set_owner_password("owner-pw");
    if !may_copy {
        o.set_permissions(Permission::PRINT);
    }
    doc.save_with_options(&path, o).unwrap();
    path
}

fn with_password(path: &str, pw: &str) -> SourceSpec {
    SourceSpec { password: Some(pw.into()), ..src(path, "") }
}

#[test]
fn a_locked_source_combines_with_its_password_and_the_result_has_none() {
    let d = dir("combine-password");
    let a = write(&d, "a.pdf", 2, 0);
    let locked = locked_copy(&d, "locked.pdf", 3, true);
    let (eng, _) = open(&a);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let out = d.join("out.pdf").to_string_lossy().into_owned();

    // none, then a wrong one: left out, the plan is the same, and the run goes on without it
    let none = combine::inspect(&[src(&a, ""), src(&locked, "")], opt).unwrap();
    assert_eq!(none.files[1].state, FileState::Encrypted);
    let wrong_specs = [src(&a, ""), with_password(&locked, "not-it")];
    let wrong = combine::inspect(&wrong_specs, opt).unwrap();
    assert_eq!(wrong.files[1].state, FileState::Encrypted);
    assert_eq!((wrong.pages, wrong.left_out, wrong.can_run), (none.pages, none.left_out, none.can_run));
    assert_eq!(combine::run(&eng, &wrong_specs, opt, &out).unwrap().plan.pages, 2);
    assert!(combine::run(&eng, &[with_password(&locked, "not-it")], opt, &out).is_err());

    // the right one: its pages go in; the result opens without any password
    let specs = [src(&a, ""), with_password(&locked, "secret-pw")];
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!((plan.files[1].state.clone(), plan.pages, plan.left_out), (FileState::Ready, 5, 0));
    let report = combine::run(&eng, &specs, opt, &out).unwrap();
    assert_eq!(report.plan, plan, "the run is what was predicted");
    let (e2, i2) = open(&out);
    assert_eq!(e2.edit_state(i2).unwrap().page_count, 5);

    // the password is in no serialised form, debug print or error text
    let mut seen = vec![format!("{specs:?}"), serde_json::to_string(&plan).unwrap(), serde_json::to_string(&report).unwrap()];
    seen.push(combine::run(&eng, &[with_password(&locked, "not-it")], opt, &out).unwrap_err().to_string());
    seen.push(serde_json::to_string(&wrong).unwrap());
    for s in seen {
        assert!(!s.contains("secret-pw") && !s.contains("not-it"), "{s}");
    }

    // the engine handle of the sheet's row: a wrong password changes nothing, the right one opens it
    let eng2 = Engine::new(2, 64 << 20);
    let (id, r) = eng2.open_pending(&locked);
    assert!(matches!(r, Err(crate::engine::EngineError::NeedsPassword)));
    assert!(matches!(eng2.authenticate(id, "not-it"), Err(crate::engine::EngineError::BadPassword)));
    assert_eq!(eng2.authenticate(id, "secret-pw").unwrap().page_count, 3);
}

#[test]
fn an_owner_restriction_still_leaves_a_source_out_after_it_opens() {
    let d = dir("combine-restricted");
    let a = write(&d, "a.pdf", 2, 0);
    let locked = locked_copy(&d, "restricted.pdf", 3, false);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let plan = combine::inspect(&[src(&a, ""), with_password(&locked, "secret-pw")], opt).unwrap();
    assert_eq!(plan.files[1].state, FileState::NoCopy);
    assert!(plan.files[1].message.as_deref().unwrap().contains("doesn't allow copying"));
    assert_eq!((plan.pages, plan.left_out), (2, 1));
    let (eng, _) = open(&a);
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    let specs = [src(&a, ""), with_password(&locked, "secret-pw")];
    assert_eq!(combine::run(&eng, &specs, opt, &out).unwrap().plan.pages, 2);
}

#[test]
fn a_source_with_security_says_its_password_and_restrictions_are_not_carried_over() {
    use mupdf::pdf::{Encryption, PdfDocument, PdfWriteOptions, Permission};
    let d = dir("combine-security");
    let a = write(&d, "a.pdf", 2, 0);
    // an owner-password-only source: opens with no password, may be copied and rearranged, forbids the rest
    let owner = d.join("owner.pdf").to_string_lossy().into_owned();
    let doc = PdfDocument::from_bytes(&fixture::build(&Fixture { pages: 2, ..Default::default() })).unwrap();
    let mut o = PdfWriteOptions::default();
    o.set_encryption(Encryption::Aes256).set_owner_password("owner-pw").set_permissions(Permission::COPY | Permission::ASSEMBLE);
    doc.save_with_options(&owner, o).unwrap();
    assert!(!PdfDocument::open(&owner).unwrap().needs_password().unwrap(), "the fixture must open without a password");
    let locked = locked_copy(&d, "locked.pdf", 2, true);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let plan = combine::inspect(&[src(&a, ""), src(&owner, ""), with_password(&locked, "secret-pw")], opt).unwrap();
    assert_eq!(plan.files[0].note, None);
    assert!(!plan.files[0].encrypted);
    assert_eq!(plan.files[1].state, FileState::Ready);
    assert!(plan.files[1].encrypted);
    assert_eq!(
        plan.files[1].note.as_deref(),
        Some("owner.pdf's security is not carried over (it does not allow printing, editing, commenting, form filling, screen reader access)")
    );
    // a source with a user password keeps the wording that names the password
    assert!(plan.files[2].encrypted);
    assert_eq!(plan.files[2].note.as_deref(), Some("locked.pdf's password and restrictions are not carried over"));
    // not opened yet: left out, and it is not said to bring anything
    let closed = combine::inspect(&[src(&locked, "")], opt).unwrap();
    assert_eq!((closed.files[0].state.clone(), closed.files[0].note.clone(), closed.files[0].encrypted), (FileState::Encrypted, None, false));
    // the result has none of it
    let (eng, _) = open(&a);
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    combine::run(&eng, &[src(&a, ""), src(&owner, "")], opt, &out).unwrap();
    assert!(!PdfDocument::open(&out).unwrap().needs_password().unwrap());
}

#[test]
fn a_revision_2_source_has_its_page_rights_read_from_the_older_bits() {
    use mupdf::pdf::{Encryption, PdfDocument, PdfWriteOptions, Permission};
    let d = dir("combine-r2");
    let a = write(&d, "a.pdf", 1, 0);
    // RC4-40 is revision 2: MuPDF writes only the bits it is given, so 9 to 12 are clear
    let r2 = |name: &str, p: Permission| {
        let path = d.join(name).to_string_lossy().into_owned();
        let doc = PdfDocument::from_bytes(&fixture::build(&Fixture { pages: 2, ..Default::default() })).unwrap();
        let mut o = PdfWriteOptions::default();
        o.set_encryption(Encryption::Rc4_40).set_owner_password("owner-pw").set_permissions(p);
        doc.save_with_options(&path, o).unwrap();
        let on_disk = PdfDocument::open(&path).unwrap();
        let enc = on_disk.trailer().unwrap().get_dict("Encrypt").unwrap().unwrap();
        assert_eq!(enc.get_dict("R").unwrap().unwrap().as_int().unwrap(), 2);
        assert_eq!(enc.get_dict("P").unwrap().unwrap().as_int().unwrap() as u32 & 0xf00, 0, "bits 9 to 12 are clear in the fixture");
        path
    };
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    // copy but no change right (bit 4 clear): page changes follow bit 4, so the file is left out
    let copy_only = r2("copy.pdf", Permission::COPY);
    let plan = combine::inspect(&[src(&a, ""), src(&copy_only, "")], opt).unwrap();
    assert_eq!(plan.files[1].state, FileState::NoCopy);
    assert!(plan.files[1].message.as_deref().unwrap().contains("doesn't allow copying"));
    // copy and modify: it goes in; page changes follow modify and screen readers follow copy, so only
    // printing, commenting and form filling (bit 9 follows bit 6) are named
    let edit = r2("edit.pdf", Permission::COPY | Permission::MODIFY);
    let plan = combine::inspect(&[src(&a, ""), src(&edit, "")], opt).unwrap();
    assert_eq!(plan.files[1].state, FileState::Ready);
    assert_eq!(
        plan.files[1].note.as_deref(),
        Some("edit.pdf's security is not carried over (it does not allow printing, commenting, form filling)")
    );
}

#[test]
fn a_password_typed_full_width_opens_a_file_protect_wrote_in_nfkc() {
    let d = dir("combine-nfkc");
    let a = write(&d, "a.pdf", 2, 0);
    // Protect writes the password in NFKC; the person types it full-width
    let locked = locked_with(&d, "locked.pdf", 3, true, "ABC-secret");
    let typed = "\u{ff21}\u{ff22}\u{ff23}-secret";
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let specs = [src(&a, ""), with_password(&locked, typed)];
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!((plan.files[1].state.clone(), plan.pages), (FileState::Ready, 5), "{:?}", plan.files[1].message);
    let (eng, _) = open(&a);
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    assert_eq!(combine::run(&eng, &specs, opt, &out).unwrap().plan.pages, 5);
    // the sheet's own unlock takes the same typed form
    let eng2 = Engine::new(2, 64 << 20);
    let (id, _) = eng2.open_pending(&locked);
    assert_eq!(eng2.authenticate(id, typed).unwrap().page_count, 3);
    // still refused when it is not the password in any form
    assert_eq!(combine::inspect(&[with_password(&locked, "ＸＹＺ")], opt).unwrap().files[0].state, FileState::Encrypted);
}

#[test]
fn unlocking_for_combine_refuses_a_document_that_is_not_waiting_for_a_password() {
    let d = dir("combine-unlock-scope");
    let locked = locked_copy(&d, "locked.pdf", 2, true);
    let plain = write(&d, "plain.pdf", 2, 0);
    let eng = Engine::new(2, 64 << 20);
    // a tab: opened with its password, then a wrong one comes in through the combine command
    let (tab, r) = eng.open_pending(&locked);
    assert!(matches!(r, Err(crate::engine::EngineError::NeedsPassword)));
    assert_eq!(eng.authenticate(tab, "secret-pw").unwrap().page_count, 2);
    assert!(eng.unlock_pending(tab, "not-it").is_err());
    assert!(eng.unlock_pending(tab, "secret-pw").is_err(), "it is open already");
    assert_eq!(eng.edit_state(tab).unwrap().page_count, 2, "and it still has its password");
    assert!(eng.outline(tab).is_ok());
    // a document that never needed one
    let open_plain = eng.open(&plain).unwrap().doc;
    assert!(eng.unlock_pending(open_plain, "x").is_err());
    assert!(eng.outline(open_plain).is_ok());
    // the one that is waiting: a wrong password changes nothing, the right one opens it
    let (wait, _) = eng.open_pending(&locked);
    assert!(matches!(eng.unlock_pending(wait, "not-it"), Err(crate::engine::EngineError::BadPassword)));
    assert!(matches!(eng.unlock_pending(wait, "not-it"), Err(crate::engine::EngineError::BadPassword)));
    assert_eq!(eng.unlock_pending(wait, "secret-pw").unwrap().page_count, 2);
}

#[test]
fn a_signature_field_of_a_source_arrives_empty_and_the_plan_says_so() {
    use crate::outline::tests::{open_bytes, raw_pdf};
    // object 5: a signature that is its own widget; 6 and 7: a signature field that holds the value and a
    // widget kid that does not
    let value = "/V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >>";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /SigFlags 3 /Fields [5 0 R 6 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 7 0 R] >>".into(),
        "<< >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (First) /Rect [10 10 100 50] /P 3 0 R /AP << /N 4 0 R >> {value} >>"),
        format!("<< /FT /Sig /T (Second) /Kids [7 0 R] {value} >>"),
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [10 60 100 100] /P 3 0 R /AP << /N 4 0 R >> >>".into(),
    ];
    let d = dir("combine-signed");
    let (eng, id, path) = open_bytes(&d, "s.pdf", &raw_pdf(&objs));
    assert!(eng.read_custom(id, |d| Ok(crate::outline::signed(d))).unwrap(), "the fixture is signed");
    let a = write(&d, "a.pdf", 1, 0);
    let opt = CombineOptions { bookmarks: false, keep_outlines: false, contents_page: false, keep_labels: true };
    let specs = [src(&a, ""), src(&path, "")];
    let plan = combine::inspect(&specs, opt).unwrap();
    assert_eq!(plan.files[0].note, None);
    assert_eq!(plan.files[1].note.as_deref(), Some("s.pdf's signature is not carried over"));
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    assert_eq!(combine::run(&eng, &specs, opt, &out).unwrap().plan, plan);

    let (e2, i2) = open(&out);
    assert!(!e2.read_custom(i2, |d| Ok(crate::outline::signed(d))).unwrap(), "no signed document comes out");
    assert_eq!(crate::forms::list_all(&e2, i2).unwrap().fields.len(), 2, "the signature fields are still there");
    let bytes = std::fs::read(&out).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("adbe.pkcs7.detached"), "no signature value in the file");
    // neither widget draws the old signature: one has its own value, the other gets it from its parent field
    let drawn = e2
        .read_custom(i2, |d| {
            let mut n = 0;
            for p in 0..d.page_count()? {
                let Some(annots) = d.find_page(p)?.get_dict("Annots")? else { continue };
                for i in 0..annots.len()? as i32 {
                    if annots.get_array(i)?.is_some_and(|w| w.get_dict("AP").ok().flatten().is_some()) {
                        n += 1;
                    }
                }
            }
            Ok(n)
        })
        .unwrap();
    assert_eq!(drawn, 0, "an empty signature field has no appearance");
}
