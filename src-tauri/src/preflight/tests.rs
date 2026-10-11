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

/// `messy()` with a signature field that has a value, and optionally a DocMDP certification of `/P` `p`.
fn protected(certify: Option<u8>) -> Spec {
    let mut sp = messy();
    sp.cat.push_str(" /AcroForm <</SigFlags 3 /Fields [130 0 R]>>");
    if let Some(p) = certify {
        sp.cat.push_str(&format!(" /Perms <</DocMDP <</Type/Sig /Reference [<</TransformMethod/DocMDP /TransformParams <</P {p}>>>>]>>>>"));
    }
    sp.objs.push((130, s("<</FT/Sig /T(Signature1) /V <</Type/Sig /Filter/Adobe.PPKLite /SubFilter/adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00>>>>>")));
    sp
}

#[test]
fn a_signed_or_certified_document_is_not_converted_in_place() {
    for (name, certify, word) in [("signed", None, "signed"), ("certified-1", Some(1), "certified"), ("certified-3", Some(3), "certified")] {
        let p = dir(name).join("Quarterly figures.pdf");
        std::fs::write(&p, build(&protected(certify))).unwrap();
        let original = std::fs::read(&p).unwrap();
        let (eng, id) = open(p.to_str().unwrap());
        let steps = eng.history(id).unwrap().len();
        let c = Convert { groups: vec![FixGroup::Metadata], lang: None };
        let e = convert_in_place(&eng, id, Profile::Pdfa2b, &c, EditSource::User, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(e.to_string().contains(word) && e.to_string().contains("new file"), "{name}: {e}");
        assert_eq!(eng.history(id).unwrap().len(), steps, "{name}: a refusal adds no step");
        let _ = eng.save(id, None);
        assert_eq!(std::fs::read(&p).unwrap(), original, "{name}: the file is as it was");
    }
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
            (131, stream("/Type/EmbeddedFile /Params <</ModDate (D:20260101000000Z)>>", b"hello")),
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
    assert_eq!(after.status("tagged-pdf"), Some(Status::Failed), "tags are not made by a conversion");
    // The file is not tagged, so it is not PDF/UA, and the XMP does not say it is.
    assert_eq!(after.status("xmp-id"), Some(Status::Failed));
    assert!(claim_doc(&eng, id).unwrap().is_none());
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
    assert_eq!(out.report.status("output-intent"), Some(Status::Failed), "a print condition is the person's choice");
    // Without the output intent the file is not PDF/X-4, and the XMP does not say so (no GTS_PDFXVersion).
    assert_eq!(out.report.status("xmp-id"), Some(Status::Failed));
    assert!(out.fixed.iter().any(|l| l.contains("Did not identify the file as PDF/X-4")), "{:?}", out.fixed);
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

// ---- the identification is written only for a file that conforms ----------------------------------

/// `fix::apply` on a copy of `sp` held in memory; `new_file` as for a conversion to a new file.
fn applied(sp: &Spec, p: Profile, groups: &[FixGroup], new_file: bool) -> (PdfDocument, fix::Applied) {
    let bytes = build(sp);
    let mut d = PdfDocument::from_bytes(&bytes).unwrap();
    let head: &[u8] = if new_file { fix::NEW_FILE_HEAD } else { &bytes[..bytes.len().min(512)] };
    let c = Convert { groups: groups.to_vec(), lang: None };
    let a = fix::apply(&mut d, p, &c, "doc", new_file, head, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    (d, a)
}

fn says(d: &PdfDocument) -> Option<String> {
    claim(d).unwrap().map(|c| c.label)
}

fn not_identified(a: &fix::Applied, label: &str) -> bool {
    a.log.iter().any(|l| l.contains(&format!("Did not identify the file as {label}")))
}

fn all_groups(p: Profile, sp: &Spec) -> Vec<FixGroup> {
    ticked(&report_of(p, sp)).groups
}

#[test]
fn metadata_alone_syncs_the_xmp_and_does_not_identify_a_failing_file() {
    // The per-rule Fix of "Standard identification" runs the Metadata group alone.
    let (d, a) = applied(&messy(), Profile::Pdfa2b, &[FixGroup::Metadata], false);
    assert_eq!(says(&d), None, "{:?}", a.log);
    assert!(not_identified(&a, "PDF/A-2b"), "{:?}", a.log);
    assert!(d.catalog().unwrap().get_dict("Metadata").unwrap().is_some(), "the XMP is still written and synced");
    // With every group ticked the same file conforms and is identified.
    let (d, a) = applied(&messy(), Profile::Pdfa2b, &all_groups(Profile::Pdfa2b, &messy()), false);
    assert_eq!(says(&d).as_deref(), Some("PDF/A-2b"), "{:?}", a.log);
    assert!(!not_identified(&a, "PDF/A-2b"));
}

#[test]
fn a1_transparency_left_in_place_leaves_the_file_unidentified() {
    let sp = Spec {
        resources: "/ExtGState <</G1 <</ca 0.5 /CA 0.5>>>>".into(),
        pages: vec![(String::new(), "/G1 gs 0 g BT /F1 12 Tf 72 700 Td (x) Tj ET 0 0 50 50 re f".into())],
        ..Spec::default()
    };
    // Appearance is unticked by default, so the transparency stays.
    let groups = all_groups(Profile::Pdfa1b, &sp);
    assert!(!groups.contains(&FixGroup::Appearance));
    let (d, a) = applied(&sp, Profile::Pdfa1b, &groups, false);
    assert_eq!(says(&d), None, "{:?}", a.log);
    assert!(not_identified(&a, "PDF/A-1b"), "{:?}", a.log);
    let mut with = groups.clone();
    with.push(FixGroup::Appearance);
    let (d, a) = applied(&sp, Profile::Pdfa1b, &with, false);
    assert_eq!(says(&d).as_deref(), Some("PDF/A-1b"), "{:?}", a.log);
}

#[test]
fn a_blocked_font_leaves_the_file_unidentified() {
    let sp = Spec { font: "<</Type/Font/Subtype/Type1/BaseFont/QuiteUnknownTypeface/Encoding/WinAnsiEncoding>>".into(), ..Spec::default() };
    let mut groups = all_groups(Profile::Pdfa2b, &sp);
    groups.push(FixGroup::Fonts);
    let (d, a) = applied(&sp, Profile::Pdfa2b, &groups, false);
    assert_eq!(says(&d), None, "{:?}", a.log);
    assert!(not_identified(&a, "PDF/A-2b"), "{:?}", a.log);
}

#[test]
fn a_skipped_new_file_only_fix_leaves_the_file_unidentified() {
    // No binary comment line after the header: only a new file's writing can repair that.
    let sp = Spec { header: b"%PDF-1.7\n".to_vec(), ..Spec::default() };
    let groups = all_groups(Profile::Pdfa2b, &sp);
    let (d, a) = applied(&sp, Profile::Pdfa2b, &groups, false);
    assert_eq!(says(&d), None, "in place the header stays wrong: {:?}", a.log);
    assert!(not_identified(&a, "PDF/A-2b"));
    let (d, a) = applied(&sp, Profile::Pdfa2b, &groups, true);
    assert_eq!(says(&d).as_deref(), Some("PDF/A-2b"), "a new file gets its header at the write: {:?}", a.log);
}

#[test]
fn pdfx4_is_identified_only_with_its_output_intent() {
    let intent = "/OutputIntents [<</Type/OutputIntent /S/GTS_PDFX /OutputConditionIdentifier (sRGB IEC61966-2.1) /RegistryName (http://www.color.org) /Info (sRGB) /DestOutputProfile 150 0 R>>]";
    let without = Spec::default();
    let (d, a) = applied(&without, Profile::Pdfx4, &all_groups(Profile::Pdfx4, &without), true);
    assert_eq!(says(&d), None, "{:?}", a.log);
    assert!(not_identified(&a, "PDF/X-4"));
    let icc = super::icc::srgb();
    let with = Spec { cat: intent.into(), objs: vec![(150, stream("/N 3 /Alternate/DeviceRGB", &icc))], ..Spec::default() };
    let (d, a) = applied(&with, Profile::Pdfx4, &all_groups(Profile::Pdfx4, &with), true);
    assert_eq!(says(&d).as_deref(), Some("PDF/X-4"), "{:?}", a.log);
}

// ---- the output profile is shared, not doubled ---------------------------------------------------

fn profile_numbers(d: &PdfDocument) -> Vec<i32> {
    let cat = d.catalog().unwrap();
    let arr = cat.get_dict("OutputIntents").unwrap().unwrap();
    arr.array_iter().unwrap().flatten().filter_map(|oi| oi.get_dict("DestOutputProfile").ok().flatten()).map(|p| p.as_indirect().unwrap_or(-1)).collect()
}

#[test]
fn a_pdfa_output_intent_shares_the_profile_of_an_existing_pdfx_one() {
    let icc = super::icc::srgb();
    let sp = Spec {
        cat: "/OutputIntents [<</Type/OutputIntent /S/GTS_PDFX /OutputConditionIdentifier (sRGB) /DestOutputProfile 150 0 R>>]".into(),
        objs: vec![(150, stream("/N 3 /Alternate/DeviceRGB", &icc))],
        ..Spec::default()
    };
    let (d, a) = applied(&sp, Profile::Pdfa2b, &[FixGroup::Colour], false);
    let nums = profile_numbers(&d);
    assert_eq!(nums.len(), 2, "{:?}", a.log);
    assert_eq!(nums[0], nums[1], "one profile object, shared: {nums:?}");
    assert_eq!(nums[0], 150);
    assert!(a.log.iter().any(|l| l.contains("shares")), "{:?}", a.log);
}

#[test]
fn a_cmyk_pdfx_output_intent_is_left_alone() {
    let sp = Spec {
        cat: "/OutputIntents [<</Type/OutputIntent /S/GTS_PDFX /OutputConditionIdentifier (CMYK) /DestOutputProfile 150 0 R>>]".into(),
        objs: vec![(150, stream("/N 4 /Alternate/DeviceCMYK", b"not a real profile"))],
        ..Spec::default()
    };
    let (d, a) = applied(&sp, Profile::Pdfa2b, &[FixGroup::Colour], false);
    assert_eq!(profile_numbers(&d), vec![150], "no second profile is added next to it");
    assert!(a.log.iter().any(|l| l.contains("one output profile only")), "{:?}", a.log);
}

// ---- fonts: subsetting scope and TrueType encodings ----------------------------------------------

#[test]
fn subsetting_is_skipped_for_forms_licences_and_fonts_the_document_brought() {
    let none = std::collections::HashSet::new();
    let plain = PdfDocument::from_bytes(&build(&Spec::default())).unwrap();
    assert_eq!(fix::no_subsetting_reason(&plain, &none, 0, false).unwrap(), None);
    assert!(fix::no_subsetting_reason(&plain, &none, 0, true).unwrap().unwrap().contains("licence"));
    assert!(fix::no_subsetting_reason(&plain, &none, 2, false).unwrap().unwrap().contains("of its own"));
    // A form's default resources hold the font the conversion embedded: fields draw with it later.
    let form = Spec { cat: "/AcroForm <</Fields [] /DR <</Font <</Helv 3 0 R>>>>>>".into(), ..Spec::default() };
    let d = PdfDocument::from_bytes(&build(&form)).unwrap();
    let ours: std::collections::HashSet<i32> = [3].into_iter().collect();
    assert!(fix::no_subsetting_reason(&d, &ours, 0, false).unwrap().unwrap().contains("default resources"));
    assert_eq!(fix::no_subsetting_reason(&d, &none, 0, false).unwrap(), None, "a DR font the conversion did not touch is not in the way");
}

#[test]
fn truetype_encodings_pdfa1_does_not_allow_are_recognised() {
    let d = PdfDocument::from_bytes(&build(&Spec::default())).unwrap();
    let font = |enc: &str, extra: &str| d.new_object_from_str(&format!("<</Type/Font/Subtype/TrueType/BaseFont/Arial {enc} {extra}>>")).unwrap();
    assert!(!fonts::truetype_encoding_unfit(&font("/Encoding/WinAnsiEncoding", "")).unwrap());
    assert!(!fonts::truetype_encoding_unfit(&font("/Encoding/MacRomanEncoding", "")).unwrap());
    assert!(fonts::truetype_encoding_unfit(&font("/Encoding<</BaseEncoding/WinAnsiEncoding/Differences[39/quotesingle]>>", "")).unwrap());
    assert!(fonts::truetype_encoding_unfit(&font("/Encoding/StandardEncoding", "")).unwrap());
    assert!(fonts::truetype_encoding_unfit(&font("", "")).unwrap(), "no Encoding at all");
    // A symbolic font is exempt.
    assert!(!fonts::truetype_encoding_unfit(&font("", "/FontDescriptor<</Type/FontDescriptor/Flags 4>>")).unwrap());
}

#[test]
fn an_installed_truetype_font_gets_the_plain_winansi_name_in_pdfa1() {
    let Some(_) = crate::engine::fonts::installed_file("Arial") else { return };
    let sp = Spec {
        font: "<</Type/Font/Subtype/TrueType/BaseFont/Arial/Encoding<</Type/Encoding/BaseEncoding/WinAnsiEncoding/Differences[39/quotesingle]>>>>".into(),
        ..Spec::default()
    };
    let (d, a) = applied(&sp, Profile::Pdfa1b, &[FixGroup::Fonts], true);
    let f = d.new_indirect(3, 0).unwrap();
    let enc = f.get_dict("Encoding").unwrap().unwrap();
    assert!(enc.is_name().unwrap(), "{:?}", a.log);
    assert_eq!(String::from_utf8_lossy(&enc.as_name().unwrap()), "WinAnsiEncoding");
    assert!(a.log.iter().any(|l| l.contains("may draw as other glyphs")), "the dropped Differences are named: {:?}", a.log);
}

// ---- PDF/A-3 embedded files ----------------------------------------------------------------------

fn a3_file(params: &str) -> Spec {
    Spec {
        cat: "/Names <</EmbeddedFiles <</Names [(a.xml) 130 0 R]>>>>".into(),
        objs: vec![
            (130, s("<</Type/Filespec /F (a.xml) /UF (a.xml) /EF <</F 131 0 R>>>>")),
            (131, stream(&format!("/Type/EmbeddedFile {params}"), b"<a/>")),
        ],
        ..Spec::default()
    }
}

#[test]
fn pdfa3_embedded_files_need_a_media_type_a_modification_date_and_a_place_in_af() {
    let sp = a3_file("/Params <</ModDate (D:20260101000000Z)>>");
    let r = report_of(Profile::Pdfa3b, &sp);
    let ef = r.result("embedded-files").unwrap();
    assert_eq!(ef.status, Status::Failed);
    let msgs: Vec<&str> = ef.findings.iter().map(|f| f.message.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("no AFRelationship")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("media type")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("/AF")), "{msgs:?}");
    // The Files group repairs all three.
    let (d, a) = applied(&sp, Profile::Pdfa3b, &[FixGroup::Files], false);
    let cat = d.catalog().unwrap();
    assert_eq!(cat.get_dict("AF").unwrap().unwrap().len().unwrap(), 1, "{:?}", a.log);
    let stream = d.new_indirect(131, 0).unwrap();
    assert!(stream.get_dict("Subtype").unwrap().is_some());
    let bytes = {
        let mut out = Vec::new();
        d.write_to_with_options(&mut out, mupdf::pdf::PdfWriteOptions::default()).unwrap();
        out
    };
    let again = PdfDocument::from_bytes(&bytes).unwrap();
    let r2 = check(&again, &bytes[..bytes.len().min(512)], Profile::Pdfa3b, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(r2.status("embedded-files"), Some(Status::Passed), "{:?}", r2.result("embedded-files").unwrap().findings);
}

#[test]
fn pdfa3_a_file_without_a_modification_date_is_left_for_a_person() {
    let r = report_of(Profile::Pdfa3b, &a3_file("/Subtype/text#2Fxml"));
    let ef = r.result("embedded-files").unwrap();
    assert!(ef.findings.iter().any(|f| f.message.contains("modification date") && !f.fixable), "{:?}", ef.findings);
    // The date is never made up: the Files group leaves it.
    let (d, _) = applied(&a3_file("/Subtype/text#2Fxml"), Profile::Pdfa3b, &[FixGroup::Files], false);
    assert!(d.new_indirect(131, 0).unwrap().get_dict("Params").unwrap().is_none());
}

#[test]
fn an_attachment_is_told_apart_by_its_first_bytes_however_big_it_is() {
    let mut big_pdf = b"%PDF-1.7\n".to_vec();
    big_pdf.extend(std::iter::repeat_n(b'x', 3_000_000));
    let mut big_other = b"PK".to_vec();
    big_other.extend(std::iter::repeat_n(b'x', 3_000_000));
    let pdf = PdfDocument::from_bytes(&build(&Spec {
        objs: vec![
            (130, s("<</Type/Filespec /F (a.pdf) /EF <</F 131 0 R>>>>")),
            (131, stream("/Type/EmbeddedFile", &big_pdf)),
            (132, s("<</Type/Filespec /F (a.zip) /EF <</F 133 0 R>>>>")),
            (133, stream("/Type/EmbeddedFile", &big_other)),
        ],
        ..Spec::default()
    }))
    .unwrap();
    assert!(check::embedded_is_pdf(&pdf.new_indirect(130, 0).unwrap()));
    assert!(!check::embedded_is_pdf(&pdf.new_indirect(132, 0).unwrap()));
}

// ---- a cancel during the last check ---------------------------------------------------------------

#[test]
fn a_cancel_during_the_check_of_the_new_file_leaves_no_file() {
    let path = file("cancel-late", &messy());
    let (eng, id) = open(&path);
    let r = run(&eng, id, Profile::Pdfa2b);
    let stop = AtomicBool::new(false);
    let dest = dir("cancel-late").join("out.pdf");
    // The flag goes up when the conversion's own steps are done: the new file is only being checked.
    let e = convert_to_file(&eng, id, Profile::Pdfa2b, &ticked(&r), dest.to_str().unwrap(), &mut |p| {
        if p.stage == "convert" && p.done == p.pages {
            stop.store(true, Ordering::Relaxed);
        }
    }, &stop)
    .unwrap_err();
    assert!(e.to_string().contains("cancelled"), "{e}");
    assert!(!dest.exists(), "nothing is left at the destination");
}

// ---- what a person has to check, a claim that is not backed, a widget's own resources ---------------

fn xmp_text(d: &PdfDocument) -> String {
    let m = d.catalog().unwrap().get_dict("Metadata").unwrap().unwrap();
    String::from_utf8_lossy(&m.read_stream().unwrap()).into_owned()
}

#[test]
fn a_rule_that_needs_a_person_keeps_a_pdfa_file_unidentified() {
    // A-3: an attachment without a modification date is left for a person, so the file is not identified.
    let undated = a3_file("/Subtype/text#2Fxml");
    let (d, a) = applied(&undated, Profile::Pdfa3b, &all_groups(Profile::Pdfa3b, &undated), false);
    assert_eq!(says(&d), None, "{:?}", a.log);
    assert!(not_identified(&a, "PDF/A-3b"), "{:?}", a.log);
    assert!(!a.identified);
    let dated = a3_file("/Subtype/text#2Fxml /Params <</ModDate (D:20260101000000Z)>>");
    let (d, a) = applied(&dated, Profile::Pdfa3b, &all_groups(Profile::Pdfa3b, &dated), false);
    assert_eq!(says(&d).as_deref(), Some("PDF/A-3b"), "{:?}", a.log);
    assert!(a.identified);
    // A-2: an embedded PDF is kept and its own conformance is left for a person.
    let with_pdf = Spec {
        cat: "/Names <</EmbeddedFiles <</Names [(a.pdf) 130 0 R]>>>>".into(),
        objs: vec![
            (130, s("<</Type/Filespec /F (a.pdf) /UF (a.pdf) /EF <</F 131 0 R>>>>")),
            (131, stream("/Type/EmbeddedFile /Subtype/application#2Fpdf", b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")),
        ],
        ..Spec::default()
    };
    let (d, a) = applied(&with_pdf, Profile::Pdfa2b, &all_groups(Profile::Pdfa2b, &with_pdf), false);
    assert_eq!(says(&d), None, "{:?}", a.log);
    assert!(not_identified(&a, "PDF/A-2b"), "{:?}", a.log);
}

#[test]
fn a_claim_the_check_does_not_back_is_taken_out_of_the_xmp() {
    // The file says PDF/A-2b, but a font the conversion cannot embed stays: the claim goes, the rest stays.
    let sp = Spec { font: "<</Type/Font/Subtype/Type1/BaseFont/QuiteUnknownTypeface/Encoding/WinAnsiEncoding>>".into(), ..with_xmp(XMP_ID) };
    assert_eq!(says(&PdfDocument::from_bytes(&build(&sp)).unwrap()).as_deref(), Some("PDF/A-2b"));
    let (d, a) = applied(&sp, Profile::Pdfa2b, &[FixGroup::Fonts, FixGroup::Metadata], false);
    assert_eq!(says(&d), None, "{:?}", a.log);
    assert!(not_identified(&a, "PDF/A-2b"), "{:?}", a.log);
    let xml = xmp_text(&d);
    assert!(!xml.contains("pdfaid:part") && !xml.contains("pdfaid:conformance"), "{xml}");
    // A file that does pass keeps it (written again, once).
    let ok = with_xmp(XMP_ID);
    let (d, a) = applied(&ok, Profile::Pdfa2b, &all_groups(Profile::Pdfa2b, &ok).into_iter().chain([FixGroup::Metadata]).collect::<Vec<_>>(), false);
    assert!(a.identified, "{:?}", a.log);
    assert_eq!(xmp_text(&d).matches("pdfaid:part").count(), 2, "one element, opened and closed");
}

#[test]
fn only_a_file_that_passes_its_final_check_may_say_it_conforms() {
    let failing = report_of(Profile::Pdfa2b, &messy());
    assert!(!super::unmet(&failing, Profile::Pdfa2b).is_empty());
    // After the Files group the undated attachment is the only thing left: a rule for a person.
    let (d, _) = applied(&a3_file("/Subtype/text#2Fxml"), Profile::Pdfa3b, &[FixGroup::Files], false);
    let r = check(&d, fix::NEW_FILE_HEAD, Profile::Pdfa3b, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let ef = r.result("embedded-files").unwrap();
    assert_eq!(ef.status, Status::Manual, "{:?}", ef.findings);
    assert!(super::unmet(&r, Profile::Pdfa3b).contains(&ef.name), "a manual rule blocks the PDF/A identification");
    assert!(!super::unmet(&r, Profile::Pdfua1).contains(&ef.name), "and is not counted for the other profiles");
}

#[test]
fn a_widget_that_carries_its_own_default_resources_keeps_the_fonts_whole() {
    let sp = Spec {
        pages: vec![("/Annots [120 0 R]".into(), TEXT.into())],
        objs: vec![(120, s("<</Type/Annot /Subtype/Widget /FT/Tx /Rect [10 10 100 30] /DR <</Font <</Helv 3 0 R>>>> /DA (/Helv 0 Tf)>>"))],
        ..Spec::default()
    };
    let d = PdfDocument::from_bytes(&build(&sp)).unwrap();
    let none = std::collections::HashSet::new();
    let ours: std::collections::HashSet<i32> = [3].into_iter().collect();
    assert!(fix::no_subsetting_reason(&d, &ours, 0, false).unwrap().unwrap().contains("default resources"));
    assert_eq!(fix::no_subsetting_reason(&d, &none, 0, false).unwrap(), None);
    // On a parent field, reached through the widget's /Parent.
    let sp = Spec {
        pages: vec![("/Annots [120 0 R]".into(), TEXT.into())],
        objs: vec![
            (120, s("<</Type/Annot /Subtype/Widget /Parent 121 0 R /Rect [10 10 100 30]>>")),
            (121, s("<</FT/Tx /T (f) /DR <</Font <</Helv 3 0 R>>>> /Kids [120 0 R]>>")),
        ],
        ..Spec::default()
    };
    let d = PdfDocument::from_bytes(&build(&sp)).unwrap();
    assert!(fix::no_subsetting_reason(&d, &ours, 0, false).unwrap().is_some());
}
