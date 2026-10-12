//! Generated PDFs: a pass and a fail case per rule, fixes through the edit lane that survive
//! save and reopen, and autotag.

use super::*;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

// ---- a tiny PDF writer ---------------------------------------------------------------------------
//
// Object numbers: 1 catalog, 2 pages, 3 font /F1, 4 info, 5 bold font /F2, then page i at 10 + 2i with its content
// at 11 + 2i; tests number their own objects from 100.

#[derive(Clone)]
pub(super) struct Spec {
    pub(super) cat: String,
    pub(super) info: Option<String>,
    pub(super) font: String,
    /// `(extra page entries, content)` per page.
    pub(super) pages: Vec<(String, String)>,
    pub(super) objs: Vec<(usize, String)>,
}

const TEXT: &str = "BT /F1 12 Tf 72 700 Td (Hello accessible world) Tj ET";

impl Default for Spec {
    fn default() -> Self {
        Spec {
            cat: String::new(),
            info: None,
            font: "<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>".into(),
            pages: vec![(String::new(), TEXT.into())],
            objs: Vec::new(),
        }
    }
}

pub(super) fn stream(dict: &str, data: &str) -> String {
    format!("<<{dict} /Length {}>>\nstream\n{data}\nendstream", data.len())
}

pub(super) fn page_ref(i: usize) -> usize {
    10 + 2 * i
}

pub(super) fn build(s: &Spec) -> Vec<u8> {
    let mut objs: Vec<(usize, String)> = vec![(1, format!("<</Type/Catalog /Pages 2 0 R {}>>", s.cat))];
    let kids: String = (0..s.pages.len()).map(|i| format!("{} 0 R ", page_ref(i))).collect();
    objs.push((2, format!("<</Type/Pages /Kids [{kids}] /Count {}>>", s.pages.len())));
    objs.push((3, s.font.clone()));
    objs.push((5, "<</Type/Font/Subtype/Type1/BaseFont/Helvetica-Bold/Encoding/WinAnsiEncoding>>".into()));
    if let Some(i) = &s.info {
        objs.push((4, i.clone()));
    }
    for (i, (extra, content)) in s.pages.iter().enumerate() {
        objs.push((
            page_ref(i),
            format!(
                "<</Type/Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources <</Font <</F1 3 0 R /F2 5 0 R>> /XObject <</Im1 90 0 R>>>> /Contents {} 0 R {extra}>>",
                page_ref(i) + 1
            ),
        ));
        objs.push((page_ref(i) + 1, stream("", content)));
    }
    objs.push((90, stream("/Type/XObject /Subtype/Image /Width 2 /Height 2 /ColorSpace/DeviceGray /BitsPerComponent 8", "abcd")));
    objs.extend(s.objs.iter().cloned());
    objs.sort_by_key(|o| o.0);
    let size = objs.last().map_or(1, |o| o.0 + 1);
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = vec![None; size];
    for (n, body) in &objs {
        offsets[*n] = Some(out.len());
        let _ = write!(out, "{n} 0 obj\n{body}\nendobj\n");
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {size}\n");
    for o in &offsets {
        match o {
            Some(o) => {
                let _ = writeln!(out, "{o:010} 00000 n ");
            }
            None => out.push_str("0000000000 65535 f \n"),
        }
    }
    let info = if s.info.is_some() { " /Info 4 0 R" } else { "" };
    let _ = write!(out, "trailer\n<</Size {size} /Root 1 0 R{info}>>\nstartxref\n{xref}\n%%EOF\n");
    out.into_bytes()
}

pub(super) fn report(s: &Spec) -> Report {
    let doc = PdfDocument::from_bytes(&build(s)).unwrap();
    check(&doc, &mut |_| {}, &AtomicBool::new(false)).unwrap()
}

/// A tagged document: Document (101) with `kids`; elements from 102 on are the caller's.
pub(super) fn tagged(kids: &str, objs: Vec<(usize, String)>, content: &str) -> Spec {
    let mut all = vec![
        (100, "<</Type/StructTreeRoot /K 101 0 R>>".to_string()),
        (101, format!("<</Type/StructElem /S/Document /P 100 0 R /K [{kids}]>>")),
    ];
    all.extend(objs);
    Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content.into())],
        objs: all,
        ..Spec::default()
    }
}

pub(super) fn el(n: usize, s: &str, parent: usize, k: &str) -> (usize, String) {
    (n, format!("<</Type/StructElem /S/{s} /P {parent} 0 R /Pg 10 0 R /K {k}>>"))
}

pub(super) fn marked(tag: &str, mcid: i32, y: i32) -> String {
    format!("/{tag} <</MCID {mcid}>> BDC BT /F1 12 Tf 72 {y} Td (Item {mcid}) Tj ET EMC\n")
}

pub(super) fn status(s: &Spec, rule: Rule) -> Status {
    report(s).status(rule)
}

// ---- one pass and one fail per rule --------------------------------------------------------------

#[test]
fn tagged_pdf() {
    assert_eq!(status(&Spec::default(), Rule::TaggedPdf), Status::Failed);
    let s = tagged("102 0 R", vec![el(102, "P", 101, "0")], &marked("P", 0, 700));
    assert_eq!(status(&s, Rule::TaggedPdf), Status::Passed);
    // A tree without MarkInfo is not enough.
    let mut s2 = tagged("102 0 R", vec![el(102, "P", 101, "0")], &marked("P", 0, 700));
    s2.cat = "/StructTreeRoot 100 0 R".into();
    assert_eq!(status(&s2, Rule::TaggedPdf), Status::Failed);
}

#[test]
fn title() {
    assert_eq!(status(&Spec::default(), Rule::Title), Status::Failed);
    assert_eq!(status(&Spec { info: Some("<</Title (  )>>".into()), ..Spec::default() }, Rule::Title), Status::Failed);
    assert_eq!(status(&Spec { info: Some("<</Title (Annual report)>>".into()), ..Spec::default() }, Rule::Title), Status::Passed);
}

#[test]
fn display_doc_title() {
    assert_eq!(status(&Spec::default(), Rule::DisplayDocTitle), Status::Failed);
    let s = Spec { cat: "/ViewerPreferences <</DisplayDocTitle false>>".into(), ..Spec::default() };
    assert_eq!(status(&s, Rule::DisplayDocTitle), Status::Failed);
    let s = Spec { cat: "/ViewerPreferences <</DisplayDocTitle true>>".into(), ..Spec::default() };
    assert_eq!(status(&s, Rule::DisplayDocTitle), Status::Passed);
}

#[test]
fn primary_language() {
    assert_eq!(status(&Spec::default(), Rule::PrimaryLanguage), Status::Failed);
    assert_eq!(status(&Spec { cat: "/Lang (sv-SE)".into(), ..Spec::default() }, Rule::PrimaryLanguage), Status::Passed);
}

#[test]
fn structure_rules_are_skipped_on_untagged_documents() {
    let r = report(&Spec::default());
    for rule in [Rule::FigureAltText, Rule::ReadingOrder, Rule::TableHeaders, Rule::HeadingNesting, Rule::ListStructure] {
        assert_eq!(r.status(rule), Status::Skipped, "{rule:?}");
    }
}

#[test]
fn figure_alt_text() {
    let fig = |alt: &str| tagged("102 0 R", vec![(102, format!("<</Type/StructElem /S/Figure /P 101 0 R /Pg 10 0 R {alt} /K 0>>"))], "/Figure <</MCID 0>> BDC q 50 0 0 50 72 600 cm /Im1 Do Q EMC");
    let r = report(&fig(""));
    let f = r.result(Rule::FigureAltText);
    assert_eq!(f.status, Status::Failed);
    assert_eq!(f.findings[0].xref, Some(102));
    assert_eq!(f.findings[0].page, Some(0));
    assert_eq!(f.findings[0].object.as_deref(), Some("Figure"));
    assert_eq!(f.fix, Some("figureAlt"));
    assert_eq!(status(&fig("/Alt (A red bicycle)"), Rule::FigureAltText), Status::Passed);
    // A role-mapped figure counts as a figure.
    let mut s = tagged("102 0 R", vec![el(102, "Photo", 101, "0")], "/Photo <</MCID 0>> BDC q 50 0 0 50 72 600 cm /Im1 Do Q EMC");
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /RoleMap <</Photo /Figure>>>>".into();
    assert_eq!(status(&s, Rule::FigureAltText), Status::Failed);
}

#[test]
fn reading_order() {
    let content = format!("{}{}", marked("P", 0, 700), marked("P", 1, 680));
    let ok = tagged("102 0 R 103 0 R", vec![el(102, "P", 101, "0"), el(103, "P", 101, "1")], &content);
    assert_eq!(status(&ok, Rule::ReadingOrder), Status::Passed);
    let swapped = tagged("103 0 R 102 0 R", vec![el(102, "P", 101, "0"), el(103, "P", 101, "1")], &content);
    let r = report(&swapped);
    assert_eq!(r.status(Rule::ReadingOrder), Status::Failed);
    assert!(r.result(Rule::ReadingOrder).findings[0].message.contains("different order"));
    // Marked content no tag points at is never read.
    let orphan = tagged("102 0 R", vec![el(102, "P", 101, "0")], &content);
    let r = report(&orphan);
    assert_eq!(r.status(Rule::ReadingOrder), Status::Failed);
    assert!(r.result(Rule::ReadingOrder).findings[0].message.contains("not in the tag tree"));
}

#[test]
fn table_headers() {
    let table = |cell: &str| {
        tagged(
            "102 0 R",
            vec![el(102, "Table", 101, "[103 0 R]"), el(103, "TR", 102, "[104 0 R 105 0 R]"), el(104, cell, 103, "0"), el(105, "TD", 103, "1")],
            &format!("{}{}", marked(cell, 0, 700), marked("TD", 1, 680)),
        )
    };
    assert_eq!(status(&table("TH"), Rule::TableHeaders), Status::Passed);
    assert_eq!(status(&table("TD"), Rule::TableHeaders), Status::Failed);
}

#[test]
fn heading_nesting() {
    let doc = |second: &str| {
        tagged(
            "102 0 R 103 0 R",
            vec![el(102, "H1", 101, "0"), el(103, second, 101, "1")],
            &format!("{}{}", marked("H1", 0, 700), marked(second, 1, 680)),
        )
    };
    assert_eq!(status(&doc("H2"), Rule::HeadingNesting), Status::Passed);
    let r = report(&doc("H3"));
    assert_eq!(r.status(Rule::HeadingNesting), Status::Failed);
    assert!(r.result(Rule::HeadingNesting).findings[0].message.contains("H3 follows an H1"));
}

#[test]
fn list_structure() {
    let good = tagged(
        "102 0 R",
        vec![el(102, "L", 101, "[103 0 R]"), el(103, "LI", 102, "[104 0 R 105 0 R]"), el(104, "Lbl", 103, "0"), el(105, "LBody", 103, "1")],
        &format!("{}{}", marked("Lbl", 0, 700), marked("LBody", 1, 680)),
    );
    assert_eq!(status(&good, Rule::ListStructure), Status::Passed);
    let bad = tagged("102 0 R", vec![el(102, "LI", 101, "[103 0 R]"), el(103, "LBody", 102, "0")], &marked("LBody", 0, 700));
    let r = report(&bad);
    assert_eq!(r.status(Rule::ListStructure), Status::Failed);
    assert!(r.result(Rule::ListStructure).findings[0].message.contains("not in an L"));
}

fn link(extra: &str) -> String {
    format!("<</Type/Annot /Subtype/Link /Rect [72 690 200 710] /Border [0 0 0] /A <</S/URI /URI (https://example.com)>> {extra}>>")
}

#[test]
fn tagged_links() {
    let mut untagged = Spec::default();
    untagged.pages[0].0 = "/Annots [120 0 R]".into();
    untagged.objs.push((120, link("")));
    let r = report(&untagged);
    assert_eq!(r.status(Rule::TaggedLinks), Status::Failed);
    assert_eq!(r.result(Rule::TaggedLinks).findings[0].xref, Some(120));

    let mut s = tagged("102 0 R", vec![el(102, "Link", 101, "[0 <</Type/OBJR /Obj 120 0 R /Pg 10 0 R>>]")], &marked("Link", 0, 700));
    s.pages[0].0 = "/StructParents 0 /Annots [120 0 R] /Tabs /S".into();
    s.objs.push((120, link("/StructParent 1")));
    assert_eq!(status(&s, Rule::TaggedLinks), Status::Passed);

    // In the tree, but under a P.
    let mut p = tagged("102 0 R", vec![el(102, "P", 101, "[0 <</Type/OBJR /Obj 120 0 R /Pg 10 0 R>>]")], &marked("P", 0, 700));
    p.pages[0].0 = "/StructParents 0 /Annots [120 0 R]".into();
    p.objs.push((120, link("/StructParent 1")));
    let r = report(&p);
    assert_eq!(r.status(Rule::TaggedLinks), Status::Failed);
    assert!(r.result(Rule::TaggedLinks).findings[0].message.contains("tagged as P"));
}

fn form(tu: &str) -> Spec {
    let mut s = Spec { cat: "/AcroForm <</Fields [130 0 R]>>".into(), ..Spec::default() };
    s.pages[0].0 = "/Annots [130 0 R]".into();
    s.objs.push((130, format!("<</Type/Annot /Subtype/Widget /FT/Tx /T (first_name) {tu} /Rect [72 600 272 620] /P 10 0 R>>")));
    s
}

#[test]
fn field_tooltips() {
    let r = report(&form(""));
    assert_eq!(r.status(Rule::FieldTooltips), Status::Failed);
    assert_eq!(r.result(Rule::FieldTooltips).findings[0].object.as_deref(), Some("Field \"first_name\""));
    assert_eq!(status(&form("/TU (First name)"), Rule::FieldTooltips), Status::Passed);
    // A widget under a named parent field: the parent carries the tooltip.
    let mut s = Spec { cat: "/AcroForm <</Fields [131 0 R]>>".into(), ..Spec::default() };
    s.pages[0].0 = "/Annots [130 0 R]".into();
    s.objs.push((130, "<</Type/Annot /Subtype/Widget /Parent 131 0 R /Rect [72 600 272 620] /P 10 0 R>>".into()));
    s.objs.push((131, "<</FT/Tx /T (city) /TU (City) /Kids [130 0 R]>>".into()));
    assert_eq!(status(&s, Rule::FieldTooltips), Status::Passed);
}

#[test]
fn color_contrast_is_a_manual_check() {
    let r = report(&Spec::default());
    assert_eq!(r.status(Rule::ColorContrast), Status::Manual);
    assert_eq!(r.results.len(), Rule::ALL.len());
}

#[test]
fn fonts_unicode() {
    assert_eq!(status(&Spec::default(), Rule::FontsUnicode), Status::Passed);
    let identity = Spec { font: "<</Type/Font /Subtype/Type0 /BaseFont/Custom /Encoding/Identity-H /DescendantFonts [140 0 R]>>".into(), objs: vec![(140, "<</Type/Font /Subtype/CIDFontType2 /BaseFont/Custom /CIDSystemInfo <</Registry (Adobe) /Ordering (Identity) /Supplement 0>>>>".into())], ..Spec::default() };
    let r = report(&identity);
    assert_eq!(r.status(Rule::FontsUnicode), Status::Failed);
    assert!(r.result(Rule::FontsUnicode).findings[0].message.contains("Custom"));
    let mut mapped = identity;
    mapped.font = mapped.font.replace(">>", " /ToUnicode 141 0 R>>");
    mapped.objs.push((141, stream("", "/CIDInit /ProcSet findresource begin endcmap")));
    assert_eq!(status(&mapped, Rule::FontsUnicode), Status::Passed);
}

#[test]
fn fonts_unicode_simple_encodings() {
    let font = |enc: &str| Spec { font: format!("<</Type/Font /Subtype/Type1 /BaseFont/Custom /Encoding {enc}>>"), ..Spec::default() };
    assert_eq!(status(&font("/MacRomanEncoding"), Rule::FontsUnicode), Status::Passed);
    assert_eq!(status(&font("/CustomEncoding"), Rule::FontsUnicode), Status::Failed);
    assert_eq!(status(&font("<</BaseEncoding /WinAnsiEncoding /Differences [65 /Aacute /uni0141 /eacute]>>"), Rule::FontsUnicode), Status::Passed);
    assert_eq!(status(&font("<</Differences [1 /g12 /g13 /g14]>>"), Rule::FontsUnicode), Status::Failed);
    assert_eq!(status(&font("<</Differences [1 /glyph7]>>"), Rule::FontsUnicode), Status::Failed);
    assert!(!opaque_glyph("a12") && !opaque_glyph("uni0041") && !opaque_glyph("g") && opaque_glyph("cid0042"));
}

#[test]
fn utf16_text_strings_are_read() {
    // Title and alternate text written as UTF-16BE (as `text_obj` writes non-ASCII text).
    let mut s = tagged("102 0 R", vec![(102, "<</Type/StructElem /S/Figure /P 101 0 R /Pg 10 0 R /Alt <FEFF00C50072> /K 0>>".into())], "/Figure <</MCID 0>> BDC q 50 0 0 50 72 600 cm /Im1 Do Q EMC");
    s.info = Some("<</Title <FEFF00C50072007300720061007000700070006F00720074>>>".into());
    let r = report(&s);
    assert_eq!(r.status(Rule::Title), Status::Passed);
    assert_eq!(r.status(Rule::FigureAltText), Status::Passed);
}

#[test]
fn bookmarks() {
    let pages = |n: usize| (0..n).map(|_| (String::new(), TEXT.to_string())).collect::<Vec<_>>();
    assert_eq!(status(&Spec { pages: pages(20), ..Spec::default() }, Rule::Bookmarks), Status::Passed);
    assert_eq!(status(&Spec { pages: pages(21), ..Spec::default() }, Rule::Bookmarks), Status::Failed);
    let with = Spec {
        pages: pages(21),
        cat: "/Outlines 150 0 R".into(),
        objs: vec![(150, "<</Type/Outlines /First 151 0 R /Last 151 0 R /Count 1>>".into()), (151, "<</Title (One) /Parent 150 0 R /Dest [10 0 R /Fit]>>".into())],
        ..Spec::default()
    };
    assert_eq!(status(&with, Rule::Bookmarks), Status::Passed);
}

#[test]
fn tab_order() {
    let mut s = Spec::default();
    s.pages[0].0 = "/Annots [120 0 R]".into();
    s.objs.push((120, link("")));
    assert_eq!(status(&s, Rule::TabOrder), Status::Failed);
    s.pages[0].0 = "/Annots [120 0 R] /Tabs /S".into();
    assert_eq!(status(&s, Rule::TabOrder), Status::Passed);
    // No annotations: nothing to tab through.
    assert_eq!(status(&Spec::default(), Rule::TabOrder), Status::Passed);
}

#[test]
fn tooltips_from_names() {
    assert_eq!(tooltip_from_name("first_name"), "First name");
    assert_eq!(tooltip_from_name("dateOfBirth"), "Date of birth");
    assert_eq!(tooltip_from_name("Address.Line2"), "Address line2");
    assert_eq!(tooltip_from_name("__"), "");
}

// ---- fixes through the edit lane -----------------------------------------------------------------

pub(super) fn file(test: &str, s: &Spec) -> String {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("a11y-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p: PathBuf = d.join("Quarterly figures.pdf");
    std::fs::write(&p, build(s)).unwrap();
    p.to_string_lossy().into_owned()
}

pub(super) fn open(path: &str) -> (Arc<Engine>, DocId) {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(path).unwrap().doc;
    (eng, id)
}

pub(super) fn check_eng(eng: &Engine, id: DocId) -> Report {
    check_doc(eng, id, &mut |_| {}, &AtomicBool::new(false)).unwrap()
}

/// Applies `fix` as one undo step, checks `rule` passes, then saves, reopens and checks again.
pub(super) fn fixes(test: &str, s: &Spec, fix: Fix, rule: Rule) -> Report {
    let path = file(test, s);
    let (eng, id) = open(&path);
    assert_eq!(check_eng(&eng, id).status(rule), Status::Failed, "{rule:?} fails before");
    let steps = eng.history(id).unwrap().len();
    fix_doc(&eng, id, &fix, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1, "one undo step");
    assert_eq!(check_eng(&eng, id).status(rule), Status::Passed, "{rule:?} passes after the fix");
    eng.undo(id).unwrap();
    assert_eq!(check_eng(&eng, id).status(rule), Status::Failed, "{rule:?} fails again after undo");
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng2, id2) = open(&path);
    let r = check_eng(&eng2, id2);
    assert_eq!(r.status(rule), Status::Passed, "{rule:?} passes after save and reopen");
    r
}

fn info_title(path: &str) -> String {
    let doc = PdfDocument::open(path).unwrap();
    let info = doc.trailer().unwrap().get_dict("Info").unwrap().unwrap();
    text(&info, "Title").unwrap().unwrap()
}

#[test]
fn fix_title_given_and_from_file_name() {
    fixes("title", &Spec::default(), Fix::Title { title: Some("Årsrapport 2026".into()) }, Rule::Title);
    assert_eq!(info_title(&file_path("title")), "Årsrapport 2026");
    // An existing Info dictionary is kept and extended.
    let s = Spec { info: Some("<</Author (Kim)>>".into()), ..Spec::default() };
    fixes("title-stem", &s, Fix::Title { title: None }, Rule::Title);
    assert_eq!(info_title(&file_path("title-stem")), "Quarterly figures");
}

pub(super) fn file_path_for(test: &str) -> String {
    file_path(test)
}

fn file_path(test: &str) -> String {
    std::env::temp_dir().join("inkwell-tests").join(format!("a11y-{test}")).join("Quarterly figures.pdf").to_string_lossy().into_owned()
}

#[test]
fn fix_language() {
    fixes("lang", &Spec::default(), Fix::Language { lang: "en-GB".into() }, Rule::PrimaryLanguage);
    let path = file("lang-bad", &Spec::default());
    let (eng, id) = open(&path);
    assert!(fix_doc(&eng, id, &Fix::Language { lang: "english please".into() }, EditSource::User).is_err());
    assert_eq!(eng.history(id).unwrap().len(), 1, "a refused fix adds no step");
}

#[test]
fn fix_display_doc_title() {
    fixes("ddt", &Spec::default(), Fix::DisplayDocTitle, Rule::DisplayDocTitle);
    let s = Spec { cat: "/ViewerPreferences <</HideToolbar true /DisplayDocTitle false>>".into(), ..Spec::default() };
    fixes("ddt-existing", &s, Fix::DisplayDocTitle, Rule::DisplayDocTitle);
    let doc = PdfDocument::open(&file_path("ddt-existing")).unwrap();
    let vp = doc.catalog().unwrap().get_dict("ViewerPreferences").unwrap().unwrap();
    assert!(vp.get_dict("HideToolbar").unwrap().unwrap().as_bool().unwrap(), "other preferences stay");
}

#[test]
fn fix_tab_order() {
    let mut s = Spec { pages: vec![(String::new(), TEXT.into()), (String::new(), TEXT.into())], ..Spec::default() };
    s.pages[1].0 = "/Annots [120 0 R]".into();
    s.objs.push((120, link("")));
    fixes("tabs", &s, Fix::TabOrder, Rule::TabOrder);
    // Nothing left to fix: refused.
    let path = file_path("tabs");
    let (eng, id) = open(&path);
    assert!(fix_doc(&eng, id, &Fix::TabOrder, EditSource::User).is_err());
}

#[test]
fn fix_field_tooltips() {
    fixes("tooltips", &form(""), Fix::FieldTooltips, Rule::FieldTooltips);
    let doc = PdfDocument::open(&file_path("tooltips")).unwrap();
    let w = doc.find_page(0).unwrap().get_dict("Annots").unwrap().unwrap().get_array(0).unwrap().unwrap();
    assert_eq!(text(&w, "TU").unwrap().as_deref(), Some("First name"));
}

#[test]
fn fix_field_tooltips_named_by_an_ancestor() {
    // The terminal field has no /T: its name comes from its parent ("address.line1" style).
    let mut s = Spec { cat: "/AcroForm <</Fields [132 0 R]>>".into(), ..Spec::default() };
    s.pages[0].0 = "/Annots [130 0 R]".into();
    s.objs.push((130, "<</Type/Annot /Subtype/Widget /Parent 131 0 R /Rect [72 600 272 620] /P 10 0 R>>".into()));
    s.objs.push((131, "<</FT/Tx /Parent 132 0 R /Kids [130 0 R]>>".into()));
    s.objs.push((132, "<</T (homeAddress) /Kids [131 0 R]>>".into()));
    fixes("tooltips-ancestor", &s, Fix::FieldTooltips, Rule::FieldTooltips);
}

#[test]
fn fix_figure_alt() {
    let s = tagged(
        "102 0 R 103 0 R",
        vec![el(102, "Figure", 101, "0"), el(103, "P", 101, "1")],
        &format!("/Figure <</MCID 0>> BDC q 50 0 0 50 72 600 cm /Im1 Do Q EMC\n{}", marked("P", 1, 500)),
    );
    let xref = report(&s).result(Rule::FigureAltText).findings[0].xref.unwrap();
    fixes("alt", &s, Fix::FigureAlt { xref, alt: "Sales by region, bar chart".into() }, Rule::FigureAltText);
    // Only figures take alternate text from this fix.
    let path = file("alt-p", &s);
    let (eng, id) = open(&path);
    let e = fix_doc(&eng, id, &Fix::FigureAlt { xref: 103, alt: "x".into() }, EditSource::User).unwrap_err();
    assert!(e.to_string().contains("not a Figure"), "{e}");
}

// ---- autotag -------------------------------------------------------------------------------------

pub(super) fn untagged_article_for_structure() -> Spec {
    untagged_article()
}

fn untagged_article() -> Spec {
    let mut c = String::new();
    c.push_str("0.9 g 0 0 612 792 re f 0 g\n");
    c.push_str("BT /F1 24 Tf 72 720 Td (Annual report) Tj ET\n");
    c.push_str("BT /F1 12 Tf 72 680 Td 14 TL (Sales grew in every region this year, led by the north.) Tj T* (The board thanks every employee for their work.) Tj ET\n");
    c.push_str("BT /F1 18 Tf 72 620 Td (Highlights) Tj ET\n");
    c.push_str("BT /F1 12 Tf 72 590 Td (- New office in Lund) Tj ET\n");
    c.push_str("BT /F1 12 Tf 72 576 Td (- Two product launches) Tj ET\n");
    c.push_str("q 100 0 0 80 72 440 cm /Im1 Do Q\n");
    c.push_str("BT /F1 12 Tf 72 400 Td (Read more online.) Tj ET\n");
    let mut s = Spec { pages: vec![("/Annots [120 0 R]".into(), c)], ..Spec::default() };
    s.objs.push((120, "<</Type/Annot /Subtype/Link /Rect [72 395 180 412] /Border [0 0 0] /A <</S/URI /URI (https://example.com)>>>>".into()));
    s
}

fn texts(eng: &Engine, id: DocId) -> Vec<String> {
    eng.page_text(id, 0).unwrap().into_iter().map(|l| l.t).collect()
}

#[test]
fn autotag_builds_a_tree_that_passes_the_structure_rules() {
    let path = file("autotag", &untagged_article());
    let (eng, id) = open(&path);
    let before = texts(&eng, id);
    let png_before = eng.page_png(id, eng.edit_state(id).unwrap().rev, 0, 300).unwrap();
    let r = check_eng(&eng, id);
    assert_eq!(r.status(Rule::TaggedPdf), Status::Failed);
    assert_eq!(r.result(Rule::TaggedPdf).fix, Some("autotag"));

    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().last().unwrap().label, "Autotag (best effort)");
    assert_eq!(texts(&eng, id), before, "the text is unchanged");
    let png_after = eng.page_png(id, eng.edit_state(id).unwrap().rev, 0, 300).unwrap();
    assert_eq!(png_before, png_after, "the page looks the same");

    let r = check_eng(&eng, id);
    for rule in [Rule::TaggedPdf, Rule::HeadingNesting, Rule::ListStructure, Rule::TaggedLinks, Rule::ReadingOrder] {
        assert_eq!(r.status(rule), Status::Passed, "{rule:?}: {:?}", r.result(rule).findings);
    }
    // The figure has no alternate text: the checker asks for it, and the fix supplies it.
    let figs = &r.result(Rule::FigureAltText).findings;
    assert_eq!(figs.len(), 1, "{figs:?}");
    let xref = figs[0].xref.unwrap();
    fix_doc(&eng, id, &Fix::FigureAlt { xref, alt: "Photo of the new office".into() }, EditSource::User).unwrap();
    assert_eq!(check_eng(&eng, id).status(Rule::FigureAltText), Status::Passed);

    eng.save(id, None).unwrap();
    eng.close(id);
    let doc = PdfDocument::open(&path).unwrap();
    let t = Tree::read(&doc, &tree::page_index(&doc).unwrap()).unwrap();
    let types: Vec<&str> = t.preorder().into_iter().map(|i| t.elems[i].ty.as_str()).collect();
    assert_eq!(
        types,
        ["Document", "H1", "P", "H2", "L", "LI", "LBody", "LI", "LBody", "Figure", "P", "Link"],
        "{types:?}"
    );
    let (eng2, id2) = open(&path);
    let r = check_eng(&eng2, id2);
    for rule in [Rule::TaggedPdf, Rule::HeadingNesting, Rule::ListStructure, Rule::TaggedLinks, Rule::ReadingOrder, Rule::FigureAltText] {
        assert_eq!(r.status(rule), Status::Passed, "{rule:?} after reopen: {:?}", r.result(rule).findings);
    }
    assert_eq!(texts(&eng2, id2), before);
}

#[test]
fn autotag_refuses_a_tagged_document_and_undoes_in_one_step() {
    let s = tagged("102 0 R", vec![el(102, "P", 101, "0")], &marked("P", 0, 700));
    let path = file("autotag-tagged", &s);
    let (eng, id) = open(&path);
    assert!(fix_doc(&eng, id, &Fix::Autotag, EditSource::User).is_err());

    let path = file("autotag-undo", &untagged_article());
    let (eng, id) = open(&path);
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    assert!(check_eng(&eng, id).tagged);
    eng.undo(id).unwrap();
    assert!(!check_eng(&eng, id).tagged);
}

#[test]
fn autotag_leaves_overlapping_runs_unmarked() {
    // Invalid content: a path is left open across a text object, so the path's artifact run would
    // start before the text run ends. It is skipped; the text is still tagged.
    let mut s = Spec::default();
    s.pages[0].1 = "0 G 72 650 m BT /F1 12 Tf 72 700 Td (Hello accessible world) Tj ET 300 650 l S".into();
    let path = file("autotag-overlap", &s);
    let (eng, id) = open(&path);
    let before = texts(&eng, id);
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    assert_eq!(texts(&eng, id), before);
    let r = check_eng(&eng, id);
    assert_eq!(r.status(Rule::TaggedPdf), Status::Passed);
    assert_eq!(r.status(Rule::ReadingOrder), Status::Passed, "{:?}", r.result(Rule::ReadingOrder).findings);
}

#[test]
fn check_is_cancellable_and_does_not_hold_the_edit_lane() {
    let s = Spec { pages: (0..3).map(|_| (String::new(), TEXT.to_string())).collect(), ..Spec::default() };
    let path = file("check-lane", &s);
    let (eng, id) = open(&path);
    let e = check_doc(&eng, id, &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
    assert_eq!(e.to_string(), "cancelled");
    // Unsaved edits are checked (the published snapshot).
    fix_doc(&eng, id, &Fix::Language { lang: "de".into() }, EditSource::User).unwrap();
    assert_eq!(check_eng(&eng, id).status(Rule::PrimaryLanguage), Status::Passed);
    // A check runs while an edit holds the lane (it would deadlock if it needed the lane).
    let mut inside = None;
    let r = eng.apply_custom(id, "probe".into(), EditSource::User, false, |_| {
        inside = Some(check_eng(&eng, id));
        Err(EngineError::Invalid("probe only".into()))
    });
    assert!(r.is_err());
    assert_eq!(inside.unwrap().pages, 3);
}

#[test]
fn check_reports_progress_per_page() {
    let s = Spec { pages: (0..5).map(|_| (String::new(), TEXT.to_string())).collect(), ..Spec::default() };
    let doc = PdfDocument::from_bytes(&build(&s)).unwrap();
    let mut seen = Vec::new();
    check(&doc, &mut |p| seen.push(p.done), &AtomicBool::new(false)).unwrap();
    assert_eq!(seen, [1, 2, 3, 4, 5]);
    assert!(check(&doc, &mut |_| {}, &AtomicBool::new(true)).is_err(), "cancelled");
}

#[test]
fn cancel_is_seen_after_the_page_scan() {
    // Cancelled while the last page reports progress: the rule pass stops before the report.
    let s = Spec { pages: (0..3).map(|_| (String::new(), TEXT.to_string())).collect(), ..Spec::default() };
    let doc = PdfDocument::from_bytes(&build(&s)).unwrap();
    let cancel = AtomicBool::new(false);
    let e = check(&doc, &mut |p| cancel.store(p.done == p.pages, Ordering::Relaxed), &cancel).unwrap_err();
    assert_eq!(e.to_string(), "cancelled");
}

#[test]
fn autotag_keeps_text_around_an_image_inside_a_text_object() {
    // `BT Tj /Im1 Do Tj ET`: the image run must not push the text runs out of order.
    let mut s = Spec::default();
    s.pages[0].1 = "BT /F1 12 Tf 72 700 Td (Before the image) Tj /Im1 Do (after it) Tj ET".into();
    let path = file("autotag-do-in-bt", &s);
    let (eng, id) = open(&path);
    let before = texts(&eng, id);
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    assert_eq!(texts(&eng, id), before);
    let data = eng.read_custom(id, |d| page_content(&d.find_page(0)?)).unwrap();
    let marked: Vec<String> = content::parse(&data)
        .iter()
        .filter(|o| o.is(b"BDC"))
        .map(|o| String::from_utf8_lossy(o.operands[0].name().unwrap()).into_owned())
        .collect();
    assert_eq!(marked, ["P", "Figure", "P"], "both text runs and the image are tagged");
    // Every marked run reaches the tree (the figure is placed by position, so tag order may differ).
    let r = check_eng(&eng, id);
    assert_eq!(r.status(Rule::TaggedPdf), Status::Passed);
    assert!(!r.result(Rule::ReadingOrder).findings.iter().any(|f| f.message.contains("not in the tag tree")));
}

#[test]
fn report_serialises_with_rule_ids() {
    let v = serde_json::to_value(report(&Spec::default())).unwrap();
    assert_eq!(v["results"][0]["rule"], "tagged-pdf");
    assert_eq!(v["results"][0]["status"], "failed");
    let f: Fix = serde_json::from_str(r#"{"fix":"figureAlt","xref":12,"alt":"A map"}"#).unwrap();
    assert_eq!(f, Fix::FigureAlt { xref: 12, alt: "A map".into() });
    let f: Fix = serde_json::from_str(r#"{"fix":"title"}"#).unwrap();
    assert_eq!(f, Fix::Title { title: None });
}

// ---- where findings are, progress, groups, report files -------------------------------------------

#[test]
fn findings_carry_a_box_for_figures_and_links() {
    // A 50 x 50 figure at (72, 600) in PDF space is at x 72..122, y 142..192 from the top of a 792 pt page.
    let mut s = tagged(
        "102 0 R",
        vec![(102, "<</Type/StructElem /S/Figure /P 101 0 R /Pg 10 0 R /K 0>>".into())],
        "/Figure <</MCID 0>> BDC q 50 0 0 50 72 600 cm /Im1 Do Q EMC",
    );
    s.pages[0].0 = "/StructParents 0 /Annots [120 0 R]".into();
    s.objs.push((120, "<</Type/Annot /Subtype/Link /Rect [72 395 180 412] /Border [0 0 0]>>".into()));
    let r = report(&s);
    let fig = &r.result(Rule::FigureAltText).findings[0];
    let b = fig.rect.expect("a figure has a box");
    assert!((b[0] - 72.0).abs() < 0.5 && (b[1] - 142.0).abs() < 0.5 && (b[2] - 122.0).abs() < 0.5 && (b[3] - 192.0).abs() < 0.5, "{b:?}");
    let link = &r.result(Rule::TaggedLinks).findings[0];
    let b = link.rect.expect("a link has a box");
    assert!((b[0] - 72.0).abs() < 0.5 && (b[1] - 380.0).abs() < 0.5 && (b[2] - 180.0).abs() < 0.5 && (b[3] - 397.0).abs() < 0.5, "{b:?}");
    // a document-level finding has none
    assert_eq!(report(&Spec::default()).result(Rule::Title).findings[0].rect, None);
}

#[test]
fn many_fixes_are_one_undo_step_and_skip_what_has_nothing_to_do() {
    let path = file("many", &Spec::default());
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    // TabOrder has no annotated page here: skipped, the others apply.
    let many = Fix::Many { fixes: vec![Fix::Title { title: None }, Fix::Language { lang: "en-US".into() }, Fix::TabOrder] };
    fix_doc(&eng, id, &many, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    let r = check_eng(&eng, id);
    assert_eq!((r.status(Rule::Title), r.status(Rule::PrimaryLanguage)), (Status::Passed, Status::Passed));
    eng.undo(id).unwrap();
    let r = check_eng(&eng, id);
    assert_eq!((r.status(Rule::Title), r.status(Rule::PrimaryLanguage)), (Status::Failed, Status::Failed));
    eng.redo(id).unwrap();
    // Nothing left to do: refused, no step.
    let n = eng.history(id).unwrap().len();
    let e = fix_doc(&eng, id, &Fix::Many { fixes: vec![Fix::TabOrder, Fix::FieldTooltips] }, EditSource::User).unwrap_err();
    assert_eq!(e.to_string(), "nothing to fix");
    assert_eq!(eng.history(id).unwrap().len(), n, "a refused group adds no step");
    let ser: Fix = serde_json::from_str(r#"{"fix":"many","fixes":[{"fix":"tabOrder"},{"fix":"language","lang":"sv"}]}"#).unwrap();
    assert_eq!(ser, Fix::Many { fixes: vec![Fix::TabOrder, Fix::Language { lang: "sv".into() }] });
}

#[test]
fn autotag_reports_progress_and_a_cancel_changes_nothing() {
    let path = file("autotag-cancel", &untagged_article());
    let (eng, id) = open(&path);
    let mut seen: Vec<(&'static str, u32)> = Vec::new();
    fix_doc_with(&eng, id, &Fix::Autotag, EditSource::User, &mut |p| seen.push((p.stage, p.done)), &AtomicBool::new(false)).unwrap();
    assert_eq!(seen, [("read", 1), ("tag", 1)]);
    eng.undo(id).unwrap();
    let steps = eng.history(id).unwrap().len();

    // Cancelled from the progress callback after the page was read: the step is not added and the document is untagged.
    let stop = AtomicBool::new(false);
    let e = fix_doc_with(&eng, id, &Fix::Autotag, EditSource::User, &mut |_| stop.store(true, Ordering::Relaxed), &stop).unwrap_err();
    assert_eq!(e.to_string(), "cancelled");
    assert_eq!(eng.history(id).unwrap().len(), steps, "no new step");
    assert!(!check_eng(&eng, id).tagged);
    // and it can still be run afterwards
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    assert!(check_eng(&eng, id).tagged);
}

#[test]
fn preview_counts_what_autotag_would_tag() {
    let path = file("preview", &untagged_article());
    let (eng, id) = open(&path);
    let p = preview_doc(&eng, id).unwrap();
    assert_eq!(p, autotag::Preview { untagged: true, pages: 1, figures: 1, links: 1, fields: 0 });
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    assert!(!preview_doc(&eng, id).unwrap().untagged);
}

#[test]
fn every_rule_has_one_of_acrobats_groups_and_the_report_lists_it() {
    for rule in Rule::ALL {
        assert!(report::GROUPS.contains(&rule.group()), "{rule:?}");
    }
    let r = report(&untagged_article());
    assert!(r.results.iter().all(|x| !x.group.is_empty()));
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["results"][0]["group"], "Document");
}

#[test]
fn the_report_is_written_as_html_and_as_a_pdf() {
    let r = report(&untagged_article());
    let html = report::html("Annual <report>.pdf", "11 Oct 2026", &r);
    assert!(html.contains("<h1>Accessibility report</h1>"));
    assert!(html.contains("Annual &lt;report&gt;.pdf"), "the name is escaped");
    assert!(html.contains("<h3>Alternate text</h3>") && html.contains("Tagged PDF"));
    assert!(html.contains("class=\"failed\">Failed"));
    let pdf = report::pdf("Annual report.pdf", "11 Oct 2026", &r).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let doc = PdfDocument::from_bytes(&pdf).unwrap();
    assert!(doc.page_count().unwrap() >= 1);
    // through the engine, cancellable
    let path = file("report-file", &untagged_article());
    let (eng, id) = open(&path);
    let bytes = report_bytes(&eng, id, false, "today", &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert!(String::from_utf8(bytes).unwrap().contains("Quarterly figures.pdf"));
    assert!(report_bytes(&eng, id, true, "today", &mut |_| {}, &AtomicBool::new(true)).is_err());
}

#[test]
fn a_report_never_replaces_a_document_that_is_open() {
    let path = file("report-target", &untagged_article());
    let (eng, _id) = open(&path);
    // the guard both report exports call before they write
    assert!(eng.check_not_open(&path, None).is_err(), "the open document's own file is refused");
    let elsewhere = file("report-elsewhere", &untagged_article());
    assert!(eng.check_not_open(&elsewhere, None).is_ok(), "a file that is not open is fine");
    // and the commands do call it, ahead of the write
    for (name, src) in [("a11y_export_report", include_str!("app.rs")), ("history_compare_report", include_str!("../history_ipc.rs"))] {
        let at = src.find(&format!("fn {name}")).unwrap();
        let body = &src[at..];
        let guard = body.find("check_not_open").unwrap_or(usize::MAX);
        let write = body.find("write_replace").or_else(|| body.find("report_to(")).unwrap();
        assert!(guard < write, "{name} checks the target before writing");
    }
}
