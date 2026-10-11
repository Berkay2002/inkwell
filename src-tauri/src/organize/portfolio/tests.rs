//! Portfolios: reading the collection, creating one, extracting, and editing one through the edit lane.

use super::*;
use crate::engine::edit::EditSource;
use crate::engine::fixture::{self, Fixture};
use crate::engine::{DocId, Engine};
use crate::organize::attach::{self, Entry, NewFile, stream_of};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("portfolio-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(path: &str) -> (Arc<Engine>, DocId) {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(path).unwrap().doc;
    (eng, id)
}

fn pdf(d: &Path, name: &str, pages: usize) -> String {
    let p = d.join(name);
    std::fs::write(&p, fixture::build(&Fixture { pages, chapter_every: 0, lines: 4, ..Default::default() })).unwrap();
    p.to_string_lossy().into_owned()
}

// ---- building portfolios by hand ----------------------------------------------------------------

fn file(name: &str, data: &[u8]) -> NewFile {
    NewFile { name: name.into(), data: data.to_vec(), description: String::new(), modified: None }
}

fn dict(doc: &mut PdfDocument, ty: &str) -> PdfObject {
    let mut d = doc.new_dict().unwrap();
    put_name(&mut d, "Type", ty).unwrap();
    d
}

/// (key, subtype, shown name, /O, visible)
type FieldSpec = (&'static str, &'static str, &'static str, i32, bool);

fn set_collection(doc: &mut PdfDocument, fields: &[FieldSpec], sort: &[(&str, bool)], folders: Option<PdfObject>, initial: Option<&str>) {
    let mut schema = dict(doc, "CollectionSchema");
    for (key, sub, name, order, visible) in fields {
        let mut f = dict(doc, "CollectionField");
        put_name(&mut f, "Subtype", sub).unwrap();
        put_str(&mut f, "N", name).unwrap();
        put_int(&mut f, "O", *order).unwrap();
        f.dict_put("V", PdfObject::new_bool(*visible)).unwrap();
        schema.dict_put(*key, f).unwrap();
    }
    let mut coll = dict(doc, "Collection");
    coll.dict_put("Schema", schema).unwrap();
    if !sort.is_empty() {
        let mut s = dict(doc, "CollectionSort");
        let mut keys = doc.new_array().unwrap();
        let mut asc = doc.new_array().unwrap();
        for (k, a) in sort {
            keys.array_push(PdfObject::new_name(k).unwrap()).unwrap();
            asc.array_push(PdfObject::new_bool(*a)).unwrap();
        }
        s.dict_put("S", keys).unwrap();
        s.dict_put("A", asc).unwrap();
        coll.dict_put("Sort", s).unwrap();
    }
    if let Some(f) = folders {
        coll.dict_put("Folders", f).unwrap();
    }
    if let Some(d) = initial {
        put_str(&mut coll, "D", d).unwrap();
    }
    doc.catalog().unwrap().dict_put("Collection", coll).unwrap();
}

fn spec_of(doc: &PdfDocument, key: &str) -> PdfObject {
    let e = tree(doc).unwrap().into_iter().find(|e| e.key == key).unwrap();
    e.spec.resolve().unwrap().unwrap()
}

fn set_ci(doc: &mut PdfDocument, key: &str, strs: &[(&str, &str)], nums: &[(&str, i32)]) {
    let mut ci = dict(doc, "CollectionItem");
    for (k, v) in strs {
        put_str(&mut ci, k, v).unwrap();
    }
    for (k, v) in nums {
        put_int(&mut ci, k, *v).unwrap();
    }
    spec_of(doc, key).dict_put("CI", ci).unwrap();
}

/// Renames embedded files' keys (a folder prefix), in place.
fn rename_keys(doc: &mut PdfDocument, renames: &[(&str, &str)]) {
    let mut entries: Vec<Entry> = tree(doc).unwrap();
    for e in &mut entries {
        if let Some((_, to)) = renames.iter().find(|(from, _)| *from == e.key) {
            e.key = (*to).to_string();
        }
    }
    attach::write_tree(doc, entries).unwrap();
}

fn folder(doc: &mut PdfDocument, id: i32, name: &str) -> PdfObject {
    let mut f = dict(doc, "Folder");
    put_int(&mut f, "ID", id).unwrap();
    if !name.is_empty() {
        put_str(&mut f, "Name", name).unwrap();
    }
    doc.add_object(&f).unwrap()
}

fn resolved(o: &PdfObject) -> PdfObject {
    o.resolve().unwrap().unwrap()
}

const FOUR: [FieldSpec; 5] = [
    ("Name", "F", "Name", 0, true),
    ("Author", "S", "Author", 1, true),
    ("Pages", "N", "Pages", 2, true),
    ("Due", "D", "Due", 3, true),
    ("Notes", "S", "Notes", 4, false),
];

/// A portfolio with a five-field schema, a two-key sort and nested folders.
fn sample() -> PdfDocument {
    let mut doc = PdfDocument::new();
    for n in ["alpha.pdf", "beta.pdf", "gamma.txt", "delta.pdf", "epsilon.pdf"] {
        let data = if n.ends_with(".pdf") { fixture::build(&Fixture { pages: 1, chapter_every: 0, lines: 4, ..Default::default() }) } else { n.as_bytes().to_vec() };
        attach::add_one(&mut doc, &file(n, &data)).unwrap();
    }
    rename_keys(&mut doc, &[("beta.pdf", "<1>beta.pdf"), ("gamma.txt", "<2>gamma.txt"), ("delta.pdf", "<3>delta.pdf")]);
    set_ci(&mut doc, "alpha.pdf", &[("Author", "Zed"), ("Due", "D:20260301")], &[("Pages", 10)]);
    set_ci(&mut doc, "<1>beta.pdf", &[("Author", "Amy")], &[("Pages", 9)]);
    set_ci(&mut doc, "<2>gamma.txt", &[("Author", "amy")], &[("Pages", 100)]);
    set_ci(&mut doc, "<3>delta.pdf", &[("Author", "Bob"), ("Due", "D:20250101120000+02'00'")], &[("Pages", 2)]);
    // root > Reports (1) > 2026 (2), and Archive (3) beside Reports
    let b = folder(&mut doc, 2, "2026");
    let mut c = folder(&mut doc, 3, "Archive");
    let mut a = folder(&mut doc, 1, "Reports");
    resolved(&a).dict_put("Child", b).unwrap();
    resolved(&a).dict_put("Next", c.clone()).unwrap();
    let _ = &mut c;
    let mut root = dict(&mut doc, "Folder");
    put_int(&mut root, "ID", 0).unwrap();
    root.dict_put("Child", a.clone()).unwrap();
    let _ = &mut a;
    let root = doc.add_object(&root).unwrap();
    set_collection(&mut doc, &FOUR, &[("Author", true), ("Pages", false)], Some(root), Some("alpha.pdf"));
    doc
}

/// Writes `sample()` (with a cover page) where the hand-made portfolio can be looked at in the app:
/// `INKWELL_SAMPLE_OUT=/path/sample.pdf cargo test --lib write_sample_portfolio -- --ignored`.
#[test]
#[ignore]
fn write_sample_portfolio() {
    let out = std::env::var("INKWELL_SAMPLE_OUT").unwrap_or_else(|_| std::env::temp_dir().join("inkwell-portfolio-sample.pdf").to_string_lossy().into_owned());
    let mut doc = sample();
    let rows = [("alpha.pdf".to_string(), 1u64), ("beta.pdf".to_string(), 1)];
    super::make::write_cover(&mut doc, &rows, 2).unwrap();
    let bytes = crate::engine::edit::write_doc(&doc, crate::engine::edit::rewrite_opts(), 1 << 20).unwrap();
    std::fs::write(&out, bytes).unwrap();
    println!("wrote {out}");
}

#[test]
fn a_collection_without_a_schema_lists_the_files_own_facts() {
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("b.txt", &[1u8; 20])).unwrap();
    attach::add_one(&mut doc, &NewFile { name: "a.pdf".into(), data: vec![2u8; 3000], description: "First".into(), modified: Some(1_700_000_000) }).unwrap();
    let mut coll = dict(&mut doc, "Collection");
    put_name(&mut coll, "View", "D").unwrap();
    put_str(&mut coll, "D", "a.pdf").unwrap();
    doc.catalog().unwrap().dict_put("Collection", coll).unwrap();
    let info = read(&doc).unwrap().unwrap();
    assert_eq!(info.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Name", "Description", "Size", "Modified"]);
    assert_eq!(info.files.iter().map(|f| f.cells.clone()).collect::<Vec<_>>(), [
        vec!["a.pdf", "First", "2.9 KB", "2023-11-14 22:13"],
        vec!["b.txt", "", "20 B", ""],
    ]);
    assert_eq!(info.initial.as_deref(), Some("n:a.pdf"));

    // a schema with no visible file name gets one in front
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("a.txt", b"x")).unwrap();
    set_collection(&mut doc, &[("Who", "S", "Author", 0, true)], &[("Who", true)], None, None);
    set_ci(&mut doc, "a.txt", &[("Who", "Amy")], &[]);
    let info = read(&doc).unwrap().unwrap();
    assert_eq!(info.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Name", "Author"]);
    assert_eq!(info.files[0].cells, ["a.txt", "Amy"]);
    assert_eq!(info.sort, [SortKey { field: 1, ascending: true }]);
}

#[test]
fn a_plain_pdf_is_not_a_portfolio() {
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("a.txt", b"x")).unwrap();
    assert!(read(&doc).unwrap().is_none());
}

#[test]
fn schema_sort_and_folders_come_out_in_the_authors_order() {
    let doc = sample();
    let info = read(&doc).unwrap().unwrap();
    assert_eq!(info.view, "details");
    assert_eq!(info.fields.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["Name", "Author", "Pages", "Due", "Notes"]);
    assert_eq!(info.fields.iter().map(|f| f.kind.as_str()).collect::<Vec<_>>(), ["text", "text", "number", "date", "text"]);
    assert_eq!(info.fields.iter().map(|f| f.visible).collect::<Vec<_>>(), [true, true, true, true, false]);
    assert_eq!(info.sort, [SortKey { field: 1, ascending: true }, SortKey { field: 2, ascending: false }]);
    assert_eq!(info.initial.as_deref(), Some("n:alpha.pdf"));

    // Author ascending (case-insensitive: "Amy" and "amy" tie), then pages descending; no author last.
    let names: Vec<&str> = info.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["gamma.txt", "beta.pdf", "delta.pdf", "alpha.pdf", "epsilon.pdf"]);
    let gamma = &info.files[0];
    assert_eq!(gamma.id, "n:<2>gamma.txt");
    assert_eq!(gamma.cells, ["gamma.txt", "amy", "100", "", ""]);
    assert!(!gamma.is_pdf && info.files[1].is_pdf);
    // dates read as dates: 2025 comes before 2026, and the zone moves the clock (10:00 UTC for 12:00+02:00)
    let due = |n: &str| info.files.iter().find(|f| f.name == n).unwrap();
    assert!(due("delta.pdf").ranks[3] < due("alpha.pdf").ranks[3]);
    assert_eq!(due("delta.pdf").cells[3], "2025-01-01 12:00");
    assert_eq!(due("epsilon.pdf").ranks[1], None);

    // folders: parents first, siblings by name; files map through the <id> prefix
    let f: Vec<(&str, Option<u32>)> = info.folders.iter().map(|f| (f.name.as_str(), f.parent)).collect();
    assert_eq!(f, [("Archive", None), ("Reports", None), ("2026", Some(1))]);
    let in_folder = |n: &str| due(n).folder;
    assert_eq!((in_folder("beta.pdf"), in_folder("gamma.txt"), in_folder("delta.pdf"), in_folder("alpha.pdf")), (Some(1), Some(2), Some(3), None));
}

#[test]
fn without_a_ci_the_files_own_facts_fill_the_columns() {
    let mut doc = PdfDocument::new();
    let f = NewFile { name: "q3.pdf".into(), data: vec![7u8; 2048], description: "Quarterly".into(), modified: Some(1_700_000_000) };
    attach::add_one(&mut doc, &f).unwrap();
    let fields: [FieldSpec; 4] = [("N", "F", "Name", 0, true), ("S", "Size", "Size", 1, true), ("M", "ModDate", "Modified", 2, true), ("D", "Desc", "Description", 3, true)];
    set_collection(&mut doc, &fields, &[], None, None);
    let info = read(&doc).unwrap().unwrap();
    assert_eq!(info.files[0].cells, ["q3.pdf", "2.0 KB", "2023-11-14 22:13", "Quarterly"]);
    // the file's own description, size and date win over a /CI that says otherwise (ISO 32000, as Acrobat reads it) ...
    set_ci(&mut doc, "q3.pdf", &[("D", "Final"), ("M", "D:20240102030405Z")], &[("S", 5)]);
    let info = read(&doc).unwrap().unwrap();
    assert_eq!(info.files[0].cells, ["q3.pdf", "2.0 KB", "2023-11-14 22:13", "Quarterly"]);
    // ... and /CI stands in for a fact the file does not give
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("n.txt", b"x")).unwrap();
    let mut stream = stream_of(&spec_of(&doc, "n.txt")).unwrap().resolve().unwrap().unwrap();
    stream.dict_delete("Params").unwrap();
    set_collection(&mut doc, &fields, &[], None, None);
    set_ci(&mut doc, "n.txt", &[("D", "Only in CI"), ("M", "D:20240102030405Z")], &[("S", 5)]);
    let info = read(&doc).unwrap().unwrap();
    assert_eq!(info.files[0].cells[2], "2024-01-02 03:04");
    assert_eq!(info.files[0].cells[3], "Only in CI");
}

#[test]
fn a_sub_item_brings_its_prefix() {
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("a.txt", b"x")).unwrap();
    set_collection(&mut doc, &[("Rev", "S", "Revision", 0, true)], &[], None, None);
    let mut sub = dict(&mut doc, "CollectionSubitem");
    put_str(&mut sub, "D", "12").unwrap();
    put_str(&mut sub, "P", "Rev. ").unwrap();
    let mut ci = dict(&mut doc, "CollectionItem");
    ci.dict_put("Rev", sub).unwrap();
    spec_of(&doc, "a.txt").dict_put("CI", ci).unwrap();
    assert_eq!(read(&doc).unwrap().unwrap().files[0].cells, ["a.txt", "Rev. 12"]);
}

#[test]
fn folders_that_loop_are_refused() {
    // a sibling that points back at itself
    let mut doc = PdfDocument::new();
    let a = folder(&mut doc, 1, "A");
    resolved(&a).dict_put("Next", a.clone()).unwrap();
    let mut root = dict(&mut doc, "Folder");
    root.dict_put("Child", a).unwrap();
    let root = doc.add_object(&root).unwrap();
    set_collection(&mut doc, &[], &[], Some(root), None);
    let e = read(&doc).unwrap_err().to_string();
    assert!(e.contains("loop"), "{e}");

    // a child that is its own ancestor
    let mut doc = PdfDocument::new();
    let a = folder(&mut doc, 1, "A");
    let b = folder(&mut doc, 2, "B");
    resolved(&a).dict_put("Child", b.clone()).unwrap();
    resolved(&b).dict_put("Child", a.clone()).unwrap();
    let mut root = dict(&mut doc, "Folder");
    root.dict_put("Child", a).unwrap();
    let root = doc.add_object(&root).unwrap();
    set_collection(&mut doc, &[], &[], Some(root), None);
    assert!(read(&doc).unwrap_err().to_string().contains("loop"));
}

#[test]
fn folders_nested_too_deep_are_refused() {
    let mut doc = PdfDocument::new();
    let mut parent = folder(&mut doc, 1, "d1");
    let first = parent.clone();
    for i in 2..=40 {
        let c = folder(&mut doc, i, "d");
        resolved(&parent).dict_put("Child", c.clone()).unwrap();
        parent = c;
    }
    let mut root = dict(&mut doc, "Folder");
    root.dict_put("Child", first).unwrap();
    let root = doc.add_object(&root).unwrap();
    set_collection(&mut doc, &[], &[], Some(root), None);
    assert!(read(&doc).unwrap_err().to_string().contains("deeply"));
}

#[test]
fn natural_order_and_dates() {
    let mut names = ["file10", "File2", "file1", "file02", "a", "B"].map(|s| fold_text(s));
    names.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(names, ["a", "b", "file1", "file2", "file02", "file10"].map(String::from));
    // a digit run of any length compares without overflowing
    assert_eq!(natural_cmp(&"9".repeat(40), &format!("1{}", "0".repeat(40))), Ordering::Less);
    assert_eq!(parse_pdf_date("D:20260102").unwrap(), (1_767_312_000, "2026-01-02".to_string()));
    assert_eq!(parse_pdf_date("D:20260102030405-05'00'").unwrap().0, 1_767_312_000 + 3 * 3600 + 4 * 60 + 5 + 5 * 3600);
    assert!(parse_pdf_date("D:2026133").is_none() && parse_pdf_date("not a date").is_none());
}

// ---- create --------------------------------------------------------------------------------------

fn sources(files: &[(&str, &str)]) -> Vec<Source> {
    files.iter().map(|(n, p)| Source { name: (*n).into(), path: (*p).into() }).collect()
}

#[test]
fn create_writes_a_portfolio_that_reads_back_byte_identical() {
    let d = dir("create");
    let a = pdf(&d, "a.pdf", 2);
    let b = pdf(&d, "b.pdf", 1);
    let c = d.join("notes.txt");
    std::fs::write(&c, "line one\nline two\n").unwrap();
    std::fs::create_dir_all(d.join("other")).unwrap();
    let c2 = d.join("other").join("notes.txt");
    std::fs::write(&c2, "second\n").unwrap();
    let before: Vec<Vec<u8>> = [&a, &b].iter().map(|p| std::fs::read(p).unwrap()).collect();
    let out = d.join("Portfolio.pdf").to_string_lossy().into_owned();

    let eng = Engine::new(2, 64 << 20);
    let src = sources(&[("notes.txt", c.to_str().unwrap()), ("b.pdf", &b), ("a.pdf", &a), ("notes.txt", c2.to_str().unwrap())]);
    let made = create(&eng, &src, &out).unwrap();
    assert_eq!(made.files, 4);

    let (e2, id) = open(&out);
    let info = e2.read_custom(id, |d| read(d)).unwrap().expect("a /Collection");
    // the reader that ignores /Collection sees a cover page
    assert_eq!(e2.read_custom(id, |d| Ok(d.page_count()?)).unwrap(), 1);
    assert_eq!(info.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Name", "Description", "Size", "Modified"]);
    assert_eq!(info.sort, [SortKey { field: 0, ascending: true }]);
    // sorted by name, a clash kept both ("notes (2).txt"), the first PDF opens first
    let names: Vec<&str> = info.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a.pdf", "b.pdf", "notes (2).txt", "notes.txt"]);
    assert_eq!(info.initial.as_deref(), Some("n:b.pdf"));
    for (f, want) in info.files.iter().zip(["a.pdf", "b.pdf", "notes (2).txt", "notes.txt"]) {
        assert!(!f.cells[2].is_empty() && !f.cells[3].is_empty(), "{want}: {:?}", f.cells);
        assert_eq!(f.cells[0], want);
    }
    // what the file carries for readers of the PDF itself
    e2.read_custom(id, |d| {
        let spec = spec_of(d, "a.pdf");
        assert_eq!(get(&spec, "AFRelationship").and_then(|v| name_of(&v)).as_deref(), Some("Unspecified"));
        let params = resolved(&get(&stream_of(&spec).unwrap().resolve().unwrap().unwrap(), "Params").unwrap());
        assert_eq!(get(&params, "Size").unwrap().as_int().unwrap() as usize, before[0].len());
        assert!(get(&params, "ModDate").unwrap().as_string().unwrap().starts_with("D:"));
        let ci = resolved(&get(&spec, "CI").unwrap());
        assert_eq!(get(&ci, "Name").unwrap().as_string().unwrap(), "a.pdf");
        assert_eq!(get(&ci, "Description").unwrap().as_string().unwrap(), "");
        assert_eq!(get(&ci, "Size").unwrap().as_int().unwrap() as usize, before[0].len());
        assert!(get(&ci, "Modified").unwrap().as_string().unwrap().starts_with("D:"));
        let coll = collection(d)?.unwrap();
        assert_eq!(get(&coll, "View").and_then(|v| name_of(&v)).as_deref(), Some("D"));
        let sort = resolved(&get(&coll, "Sort").unwrap());
        assert_eq!(get(&sort, "S").and_then(|v| name_of(&v)).as_deref(), Some("Name"));
        Ok(())
    })
    .unwrap();
    let read_back = |key: &str| e2.read_custom(id, |d| attach::read_data(d, &format!("n:{key}"))).unwrap().1;
    assert_eq!(read_back("a.pdf"), before[0]);
    assert_eq!(read_back("b.pdf"), before[1]);
    assert_eq!(read_back("notes.txt"), b"line one\nline two\n");
    assert_eq!(read_back("notes (2).txt"), b"second\n");
    // the sources are as they were
    assert_eq!(std::fs::read(&a).unwrap(), before[0]);
    assert_eq!(std::fs::read(&b).unwrap(), before[1]);
    assert_eq!(std::fs::read(&c).unwrap(), b"line one\nline two\n");
}

#[test]
fn create_refuses_what_it_should() {
    let d = dir("create-refuse");
    let a = pdf(&d, "a.pdf", 1);
    let eng = Engine::new(2, 64 << 20);
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    assert!(create(&eng, &[], &out).is_err());
    let many: Vec<Source> = (0..=super::make::CREATE_MAX_FILES).map(|_| Source { name: "a.pdf".into(), path: a.clone() }).collect();
    assert!(create(&eng, &many, &out).unwrap_err().to_string().contains("500"));
    // writing over one of its own sources
    let e = create(&eng, &sources(&[("a.pdf", &a)]), &a).unwrap_err().to_string();
    assert!(e.contains("being written"), "{e}");
    // ... spelled in another case on a disk that does not tell them apart
    let upper = d.join("A.PDF");
    if upper.exists() {
        let e = create(&eng, &sources(&[("a.pdf", &a)]), upper.to_str().unwrap()).unwrap_err().to_string();
        assert!(e.contains("being written"), "{e}");
    }
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&a).unwrap());
    // a folder is not a file
    assert!(create(&eng, &sources(&[("d", d.to_str().unwrap())]), &out).is_err());
    assert!(!Path::new(&out).exists());
}

// ---- extract -------------------------------------------------------------------------------------

fn hostile() -> PdfDocument {
    let mut doc = PdfDocument::new();
    let long = format!("{}.pdf", "n".repeat(300));
    let names = ["../../x.exe", "CON", "a\u{7}b\u{202E}.txt", long.as_str(), "..\\..\\x.exe"];
    for (i, n) in names.iter().enumerate() {
        attach::add_one(&mut doc, &file(&format!("f{i}.bin"), format!("data{i}").as_bytes())).unwrap();
        let mut spec = spec_of(&doc, &format!("f{i}.bin"));
        put_str(&mut spec, "UF", n).unwrap();
        put_str(&mut spec, "F", n).unwrap();
    }
    doc
}

#[test]
fn hostile_names_are_cleaned_and_nothing_is_overwritten() {
    let d = dir("extract");
    let out = d.join("out");
    std::fs::create_dir_all(&out).unwrap();
    // a file already there stays as it is
    std::fs::write(out.join("x.exe"), "mine").unwrap();
    let doc = hostile();

    let r = extract_all(&doc, &out, &mut |_| {}).unwrap();
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    let long = format!("{}.pdf", "n".repeat(196));
    assert_eq!(r.written, ["x (2).exe", "_CON", "ab.txt", long.as_str(), "x (3).exe"]);
    assert_eq!(long.chars().count(), 200);
    assert_eq!(std::fs::read_to_string(out.join("x.exe")).unwrap(), "mine");
    assert_eq!(std::fs::read(out.join("x (2).exe")).unwrap(), b"data0");
    // nothing escaped the folder
    assert!(!d.join("x.exe").exists() && !d.parent().unwrap().join("x.exe").exists());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 6);

    // and again: every file gets another number, none is replaced
    let again = extract_all(&doc, &out, &mut |_| {}).unwrap();
    assert_eq!(again.written.len(), 5);
    assert!(again.written.iter().all(|n| !r.written.contains(n)), "{:?}", again.written);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 11);
    assert_eq!(std::fs::read(out.join("x (2).exe")).unwrap(), b"data0");
    assert!(extract_all(&doc, &d.join("nope"), &mut |_| {}).is_err());
}

#[test]
fn extract_leaves_the_document_as_it_was() {
    let d = dir("extract-same");
    let a = pdf(&d, "a.pdf", 1);
    let made = d.join("p.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    create(&eng, &sources(&[("a.pdf", &a)]), &made).unwrap();
    let before = std::fs::read(&made).unwrap();
    let (e2, id) = open(&made);
    let out = d.join("files");
    std::fs::create_dir_all(&out).unwrap();
    let r = e2.read_custom(id, |doc| extract_all(doc, &out, &mut |_| {})).unwrap();
    assert_eq!(r.written, ["a.pdf"]);
    assert_eq!(std::fs::read(out.join("a.pdf")).unwrap(), std::fs::read(&a).unwrap());
    assert_eq!(std::fs::read(&made).unwrap(), before);
    assert!(!e2.edit_state(id).unwrap().dirty);
}

#[test]
fn extract_names_fit_every_file_system_and_each_file_is_reported_as_it_lands() {
    // 300 astral characters are 600 UTF-16 units; 200 CJK characters are 600 UTF-8 bytes
    let astral = format!("{}.pdf", "\u{1D538}".repeat(300));
    let cjk = format!("{}.pdf", "\u{6587}".repeat(200));
    for n in [&astral, &cjk] {
        let name = extract_name(n);
        assert!(name.ends_with(".pdf"), "{name}");
        assert!(name.encode_utf16().count() <= 255 && name.len() <= 255, "{} units, {} bytes", name.encode_utf16().count(), name.len());
    }
    // a clash on such a name still gets its own, in the limit
    let d = dir("extract-long");
    let mut next = HashMap::new();
    let a = super::make::write_new_for_test(&d, &extract_name(&cjk), b"1", &mut next);
    let b = super::make::write_new_for_test(&d, &extract_name(&cjk), b"2", &mut next);
    assert_ne!(a, b);
    assert!(b.len() <= 255 && b.ends_with(" (2).pdf"), "{b}");

    let mut doc = PdfDocument::new();
    for (i, n) in [&astral, &cjk, &"p.txt".to_string()].iter().enumerate() {
        attach::add_one(&mut doc, &file(&format!("f{i}.bin"), b"x")).unwrap();
        put_str(&mut spec_of(&doc, &format!("f{i}.bin")), "UF", n).unwrap();
    }
    // two names for one stream: it is decoded once, the second is said to be left out
    let tree_now = tree(&doc).unwrap();
    let shared = tree_now[0].spec.clone();
    let mut entries = vec![Entry { key: "one.txt".into(), spec: shared.clone() }, Entry { key: "two.txt".into(), spec: shared }];
    entries.extend(tree_now.into_iter().skip(1));
    attach::write_tree(&mut doc, entries).unwrap();
    let out = d.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let mut seen = Vec::new();
    let r = extract_all(&doc, &out, &mut |p| seen.push(p.to_path_buf())).unwrap();
    assert_eq!(r.written.len(), 3, "{r:?}");
    assert_eq!(r.skipped.len(), 1);
    assert!(r.skipped[0].contains("same data"), "{:?}", r.skipped);
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|p| p.exists()));
}

#[cfg(unix)]
#[test]
fn a_folder_that_refuses_a_file_leaves_it_out_and_does_not_stop_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("extract-readonly");
    let out = d.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = std::fs::write(out.join("probe"), b"");
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("a.txt", b"x")).unwrap();
    attach::add_one(&mut doc, &file("b.txt", b"y")).unwrap();
    let mut seen = 0;
    let r = extract_all(&doc, &out, &mut |_| seen += 1);
    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).unwrap();
    if probe.is_ok() {
        return; // running as a user the permission bits do not stop (root)
    }
    let r = r.unwrap();
    assert!(r.written.is_empty() && seen == 0);
    assert_eq!(r.skipped.len(), 2, "{:?}", r.skipped);
}

// ---- hostile structure -------------------------------------------------------------------------------

/// A name tree whose /Kids point back at the node itself, many times over: nine levels of that is
/// K^9 visits unless a node is read once.
#[test]
fn a_name_tree_whose_kids_loop_is_read_once() {
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("a.txt", b"x")).unwrap();
    let mut root = doc.new_dict().unwrap();
    let root_ref = doc.add_object(&root).unwrap();
    let mut kids = doc.new_array().unwrap();
    for _ in 0..40 {
        kids.array_push(root_ref.clone()).unwrap();
    }
    let mut r = resolved(&root_ref);
    r.dict_put("Kids", kids).unwrap();
    let mut names = doc.new_dict().unwrap();
    names.dict_put("EmbeddedFiles", root_ref).unwrap();
    doc.catalog().unwrap().dict_put("Names", names).unwrap();
    let _ = &mut root;
    let t = std::time::Instant::now();
    assert!(tree(&doc).unwrap().is_empty());
    assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(attach::list(&doc).unwrap().is_empty());
    set_collection(&mut doc, &[], &[], None, None);
    assert!(read(&doc).unwrap().unwrap().files.is_empty());
}

/// Thousands of names for one file specification whose 64 fields and description are one huge string.
#[test]
fn one_huge_string_shared_by_many_fields_and_files_is_read_once_and_cut() {
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("a.txt", b"x")).unwrap();
    let big = doc.add_object(&PdfObject::new_string(&"abc 123 ".repeat(1 << 19)).unwrap()).unwrap();
    let mut spec = spec_of(&doc, "a.txt");
    spec.dict_put("Desc", big.clone()).unwrap();
    let mut ci = dict(&mut doc, "CollectionItem");
    let keys: Vec<String> = (0..64).map(|i| format!("f{i}")).collect();
    for k in &keys {
        ci.dict_put(k.as_str(), big.clone()).unwrap();
    }
    spec.dict_put("CI", ci).unwrap();
    let mut fields: Vec<FieldSpec> = vec![("Name", "F", "Name", 0, true)];
    let leaked: Vec<&'static str> = keys.iter().map(|k| &*Box::leak(k.clone().into_boxed_str())).collect();
    fields.extend(leaked.iter().take(63).enumerate().map(|(i, k)| (*k, "S", *k, i as i32 + 1, true)));
    set_collection(&mut doc, &fields, &[("f0", true)], None, None);
    let shared = tree(&doc).unwrap().remove(0).spec;
    let entries: Vec<Entry> = (0..3000).map(|i| Entry { key: format!("n{i:05}.txt"), spec: shared.clone() }).collect();
    attach::write_tree(&mut doc, entries).unwrap();

    let t = std::time::Instant::now();
    let info = read(&doc).unwrap().unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:?}", t.elapsed());
    assert_eq!(info.files.len(), 3000);
    assert!(info.files[0].cells.iter().all(|c| c.chars().count() <= 500));
    assert!(info.files[0].description.chars().count() <= 500);
    // and the listing of the attachments is as bounded
    let t = std::time::Instant::now();
    let all = attach::list(&doc).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:?}", t.elapsed());
    assert!(all[0].description.len() <= 2048);
}

/// Thousands of separate file specifications that share one /CI whose 62 values are big direct strings,
/// and one stream: the decode must happen once per shared object, not once per file.
#[test]
fn many_filespecs_sharing_one_ci_of_big_direct_strings_are_read_in_time() {
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &NewFile { name: "a.txt".into(), data: b"x".to_vec(), description: String::new(), modified: Some(1_700_000_000) }).unwrap();
    let base = spec_of(&doc, "a.txt");
    let big = "abc 123 ".repeat(1 << 15);
    let mut ci = dict(&mut doc, "CollectionItem");
    let keys: Vec<&'static str> = (0..62).map(|i| &*Box::leak(format!("f{i}").into_boxed_str())).collect();
    for k in &keys {
        ci.dict_put(*k, PdfObject::new_string(&big).unwrap()).unwrap();
    }
    ci.dict_put("M", PdfObject::new_string(&"D:20240102030405Z".to_string()).unwrap()).unwrap();
    let ci = doc.add_object(&ci).unwrap();
    let mut fields: Vec<FieldSpec> = vec![("Name", "F", "Name", 0, true), ("M", "ModDate", "Modified", 1, true)];
    fields.extend(keys.iter().enumerate().map(|(i, k)| (*k, "S", *k, i as i32 + 2, true)));
    set_collection(&mut doc, &fields, &[("f0", true)], None, None);
    let entries: Vec<Entry> = (0..5000)
        .map(|i| {
            let mut spec = doc.new_dict().unwrap();
            put_name(&mut spec, "Type", "Filespec").unwrap();
            put_str(&mut spec, "F", &format!("n{i:05}.txt")).unwrap();
            spec.dict_put("EF", get(&base, "EF").unwrap()).unwrap();
            spec.dict_put("CI", ci.clone()).unwrap();
            Entry { key: format!("n{i:05}.txt"), spec: doc.add_object(&spec).unwrap() }
        })
        .collect();
    attach::write_tree(&mut doc, entries).unwrap();

    let t = std::time::Instant::now();
    let info = read(&doc).unwrap().unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:?}", t.elapsed());
    assert_eq!(info.files.len(), 5000);
    assert_eq!(info.files[0].cells[1], "2023-11-14 22:13");
    assert!(info.files[0].cells[2].chars().count() <= 500);
    assert_eq!(info.files[0].cells[0], "n00000.txt");
    assert_eq!(info.files[4999].cells[0], "n04999.txt");
}

#[test]
fn two_entries_with_one_key_have_two_ids() {
    let mut doc = PdfDocument::new();
    attach::add_one(&mut doc, &file("a.txt", b"first")).unwrap();
    attach::add_one(&mut doc, &file("b.txt", b"second")).unwrap();
    rename_keys(&mut doc, &[("a.txt", "dup.txt"), ("b.txt", "dup.txt")]);
    set_collection(&mut doc, &[], &[], None, None);
    let ids: Vec<String> = attach::list(&doc).unwrap().into_iter().map(|a| a.id).collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(ids[0], "n:dup.txt");
    let got: Vec<Vec<u8>> = ids.iter().map(|i| attach::read_data(&doc, i).unwrap().1).collect();
    assert_eq!(got, [b"first".to_vec(), b"second".to_vec()]);
    let info = read(&doc).unwrap().unwrap();
    let mut from_view: Vec<String> = info.files.iter().map(|f| f.id.clone()).collect();
    from_view.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(from_view, want);
    // removing the second takes only the second
    attach::remove(&mut doc, &[ids[1].clone()]).unwrap();
    assert_eq!(attach::list(&doc).unwrap().len(), 1);
    assert_eq!(attach::read_data(&doc, "n:dup.txt").unwrap().1, b"first");
}

// ---- editing a portfolio ---------------------------------------------------------------------------

#[test]
fn adding_describing_and_removing_keep_the_collection_in_step() {
    let d = dir("edit");
    let a = pdf(&d, "a.pdf", 1);
    let made = d.join("p.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    create(&eng, &sources(&[("a.pdf", &a)]), &made).unwrap();
    let (e2, id) = open(&made);
    let user = || EditSource::User;

    let table = |e: &Engine| crate::organize::portfolio_info(e, id).unwrap().unwrap().files.iter().map(|f| f.cells.clone()).collect::<Vec<_>>();
    let start = table(&e2);
    crate::organize::attach_add(&e2, id, vec![NewFile { name: "extra.txt".into(), data: b"hello".to_vec(), description: String::new(), modified: Some(1_700_000_000) }], user()).unwrap();
    let info = crate::organize::portfolio_info(&e2, id).unwrap().unwrap();
    let extra = info.files.iter().find(|f| f.name == "extra.txt").unwrap();
    assert_eq!(extra.cells, ["extra.txt", "", "5 B", "2023-11-14 22:13"]);
    let added = table(&e2);

    crate::organize::attach_describe(&e2, id, "n:extra.txt", "Hello file", user()).unwrap();
    let info = crate::organize::portfolio_info(&e2, id).unwrap().unwrap();
    assert_eq!(info.files.iter().find(|f| f.name == "extra.txt").unwrap().cells[1], "Hello file");
    let described = table(&e2);

    // the initial document goes: /D no longer names a file that is not there
    crate::organize::attach_remove(&e2, id, &["n:a.pdf".to_string()], user()).unwrap();
    let info = crate::organize::portfolio_info(&e2, id).unwrap().unwrap();
    assert_eq!(info.initial, None);
    assert_eq!(info.files.len(), 1);

    // one undo step each, and the saved file reads back the same
    let saved = e2.save(id, None).unwrap().path;
    let (e3, i3) = open(&saved);
    let again = crate::organize::portfolio_info(&e3, i3).unwrap().unwrap();
    assert_eq!(again.files.iter().map(|f| f.cells.clone()).collect::<Vec<_>>(), info.files.iter().map(|f| f.cells.clone()).collect::<Vec<_>>());

    // each edit is one undo step: undoing takes back that edit and only that one
    e2.undo(id).unwrap();
    let back = crate::organize::portfolio_info(&e2, id).unwrap().unwrap();
    assert_eq!(table(&e2), described);
    assert_eq!(back.initial.as_deref(), Some("n:a.pdf"), "the initial document is back with the file");
    e2.undo(id).unwrap();
    assert_eq!(table(&e2), added);
    e2.undo(id).unwrap();
    assert_eq!(table(&e2), start);
}
