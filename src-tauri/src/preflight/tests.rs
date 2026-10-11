//! Generated PDFs: a pass and a fail per rule family, conversions through the edit lane and to a new
//! file that survive save and reopen, the report files, cancel, and a 1000-page run.

use super::*;
use crate::engine::edit::EditSource;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use mupdf::TextPageFlags;

// ---- a tiny PDF writer ---------------------------------------------------------------------------
//
// Object numbers: 1 catalog, 2 pages, 3 font /F1, 4 info, then page i at 10 + 2i with its content
// at 11 + 2i; tests number their own objects from 100.

struct Spec {
    cat: String,
    info: Option<String>,
    font: String,
    header: Vec<u8>,
    trailer: String,
    /// Extra entries of every page's /Resources dictionary (`/ExtGState <<...>>`).
    resources: String,
    /// `(extra page entries, content)` per page.
    pages: Vec<(String, String)>,
    objs: Vec<(usize, Vec<u8>)>,
}

const TEXT: &str = "0 g BT /F1 12 Tf 72 700 Td (Hello preflight world) Tj ET";

impl Default for Spec {
    fn default() -> Self {
        Spec {
            cat: String::new(),
            info: None,
            font: "<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>".into(),
            header: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            trailer: String::new(),
            resources: String::new(),
            pages: vec![(String::new(), TEXT.into())],
            objs: Vec::new(),
        }
    }
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<<{dict} /Length {}>>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

fn s(x: &str) -> Vec<u8> {
    x.as_bytes().to_vec()
}

fn page_ref(i: usize) -> usize {
    10 + 2 * i
}

fn build(sp: &Spec) -> Vec<u8> {
    let mut objs: Vec<(usize, Vec<u8>)> = vec![(1, s(&format!("<</Type/Catalog /Pages 2 0 R {}>>", sp.cat)))];
    let kids: String = (0..sp.pages.len()).map(|i| format!("{} 0 R ", page_ref(i))).collect();
    objs.push((2, s(&format!("<</Type/Pages /Kids [{kids}] /Count {}>>", sp.pages.len()))));
    objs.push((3, s(&sp.font)));
    if let Some(i) = &sp.info {
        objs.push((4, s(i)));
    }
    for (i, (extra, content)) in sp.pages.iter().enumerate() {
        objs.push((
            page_ref(i),
            s(&format!(
                "<</Type/Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources <</Font <</F1 3 0 R>> /XObject <</Im1 90 0 R>> {}>> /Contents {} 0 R {extra}>>",
                sp.resources,
                page_ref(i) + 1
            )),
        ));
        objs.push((page_ref(i) + 1, stream("", content.as_bytes())));
    }
    objs.push((90, stream("/Type/XObject /Subtype/Image /Width 2 /Height 2 /ColorSpace/DeviceGray /BitsPerComponent 8", b"abcd")));
    objs.extend(sp.objs.iter().cloned());
    objs.sort_by_key(|o| o.0);
    let size = objs.last().map_or(1, |o| o.0 + 1);
    let mut out = sp.header.clone();
    let mut offsets = vec![None; size];
    for (n, body) in &objs {
        offsets[*n] = Some(out.len());
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let mut t = format!("xref\n0 {size}\n");
    for o in &offsets {
        match o {
            Some(o) => {
                let _ = writeln!(t, "{o:010} 00000 n ");
            }
            None => t.push_str("0000000000 65535 f \n"),
        }
    }
    let info = if sp.info.is_some() { " /Info 4 0 R" } else { "" };
    let _ = write!(t, "trailer\n<</Size {size} /Root 1 0 R{info} {}>>\nstartxref\n{xref}\n%%EOF\n", sp.trailer);
    out.extend_from_slice(t.as_bytes());
    out
}

fn report_of(p: Profile, sp: &Spec) -> Report {
    let bytes = build(sp);
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    check(&doc, &bytes[..bytes.len().min(512)], p, &mut |_| {}, &AtomicBool::new(false)).unwrap()
}

fn status(p: Profile, sp: &Spec, rule: Rule) -> Status {
    report_of(p, sp).status(rule.id()).unwrap_or_else(|| panic!("{rule:?} is not in {p:?}"))
}

const XMP_ID: &str = "<?xpacket begin=\"x\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"2\" pdfaid:conformance=\"B\"/></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>";

fn with_xmp(xml: &str) -> Spec {
    Spec { cat: "/Metadata 100 0 R".into(), objs: vec![(100, stream("/Type/Metadata /Subtype/XML", xml.as_bytes()))], ..Spec::default() }
}

fn link(extra: &str) -> Vec<u8> {
    s(&format!("<</Type/Annot /Subtype/Link /Rect [10 10 100 40] /F 4 {extra}>>"))
}

// ---- the table -----------------------------------------------------------------------------------

#[test]
fn rules_are_listed_once_and_profiles_pick_their_own() {
    let mut ids: Vec<&str> = Rule::ALL.iter().map(|r| r.id()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), Rule::ALL.len());
    let count = |p: Profile| Rule::ALL.iter().filter(|r| r.applies(p)).count();
    assert!(count(Profile::Pdfa1b) > count(Profile::Pdfa2b) - 5);
    assert!(Rule::FontsUnicode.applies(Profile::Pdfa2u));
    assert!(!Rule::FontsUnicode.applies(Profile::Pdfa2b));
    assert!(Rule::Transparency.applies(Profile::Pdfa1b) && !Rule::Transparency.applies(Profile::Pdfa2b));
    assert!(Rule::PageBoxes.applies(Profile::Pdfx4) && !Rule::PageBoxes.applies(Profile::Pdfa2b));
    assert_eq!(claimed_profile("PDF/A-2b"), Some(Profile::Pdfa2b));
    assert_eq!(claimed_profile("pdf/ua-1"), Some(Profile::Pdfua1));
    assert_eq!(claimed_profile("PDF/A-4"), None);
}

// ---- rules: one pass and one fail ----------------------------------------------------------------

#[test]
fn file_header_and_identifier() {
    let ok = Spec::default();
    assert_eq!(status(Profile::Pdfa2b, &ok, Rule::FileHeader), Status::Passed);
    let no_binary = Spec { header: b"%PDF-1.7\n".to_vec(), ..Spec::default() };
    assert_eq!(status(Profile::Pdfa2b, &no_binary, Rule::FileHeader), Status::Failed);
    let v2 = Spec { header: b"%PDF-2.0\n%\xE2\xE3\xCF\xD3\n".to_vec(), ..Spec::default() };
    assert_eq!(status(Profile::Pdfa2b, &v2, Rule::FileHeader), Status::Failed);
    assert_eq!(status(Profile::Pdfa2b, &ok, Rule::TrailerId), Status::Failed);
    let id = Spec { trailer: "/ID [<00112233445566778899AABBCCDDEEFF><00112233445566778899AABBCCDDEEFF>]".into(), ..Spec::default() };
    assert_eq!(status(Profile::Pdfa2b, &id, Rule::TrailerId), Status::Passed);
}

#[test]
fn xmp_rules() {
    let none = Spec::default();
    assert_eq!(status(Profile::Pdfa2b, &none, Rule::XmpPresent), Status::Failed);
    let ok = with_xmp(XMP_ID);
    assert_eq!(status(Profile::Pdfa2b, &ok, Rule::XmpPresent), Status::Passed);
    assert_eq!(status(Profile::Pdfa2b, &ok, Rule::XmpId), Status::Passed);
    // The same packet claims part 2: it is not PDF/A-1b, nor 3b.
    assert_eq!(status(Profile::Pdfa1b, &ok, Rule::XmpId), Status::Failed);
    assert_eq!(status(Profile::Pdfa3b, &ok, Rule::XmpId), Status::Failed);
    // A Title in the document information the XMP does not carry.
    let titled = Spec { info: Some("<</Title (Annual report)>>".into()), ..with_xmp(XMP_ID) };
    assert_eq!(status(Profile::Pdfa2b, &titled, Rule::XmpSync), Status::Failed);
    // A compressed packet.
    let flate = Spec { objs: vec![(100, stream("/Type/Metadata /Subtype/XML /Filter/ASCIIHexDecode", b"3C783E3C2F783E3E"))], cat: "/Metadata 100 0 R".into(), ..Spec::default() };
    assert_eq!(status(Profile::Pdfa2b, &flate, Rule::XmpUnfiltered), Status::Failed);
}

#[test]
fn fonts_embedded_blocked_and_builtin() {
    let r = report_of(Profile::Pdfa2b, &Spec::default());
    let f = r.result("fonts-embedded").unwrap();
    assert_eq!(f.status, Status::Failed);
    assert!(f.findings[0].fixable, "Helvetica has a built-in clone");
    assert!(f.findings[0].message.contains("Helvetica"));
    assert_eq!(f.findings[0].page, Some(0));
    let blocked = Spec { font: "<</Type/Font/Subtype/Type1/BaseFont/QuiteUnknownTypeface/Encoding/WinAnsiEncoding>>".into(), ..Spec::default() };
    let r = report_of(Profile::Pdfa2b, &blocked);
    let f = r.result("fonts-embedded").unwrap();
    assert!(!f.findings[0].fixable);
    assert_eq!(r.plan.remaining.len(), 1);
    assert_eq!(r.plan.remaining[0].text, "fonts that aren't installed");
    assert_eq!(r.plan.remaining[0].count, 1);
    assert!(r.plan.groups.iter().find(|g| g.id == FixGroup::Fonts).is_none() || r.plan.groups.iter().find(|g| g.id == FixGroup::Fonts).unwrap().disabled.is_some());
    // An embedded font passes.
    let embedded = Spec {
        font: "<</Type/Font/Subtype/TrueType/BaseFont/Embedded/FontDescriptor 101 0 R>>".into(),
        objs: vec![(101, s("<</Type/FontDescriptor/FontName/Embedded/FontFile2 102 0 R>>")), (102, stream("", b"not a real font"))],
        ..Spec::default()
    };
    assert_eq!(status(Profile::Pdfa2b, &embedded, Rule::FontsEmbedded), Status::Passed);
}

#[test]
fn colour_and_output_intent() {
    let gray = Spec::default();
    let r = report_of(Profile::Pdfa2b, &gray);
    assert_eq!(r.status("output-intent"), Some(Status::Failed), "DeviceGray with no intent");
    assert!(r.result("output-intent").unwrap().findings[0].fixable);
    let cmyk = Spec { pages: vec![(String::new(), "0 0 0 1 k 0 0 10 10 re f".into())], ..Spec::default() };
    let r = report_of(Profile::Pdfa2b, &cmyk);
    let f = &r.result("output-intent").unwrap().findings[0];
    assert!(!f.fixable);
    assert!(f.note.as_deref().unwrap().contains("CMYK"));
    // A Default colour space covers the device colour.
    let covered = Spec { resources: "/ColorSpace <</DefaultGray [/ICCBased 100 0 R]>>".into(), objs: vec![(100, stream("/N 1", b"x"))], ..Spec::default() };
    assert_eq!(status(Profile::Pdfa2b, &covered, Rule::OutputIntent), Status::Passed);
    // PDF/X-4 lists RGB as information only.
    let rgb = Spec { pages: vec![(String::new(), "1 0 0 rg 0 0 10 10 re f".into())], ..Spec::default() };
    assert_eq!(status(Profile::Pdfx4, &rgb, Rule::RgbContent), Status::Info);
    assert_eq!(status(Profile::Pdfx4, &rgb, Rule::OutputIntent), Status::Failed);
}

#[test]
fn actions_annotations_and_files() {
    let js = Spec {
        pages: vec![("/Annots [120 0 R 121 0 R]".into(), TEXT.into())],
        objs: vec![
            (120, link("/A <</S/JavaScript /JS (app.alert(1))>>")),
            (121, link("/A <</S/URI /URI (https://example.com)>>")),
        ],
        cat: "/OpenAction <</S/Launch /F (calc.exe)>> /Names <</JavaScript <</Names [(a) 122 0 R]>>>>".into(),
        ..Spec::default()
    };
    let r = report_of(Profile::Pdfa2b, &js);
    let a = r.result("actions-forbidden").unwrap();
    assert_eq!(a.status, Status::Failed);
    assert_eq!(a.count, 2, "the JavaScript link and the launch open action; a URI link is allowed: {:?}", a.findings);
    assert!(a.findings.iter().any(|f| f.page == Some(0) && f.xref == Some(120)));
    assert_eq!(r.status("javascript"), Some(Status::Failed));
    // Annotation types and flags.
    let an = Spec {
        pages: vec![("/Annots [120 0 R 121 0 R]".into(), TEXT.into())],
        objs: vec![
            (120, s("<</Type/Annot /Subtype/Sound /Rect [10 10 20 20] /F 4>>")),
            (121, s("<</Type/Annot /Subtype/Text /Rect [10 50 20 60] /Contents (hi) /AP <</N 90 0 R>>>>")),
        ],
        ..Spec::default()
    };
    let r = report_of(Profile::Pdfa2b, &an);
    assert_eq!(r.status("annotation-types"), Some(Status::Failed));
    assert_eq!(r.status("annotation-flags"), Some(Status::Failed), "the text annotation doesn't print");
    // Embedded files by family.
    let ef = Spec {
        cat: "/Names <</EmbeddedFiles <</Names [(a.txt) 130 0 R]>>>>".into(),
        objs: vec![(130, s("<</Type/Filespec /F (a.txt) /UF (a.txt) /EF <</F 131 0 R>>>>")), (131, stream("/Type/EmbeddedFile", b"hello"))],
        ..Spec::default()
    };
    assert_eq!(status(Profile::Pdfa1b, &ef, Rule::EmbeddedFiles), Status::Failed);
    assert_eq!(status(Profile::Pdfa2b, &ef, Rule::EmbeddedFiles), Status::Failed, "a text file isn't a PDF/A file");
    assert_eq!(status(Profile::Pdfa3b, &ef, Rule::EmbeddedFiles), Status::Failed, "no AFRelationship");
}

#[test]
fn transparency_is_pdfa1_only_and_lzw_is_found() {
    let t = Spec {
        resources: "/ExtGState <</G1 <</ca 0.5 /CA 0.5>>>>".into(),
        pages: vec![(String::new(), "/G1 gs 0 g 0 0 50 50 re f".into())],
        ..Spec::default()
    };
    assert_eq!(status(Profile::Pdfa1b, &t, Rule::Transparency), Status::Failed);
    assert!(report_of(Profile::Pdfa2b, &t).result("transparency").is_none());
    let lzw = Spec { objs: vec![(140, stream("/Filter/LZWDecode", &[0x80, 0x18, 0x60, 0x20]))], ..Spec::default() };
    let r = report_of(Profile::Pdfa2b, &lzw);
    assert_eq!(r.status("lzw"), Some(Status::Failed));
    assert_eq!(r.result("lzw").unwrap().findings[0].xref, Some(140));
}

#[test]
fn pdfa1_objects_streams_are_new_file_issues() {
    let r = report_of(Profile::Pdfa1b, &Spec { header: b"%PDF-1.7\n".to_vec(), ..Spec::default() });
    let h = &r.result("file-header").unwrap().findings[0];
    assert!(h.fixable && h.new_file);
    let g = r.plan.groups.iter().find(|g| g.id == FixGroup::Structure).unwrap();
    assert!(g.new_file_issues >= 1 && g.issues >= g.new_file_issues);
}

#[test]
fn ua_profile_reuses_the_accessibility_checker() {
    let r = report_of(Profile::Pdfua1, &Spec::default());
    assert_eq!(r.profile, Profile::Pdfua1);
    assert_eq!(r.status("title"), Some(Status::Failed));
    assert_eq!(r.status("tagged-pdf"), Some(Status::Failed));
    assert_eq!(r.status("xmp-present"), Some(Status::Failed), "no metadata, so no pdfuaid");
    let title = r.result("title").unwrap();
    assert_eq!(title.a11y_fix, Some("title"));
    assert_eq!(title.fix, Some(FixGroup::Accessibility));
    assert!(!r.conforms);
}

// ---- conversions ---------------------------------------------------------------------------------

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("preflight-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn file(test: &str, sp: &Spec) -> String {
    let p = dir(test).join("Quarterly figures.pdf");
    std::fs::write(&p, build(sp)).unwrap();
    p.to_string_lossy().into_owned()
}

fn open(path: &str) -> (Arc<Engine>, DocId) {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(path).unwrap().doc;
    (eng, id)
}

fn run(eng: &Engine, id: DocId, p: Profile) -> Report {
    check_doc(eng, id, p, &mut |_| {}, &AtomicBool::new(false)).unwrap()
}

/// What the rail ticks by default.
fn ticked(r: &Report) -> Convert {
    Convert { groups: r.plan.groups.iter().filter(|g| g.default_on && g.disabled.is_none()).map(|g| g.id).collect(), lang: None }
}

fn messy() -> Spec {
    Spec {
        info: Some("<</Title (Quarterly figures) /Author (Kim) /CreationDate (D:20260101120000Z)>>".into()),
        pages: vec![("/Annots [120 0 R 121 0 R]".into(), "1 0 0 rg 0 g BT /F1 12 Tf 72 700 Td (Hello) Tj ET".into())],
        objs: vec![
            (120, link("/A <</S/JavaScript /JS (app.alert(1))>>")),
            (121, s("<</Type/Annot /Subtype/Text /Rect [10 50 20 60] /Contents (note) /AP <</N 90 0 R>>>>")),
        ],
        cat: "/Names <</JavaScript <</Names [(a) 122 0 R]>>>>".into(),
        ..Spec::default()
    }
}

#[test]
fn convert_in_place_is_one_undo_step_and_survives_save() {
    let path = file("in-place", &messy());
    let (eng, id) = open(&path);
    let before = run(&eng, id, Profile::Pdfa2b);
    assert!(!before.conforms);
    let c = ticked(&before);
    assert!(c.groups.contains(&FixGroup::Fonts) && c.groups.contains(&FixGroup::Metadata) && c.groups.contains(&FixGroup::Actions));
    let steps = eng.history(id).unwrap().len();
    convert_in_place(&eng, id, Profile::Pdfa2b, &c, EditSource::User, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1, "one undo step");
    let after = run(&eng, id, Profile::Pdfa2b);
    let failed: Vec<_> = after.results.iter().filter(|r| r.status == Status::Failed).map(|r| (r.rule, r.findings.iter().map(|f| f.message.clone()).collect::<Vec<_>>())).collect();
    assert!(after.conforms, "left over: {failed:?}");
    assert_eq!(claim_doc(&eng, id).unwrap().unwrap().label, "PDF/A-2b");
    eng.undo(id).unwrap();
    assert!(!run(&eng, id, Profile::Pdfa2b).conforms, "undo restores the failures");
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng2, id2) = open(&path);
    let reread = run(&eng2, id2, Profile::Pdfa2b);
    let failed: Vec<_> = reread.results.iter().filter(|r| r.status == Status::Failed).map(|r| (r.rule, r.findings.iter().map(|f| f.message.clone()).collect::<Vec<_>>())).collect();
    // A saved file is a new write: its header and identifier are the writer's.
    assert!(reread.conforms, "after save and reopen: {failed:?}");
    assert_eq!(reread.claimed.as_deref(), Some("PDF/A-2b"));
}

#[test]
fn convert_to_a_new_file_leaves_the_source_alone() {
    let path = file("new-file", &messy());
    let before = std::fs::read(&path).unwrap();
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let dest = dir("new-file").join("Quarterly figures (PDF-A).pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfa2b, &ticked(&r), dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert!(out.report.conforms, "{:?}", out.report.results.iter().filter(|r| r.status == Status::Failed).map(|r| r.rule).collect::<Vec<_>>());
    assert!(!out.fixed.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before, "the source file is untouched");
    assert!(!eng.history(id).unwrap().iter().any(|h| h.label.starts_with("Convert")), "no edit in the open document");
    // The file on disk, read fresh.
    let bytes = std::fs::read(&dest).unwrap();
    assert!(bytes.starts_with(b"%PDF-1."));
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    assert!(doc.trailer().unwrap().get_dict("ID").unwrap().is_some());
    assert!(doc.trailer().unwrap().get_dict("Encrypt").unwrap().is_none());
    let meta = doc.catalog().unwrap().get_dict("Metadata").unwrap().unwrap();
    assert!(check::filters(&meta).is_empty(), "the metadata stream is stored plain");
}

#[test]
fn a_new_file_is_refused_over_an_open_document() {
    let path = file("overwrite", &messy());
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let before = std::fs::read(&path).unwrap();
    let e = convert_to_file(&eng, id, Profile::Pdfa2b, &ticked(&r), &path, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(e.to_string().contains("open"), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_font_that_is_not_installed_is_left_and_reported() {
    let sp = Spec { font: "<</Type/Font/Subtype/Type1/BaseFont/QuiteUnknownTypeface/Encoding/WinAnsiEncoding>>".into(), ..Spec::default() };
    let path = file("blocked", &sp);
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let mut c = ticked(&r);
    c.groups.push(FixGroup::Fonts);
    let dest = dir("blocked").join("out.pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfa2b, &c, dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let f = out.report.result("fonts-embedded").unwrap();
    assert_eq!(f.status, Status::Failed, "the font is still not embedded");
    assert!(out.fixed.iter().any(|l| l.contains("QuiteUnknownTypeface")), "{:?}", out.fixed);
    let doc = PdfDocument::open(dest.to_str().unwrap()).unwrap();
    let page = doc.find_page(0).unwrap();
    let font = page.get_dict_inheritable("Resources").unwrap().unwrap().get_dict("Font").unwrap().unwrap().get_dict("F1").unwrap().unwrap();
    assert_eq!(crate::a11y::tree::name(&font, "BaseFont").unwrap().as_deref(), Some("QuiteUnknownTypeface"), "nothing was substituted");
}

#[test]
fn builtin_and_installed_fonts_are_embedded() {
    let path = file("fonts", &Spec::default());
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let dest = dir("fonts").join("out.pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfa2b, &ticked(&r), dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.status("fonts-embedded"), Some(Status::Passed));
    // The text still draws and extracts.
    let doc = PdfDocument::open(dest.to_str().unwrap()).unwrap();
    let page = doc.load_page(0).unwrap();
    let t = page.to_text_page(TextPageFlags::empty()).unwrap().to_text().unwrap();
    assert!(t.contains("Hello preflight world"), "{t:?}");
    // A font file is in the document and is the program's own name.
    let res = doc.find_page(0).unwrap().get_dict_inheritable("Resources").unwrap().unwrap();
    let f = res.get_dict("Font").unwrap().unwrap().get_dict("F1").unwrap().unwrap();
    let d = f.get_dict("FontDescriptor").unwrap().unwrap();
    assert!(d.get_dict("FontFile3").unwrap().is_some() || d.get_dict("FontFile2").unwrap().is_some());
}

#[test]
fn installed_windows_font_is_embedded_when_present() {
    let Some(_) = crate::engine::fonts::installed_file("Arial") else { return };
    let sp = Spec { font: "<</Type/Font/Subtype/TrueType/BaseFont/Arial/Encoding/WinAnsiEncoding>>".into(), ..Spec::default() };
    let path = file("arial", &sp);
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let f = r.result("fonts-embedded").unwrap();
    assert!(f.findings[0].fixable, "{:?}", f.findings);
    let dest = dir("arial").join("out.pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfa2b, &ticked(&r), dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.status("fonts-embedded"), Some(Status::Passed), "{:?}", out.fixed);
    let doc = PdfDocument::open(dest.to_str().unwrap()).unwrap();
    let t = doc.load_page(0).unwrap().to_text_page(TextPageFlags::empty()).unwrap().to_text().unwrap();
    assert!(t.contains("Hello preflight world"), "{t:?}");
}

#[test]
fn encrypted_documents_convert_to_a_new_file_only() {
    let plain = build(&messy());
    let doc = PdfDocument::from_bytes(&plain).unwrap();
    let mut o = mupdf::pdf::PdfWriteOptions::default();
    o.set_encryption(mupdf::pdf::Encryption::Aes256).set_user_password("").set_owner_password("owner");
    let mut enc = Vec::new();
    doc.write_to_with_options(&mut enc, o).unwrap();
    let p = dir("encrypted").join("Quarterly figures.pdf");
    std::fs::write(&p, &enc).unwrap();
    let (eng, id) = open(p.to_str().unwrap());
    let r = run(&eng, id, Profile::Pdfa2b);
    assert_eq!(r.status("encryption"), Some(Status::Failed));
    let e = convert_in_place(&eng, id, Profile::Pdfa2b, &ticked(&r), EditSource::User, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(e.to_string().contains("encrypted"), "{e}");
    let dest = dir("encrypted").join("out.pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfa2b, &ticked(&r), dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.status("encryption"), Some(Status::Passed));
    assert!(!PdfDocument::open(dest.to_str().unwrap()).unwrap().needs_password().unwrap());
}

#[test]
fn pdfa1_removes_transparency_only_when_the_appearance_group_is_ticked() {
    let sp = Spec {
        resources: "/ExtGState <</G1 <</ca 0.5 /CA 0.5 /TR /Identity2>>>>".into(),
        pages: vec![(String::new(), "/G1 gs 0 g BT /F1 12 Tf 72 700 Td (x) Tj ET 0 0 50 50 re f".into())],
        ..Spec::default()
    };
    let path = file("a1", &sp);
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa1b);
    let ap = r.plan.groups.iter().find(|g| g.id == FixGroup::Appearance).unwrap();
    assert!(!ap.default_on, "what changes how pages look is not ticked by default");
    let c = ticked(&r);
    assert!(!c.groups.contains(&FixGroup::Appearance));
    let dest = dir("a1").join("out.pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfa1b, &c, dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.status("transparency"), Some(Status::Failed), "left alone unless asked");
    let mut c2 = c.clone();
    c2.groups.push(FixGroup::Appearance);
    let dest2 = dir("a1").join("out2.pdf");
    let out2 = convert_to_file(&eng, id, Profile::Pdfa1b, &c2, dest2.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let left: Vec<_> = out2.report.result("transparency").unwrap().findings.iter().map(|f| f.message.clone()).collect();
    assert_eq!(out2.report.status("transparency"), Some(Status::Passed), "{left:?} {:?}", out2.fixed);
    assert!(out2.report.conforms, "{:?}", out2.report.results.iter().filter(|r| r.status == Status::Failed).map(|r| r.rule).collect::<Vec<_>>());
}

/// Some stream object of the file decodes to exactly `data`.
fn has_stream(path: &str, data: &[u8]) -> bool {
    let doc = PdfDocument::open(path).unwrap();
    (1..doc.count_objects().unwrap() as i32).any(|i| {
        let o = doc.new_indirect(i, 0).unwrap();
        o.is_stream().unwrap_or(false) && o.read_stream().map(|d| d == data).unwrap_or(false)
    })
}

#[test]
fn files_actions_and_annotations_are_removed_per_family() {
    let sp = Spec {
        pages: vec![("/Annots [120 0 R 121 0 R]".into(), TEXT.into())],
        objs: vec![
            (120, s("<</Type/Annot /Subtype/FileAttachment /Rect [10 10 20 20] /F 4 /FS 130 0 R /AP <</N 90 0 R>>>>")),
            (121, s("<</Type/Annot /Subtype/Sound /Rect [30 10 40 20] /F 4 /Popup 140 0 R>>")),
            (130, s("<</Type/Filespec /F (a.txt) /UF (a.txt) /EF <</F 131 0 R>>>>")),
            (131, stream("/Type/EmbeddedFile", b"hello")),
            (140, s("<</Type/Annot /Subtype/Popup /Parent 121 0 R /Rect [0 0 10 10]>>")),
        ],
        cat: "/Names <</EmbeddedFiles <</Names [(a.txt) 130 0 R]>>>>".into(),
        ..Spec::default()
    };
    let path = file("files", &sp);
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let dest = dir("files").join("out.pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfa2b, &ticked(&r), dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.status("embedded-files"), Some(Status::Passed));
    assert_eq!(out.report.status("annotation-types"), Some(Status::Passed));
    assert!(!has_stream(dest.to_str().unwrap(), b"hello"), "the attached file's content is gone from the file");
    // PDF/A-3 keeps the file and marks its relationship instead.
    let r3 = run(&eng, id, Profile::Pdfa3b);
    let dest3 = dir("files").join("out3.pdf");
    let out3 = convert_to_file(&eng, id, Profile::Pdfa3b, &ticked(&r3), dest3.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out3.report.status("embedded-files"), Some(Status::Passed), "{:?}", out3.fixed);
    assert!(has_stream(dest3.to_str().unwrap(), b"hello"), "PDF/A-3 keeps the file");
}

#[test]
fn pdfua_conversion_sets_title_language_and_identification() {
    let path = file("ua", &Spec::default());
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfua1);
    assert!(r.plan.needs_lang);
    let c = Convert { groups: vec![FixGroup::Accessibility, FixGroup::Metadata], lang: Some("sv-SE".into()) };
    convert_in_place(&eng, id, Profile::Pdfua1, &c, EditSource::User, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let after = run(&eng, id, Profile::Pdfua1);
    assert_eq!(after.status("title"), Some(Status::Passed));
    assert_eq!(after.status("primary-language"), Some(Status::Passed));
    assert_eq!(after.status("xmp-id"), Some(Status::Passed));
    assert_eq!(after.status("tagged-pdf"), Some(Status::Failed), "tags are not made by a conversion");
    assert_eq!(claim_doc(&eng, id).unwrap().unwrap().label, "PDF/UA-1");
}

#[test]
fn pdfx4_metadata_and_boxes_are_fixed_the_output_intent_is_not() {
    let path = file("x4", &Spec::default());
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfx4);
    assert_eq!(r.status("page-boxes"), Some(Status::Failed));
    assert_eq!(r.status("trapped"), Some(Status::Failed));
    assert!(r.plan.remaining.iter().any(|x| x.text.contains("print condition")));
    let dest = dir("x4").join("out.pdf");
    let out = convert_to_file(&eng, id, Profile::Pdfx4, &ticked(&r), dest.to_str().unwrap(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.status("page-boxes"), Some(Status::Passed));
    assert_eq!(out.report.status("trapped"), Some(Status::Passed));
    assert_eq!(out.report.status("xmp-id"), Some(Status::Passed));
    assert_eq!(out.report.status("output-intent"), Some(Status::Failed), "a print condition is the person's choice");
}

// ---- what the plan says --------------------------------------------------------------------------

#[test]
fn the_plan_counts_fixed_and_remaining() {
    let sp = Spec {
        font: "<</Type/Font/Subtype/Type1/BaseFont/QuiteUnknownTypeface/Encoding/WinAnsiEncoding>>".into(),
        pages: vec![("/Annots [120 0 R]".into(), TEXT.into())],
        objs: vec![(120, link("/A <</S/JavaScript /JS (x)>>"))],
        ..Spec::default()
    };
    let r = report_of(Profile::Pdfa2b, &sp);
    let fixable: u32 = r.plan.groups.iter().map(|g| g.issues).sum();
    let left: u32 = r.plan.remaining.iter().map(|x| x.count).sum();
    assert_eq!(fixable + left, r.plan.total, "every finding is either fixed or left, once");
    assert!(left >= 1);
    assert_eq!(r.plan.remaining.iter().find(|x| x.text == "fonts that aren't installed").map(|x| x.count), Some(1));
    assert!(r.plan.groups.iter().any(|g| g.id == FixGroup::Actions && g.issues >= 1));
}

// ---- report files --------------------------------------------------------------------------------

#[test]
fn reports_as_html_and_pdf() {
    let path = file("report", &messy());
    let (eng, id) = open(&path);
    let html = report_bytes(&eng, id, Profile::Pdfa2b, false, "2026-10-11", &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let html = String::from_utf8(html).unwrap();
    assert!(html.contains("PDF/A-2b") && html.contains("Fonts embedded") && html.contains("veraPDF"));
    assert!(html.contains("Quarterly figures.pdf"));
    let pdf = report_bytes(&eng, id, Profile::Pdfa2b, true, "2026-10-11", &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let doc = PdfDocument::from_bytes(&pdf).unwrap();
    assert!(doc.page_count().unwrap() >= 1);
    let t = doc.load_page(0).unwrap().to_text_page(TextPageFlags::empty()).unwrap().to_text().unwrap();
    assert!(t.contains("Preflight report"), "{t:?}");
}

// ---- progress, cancel, size ----------------------------------------------------------------------

#[test]
fn a_cancelled_run_stops_and_changes_nothing() {
    let path = file("cancel", &messy());
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let stop = AtomicBool::new(true);
    let steps = eng.history(id).unwrap().len();
    let e = convert_in_place(&eng, id, Profile::Pdfa2b, &ticked(&r), EditSource::User, &mut |_| {}, &stop).unwrap_err();
    assert!(e.to_string().contains("cancelled"), "{e}");
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert!(check_doc(&eng, id, Profile::Pdfa2b, &mut |_| {}, &stop).is_err());
}

fn big(pages: usize) -> Spec {
    Spec {
        pages: (0..pages).map(|i| (String::new(), format!("0 g BT /F1 12 Tf 72 700 Td (Page {i}) Tj ET"))).collect(),
        ..Spec::default()
    }
}

#[test]
fn a_thousand_pages_report_progress_and_cancel_between_pages() {
    let bytes = build(&big(1000));
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    let mut seen = Vec::new();
    let r = check(&doc, &bytes[..512], Profile::Pdfa2b, &mut |p| seen.push((p.done, p.pages)), &AtomicBool::new(false)).unwrap();
    assert_eq!(r.pages, 1000);
    assert!(seen.iter().any(|&(d, p)| d == 1000 && p == 1000), "the last page is reported");
    assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "progress only moves forward");
    // The font object is shared by every page: one finding, not a thousand.
    assert_eq!(r.result("fonts-embedded").unwrap().count, 1);
    // Cancel after 10 pages.
    let stop = AtomicBool::new(false);
    let mut n = 0;
    let e = check(&doc, &bytes[..512], Profile::Pdfa2b, &mut |_| {
        n += 1;
        if n == 10 {
            stop.store(true, Ordering::Relaxed);
        }
    }, &stop);
    assert!(e.is_err());
    assert!(n < 50, "stopped soon after cancel, saw {n} events");
}
