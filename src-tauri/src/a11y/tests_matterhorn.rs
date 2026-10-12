//! The rules added toward the Matterhorn Protocol: a pass and a fail case each, and the two fixes
//! (XMP metadata, ParentTree) through the edit lane.

use super::tests::{Spec, build, el, fixes, marked, report, status, tagged};
use super::tests_structure::doc3;
use super::*;

fn findings(s: &Spec, rule: Rule) -> Vec<Finding> {
    report(s).result(rule).findings.clone()
}

fn one_p(content: &str) -> Spec {
    tagged("102 0 R", vec![el(102, "P", 101, "0")], content)
}

#[test]
fn content_marking() {
    // Everything marked: passes. Painting outside marked content fails, with the page and what it is.
    assert_eq!(status(&one_p(&marked("P", 0, 700)), Rule::ContentMarking), Status::Passed);
    let loose = format!("{}BT /F1 12 Tf 72 500 Td (stray) Tj ET\n0 0 10 10 re f\n", marked("P", 0, 700));
    let r = report(&one_p(&loose));
    let f = &r.result(Rule::ContentMarking).findings;
    assert_eq!(r.status(Rule::ContentMarking), Status::Failed);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].page, Some(0));
    assert!(f[0].message.contains("1 text, 1 path"), "{}", f[0].message);
    // Artifacts are real marking.
    let art = format!("{}/Artifact BMC 0 0 10 10 re f EMC\n", marked("P", 0, 700));
    assert_eq!(status(&one_p(&art), Rule::ContentMarking), Status::Passed);
    // Marked content that isn't tagged or an artifact (optional content) is still loose.
    let oc = format!("{}/OC /MC0 BDC BT /F1 12 Tf 72 400 Td (x) Tj ET EMC\n", marked("P", 0, 700));
    assert_eq!(status(&one_p(&oc), Rule::ContentMarking), Status::Failed);
    // Nesting both ways.
    let a_in_t = "/P <</MCID 0>> BDC /Artifact BMC 0 0 10 10 re f EMC BT /F1 12 Tf 72 700 Td (t) Tj ET EMC\n";
    let r = report(&one_p(a_in_t));
    assert!(r.result(Rule::ContentMarking).findings[0].message.contains("inside tagged content"));
    let t_in_a = "/Artifact BMC /P <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td (t) Tj ET EMC EMC\n";
    let r = report(&one_p(t_in_a));
    assert!(r.result(Rule::ContentMarking).findings[0].message.contains("inside an artifact"));
    // An untagged document skips it.
    assert_eq!(status(&Spec::default(), Rule::ContentMarking), Status::Skipped);
}

fn with_role_map(map: &str, role: &str) -> Spec {
    let mut s = tagged("102 0 R", vec![el(102, role, 101, "0")], &marked(role, 0, 700));
    s.objs[0].1 = format!("<</Type/StructTreeRoot /K 101 0 R /RoleMap <<{map}>>>>");
    s
}

#[test]
fn role_map() {
    assert_eq!(status(&with_role_map("/Para /P", "Para"), Rule::RoleMap), Status::Passed);
    assert_eq!(status(&with_role_map("/Para /Para2 /Para2 /P", "Para"), Rule::RoleMap), Status::Passed, "a chain ends at a standard type");
    let r = report(&with_role_map("", "Para"));
    assert_eq!(r.status(Rule::RoleMap), Status::Failed);
    assert!(r.result(Rule::RoleMap).findings[0].message.contains("not mapped"));
    assert_eq!(r.result(Rule::RoleMap).findings[0].xref, Some(102));
    assert!(report(&with_role_map("/A /B /B /A", "A")).result(Rule::RoleMap).findings[0].message.contains("circle"));
    assert!(report(&with_role_map("/P /Span", "P")).result(Rule::RoleMap).findings[0].message.contains("remapped"));
    assert_eq!(status(&Spec::default(), Rule::RoleMap), Status::Skipped);
}

#[test]
fn language_values() {
    let mut s = one_p(&marked("P", 0, 700));
    assert_eq!(status(&s, Rule::LanguageValues), Status::Passed);
    s.cat.push_str(" /Lang (en-US)");
    assert_eq!(status(&s, Rule::LanguageValues), Status::Passed);
    let mut bad_doc = one_p(&marked("P", 0, 700));
    bad_doc.cat.push_str(" /Lang (not a tag)");
    assert_eq!(status(&bad_doc, Rule::LanguageValues), Status::Failed);
    let mut bad_el = one_p(&marked("P", 0, 700));
    bad_el.objs[2] = (102, "<</Type/StructElem /S/P /P 101 0 R /Pg 10 0 R /Lang (x y) /K 0>>".into());
    let f = findings(&bad_el, Rule::LanguageValues);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].xref, Some(102));
}

#[test]
fn tagged_pdf_flags_suspects_and_headings_mixing_h_with_numbered() {
    let mut s = one_p(&marked("P", 0, 700));
    s.cat = "/StructTreeRoot 100 0 R /MarkInfo <</Marked true /Suspects true>>".into();
    let f = findings(&s, Rule::TaggedPdf);
    assert!(f.iter().any(|f| f.message.contains("Suspects")), "{f:?}");
    let content = format!("{}{}", marked("H1", 0, 700), marked("H", 1, 680));
    let s = tagged("102 0 R 103 0 R", vec![el(102, "H1", 101, "0"), el(103, "H", 101, "1")], &content);
    let f = findings(&s, Rule::HeadingNesting);
    assert!(f.iter().any(|f| f.message.contains("unnumbered H")), "{f:?}");
}

const XMP_FULL: &str = r#"<?xpacket begin="" id="x"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:pdfuaid="http://www.aiim.org/pdfua/ns/id/"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Report</rdf:li></rdf:Alt></dc:title><pdfuaid:part>1</pdfuaid:part></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
const XMP_NO_ID: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Report</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn with_xmp(x: Option<&str>) -> Spec {
    let mut s = Spec::default();
    if let Some(x) = x {
        s.cat = "/Metadata 130 0 R".into();
        s.objs.push((130, super::tests::stream("/Type/Metadata /Subtype/XML", x)));
    }
    s
}

#[test]
fn metadata() {
    assert_eq!(status(&with_xmp(Some(XMP_FULL)), Rule::Metadata), Status::Passed);
    assert_eq!(status(&with_xmp(None), Rule::Metadata), Status::Failed);
    let f = findings(&with_xmp(Some(XMP_NO_ID)), Rule::Metadata);
    assert_eq!(f.len(), 1);
    assert!(f[0].message.contains("PDF/UA"));
    assert!(report(&with_xmp(Some(&XMP_FULL.replace("Report", "")))).result(Rule::Metadata).findings[0].message.contains("dc:title"));
}

#[test]
fn fix_metadata_creates_the_packet_from_the_title() {
    let s = Spec { info: Some("<</Title (Q3 <report> & plan)>>".into()), ..Spec::default() };
    fixes("metadata-new", &s, Fix::Metadata, Rule::Metadata);
    let d = PdfDocument::open(&super::tests::file_path_for("metadata-new")).unwrap();
    let x = matterhorn::xmp(&d.catalog().unwrap()).unwrap().unwrap();
    assert!(x.contains("Q3 &lt;report&gt; &amp; plan") && x.contains("<pdfuaid:part>1</pdfuaid:part>"), "{x}");
}

#[test]
fn the_pdf_ua_part_must_be_exactly_1() {
    let ten = XMP_FULL.replace("<pdfuaid:part>1<", "<pdfuaid:part>10<");
    assert_eq!(status(&with_xmp(Some(&ten)), Rule::Metadata), Status::Failed);
    assert_eq!(status(&with_xmp(Some(&XMP_FULL.replace("<pdfuaid:part>1</pdfuaid:part>", "<pdfuaid:part> 1 </pdfuaid:part>"))), Rule::Metadata), Status::Passed);
    let attr = XMP_FULL.replace("<pdfuaid:part>1</pdfuaid:part>", "").replace("rdf:about=\"\"", "rdf:about=\"\" pdfuaid:part=\"1\"");
    assert_eq!(status(&with_xmp(Some(&attr)), Rule::Metadata), Status::Passed);
    assert_eq!(status(&with_xmp(Some(&attr.replace("part=\"1\"", "part=\"12\""))), Rule::Metadata), Status::Failed);
}

#[test]
fn fix_metadata_extends_an_existing_packet() {
    fixes("metadata-amend", &with_xmp(Some(XMP_NO_ID)), Fix::Metadata, Rule::Metadata);
    let d = PdfDocument::open(&super::tests::file_path_for("metadata-amend")).unwrap();
    let x = matterhorn::xmp(&d.catalog().unwrap()).unwrap().unwrap();
    assert!(x.contains(">Report<"), "the title that was there is kept: {x}");
    assert_eq!(x.matches("<dc:title>").count(), 1);
}

#[test]
fn fix_metadata_refuses_when_there_is_nothing_to_add() {
    let path = super::tests::file("metadata-nothing", &with_xmp(Some(XMP_FULL)));
    let (eng, id) = super::tests::open(&path);
    assert!(fix_doc(&eng, id, &Fix::Metadata, EditSource::User).is_err());
}

#[test]
fn accessibility_permission() {
    let doc = PdfDocument::from_bytes(&build(&Spec::default())).unwrap();
    assert!(matterhorn::permissions(&doc).unwrap().is_empty(), "not encrypted");
    let mut trailer = doc.trailer().unwrap();
    trailer.dict_put("Encrypt", doc.new_object_from_str("<</Filter/Standard /P -3904>>").unwrap()).unwrap();
    assert_eq!(matterhorn::permissions(&doc).unwrap().len(), 1);
    trailer.dict_put("Encrypt", doc.new_object_from_str("<</Filter/Standard /P -3392>>").unwrap()).unwrap();
    assert!(matterhorn::permissions(&doc).unwrap().is_empty(), "bit 10 is on");
    assert_eq!(status(&Spec::default(), Rule::AccessibilityPermission), Status::Passed);
}

fn table(rows: &[&[(&str, &str)]], extra: &[(usize, String)]) -> Spec {
    // rows of (role, attributes); elements numbered from 110, MCIDs from 0 in order.
    let mut objs = vec![(102, String::new())];
    let mut content = String::new();
    let mut row_ids = String::new();
    let mut n = 110;
    let mut mcid = 0;
    for (r, cells) in rows.iter().enumerate() {
        let row = 200 + r;
        let mut kids = String::new();
        for (role, attrs) in cells.iter() {
            kids += &format!("{n} 0 R ");
            objs.push((n, format!("<</Type/StructElem /S/{role} /P {row} 0 R /Pg 10 0 R /K {mcid} {attrs}>>")));
            content += &marked(role, mcid, 700 - 10 * mcid);
            n += 1;
            mcid += 1;
        }
        objs.push((row, format!("<</Type/StructElem /S/TR /P 102 0 R /Pg 10 0 R /K [{kids}]>>")));
        row_ids += &format!("{row} 0 R ");
    }
    objs[0] = (102, format!("<</Type/StructElem /S/Table /P 101 0 R /Pg 10 0 R /K [{row_ids}]>>"));
    objs.extend(extra.iter().cloned());
    tagged("102 0 R", objs, &content)
}

#[test]
fn table_structure() {
    let th = ("TH", "");
    let td = ("TD", "");
    let ok = table(&[&[th, th], &[td, td]], &[]);
    assert_eq!(status(&ok, Rule::TableStructure), Status::Passed);
    let ragged = table(&[&[th, th], &[td]], &[]);
    let f = findings(&ragged, Rule::TableStructure);
    assert_eq!(f.len(), 1);
    assert!(f[0].message.contains("different numbers of columns"), "{}", f[0].message);
    // Spans line the rows up again.
    let span = table(&[&[th, th], &[("TD", "/A <</O/Table /ColSpan 2>>")]], &[]);
    assert_eq!(status(&span, Rule::TableStructure), Status::Passed);
    let rowspan = table(&[&[("TH", "/A <</O/Table /RowSpan 2>>"), th], &[td]], &[]);
    assert_eq!(status(&rowspan, Rule::TableStructure), Status::Passed);
    // A cell outside a row; headers naming nothing.
    let mut stray = table(&[&[th]], &[]);
    stray.objs[1].1 = stray.objs[1].1.replace("/P 200 0 R", "/P 102 0 R");
    stray.objs.iter_mut().find(|o| o.0 == 102).unwrap().1 = "<</Type/StructElem /S/Table /P 101 0 R /Pg 10 0 R /K [110 0 R]>>".into();
    assert!(findings(&stray, Rule::TableStructure).iter().any(|f| f.message.contains("outside a row")));
    let hdr = table(&[&[th], &[("TD", "/A <</O/Table /Headers [(nope)]>>")]], &[]);
    assert!(findings(&hdr, Rule::TableStructure).iter().any(|f| f.message.contains("nope")));
    let good_hdr = table(&[&[("TH", "/ID (h1)")], &[("TD", "/A <</O/Table /Headers [(h1)]>>")]], &[]);
    assert_eq!(status(&good_hdr, Rule::TableStructure), Status::Passed);
}

#[test]
fn annotations_in_the_tree() {
    let annot = |sub: &str, extra: &str| (120usize, format!("<</Type/Annot /Subtype/{sub} /Rect [72 640 180 660] /StructParent 1 {extra}>>"));
    let obj = |role: &str| (103usize, format!("<</Type/StructElem /S/{role} /P 101 0 R /Pg 10 0 R /K [<</Type/OBJR /Obj 120 0 R /Pg 10 0 R>>]>>"));
    let build_with = |role: &str, sub: &str, extra: &str, tagged_in_tree: bool| {
        let mut s = tagged(if tagged_in_tree { "102 0 R 103 0 R" } else { "102 0 R" }, vec![el(102, "P", 101, "0")], &marked("P", 0, 700));
        s.pages[0].0 = "/StructParents 0 /Annots [120 0 R]".into();
        s.objs.push(annot(sub, extra));
        if tagged_in_tree {
            s.objs.push(obj(role));
        }
        s
    };
    assert_eq!(status(&build_with("Annot", "Text", "/Contents (note)", true), Rule::AnnotationsTagged), Status::Passed);
    assert_eq!(status(&build_with("Annot", "Text", "", true), Rule::AnnotationsTagged), Status::Failed, "no description");
    assert_eq!(status(&build_with("Annot", "Text", "/Contents (note)", false), Rule::AnnotationsTagged), Status::Failed, "not in the tree");
    assert_eq!(status(&build_with("P", "Text", "/Contents (note)", true), Rule::AnnotationsTagged), Status::Failed, "wrong tag");
    assert_eq!(status(&build_with("Form", "Widget", "/FT/Tx /T (a)", true), Rule::AnnotationsTagged), Status::Passed);
    assert_eq!(status(&build_with("Link", "Widget", "/FT/Tx /T (a)", true), Rule::AnnotationsTagged), Status::Failed, "a field in a Link");
    assert_eq!(status(&build_with("Link", "Link", "", true), Rule::AnnotationsTagged), Status::Failed, "link without description");
    assert_eq!(status(&build_with("Link", "Link", "/Contents (home page)", true), Rule::AnnotationsTagged), Status::Passed);
    // Hidden annotations are not presented.
    assert_eq!(status(&build_with("Annot", "Text", "/F 2", false), Rule::AnnotationsTagged), Status::Passed);
}

#[test]
fn fonts_embedded() {
    assert_eq!(status(&Spec::default(), Rule::FontsEmbedded), Status::Failed);
    let f = findings(&Spec::default(), Rule::FontsEmbedded);
    assert_eq!(f[0].page, Some(0));
    assert!(f[0].message.contains("Helvetica"));
    let s = Spec {
        font: "<</Type/Font/Subtype/TrueType/BaseFont/Arial /FontDescriptor 131 0 R>>".into(),
        objs: vec![(131, "<</Type/FontDescriptor /FontName/Arial /FontFile2 132 0 R>>".into()), (132, super::tests::stream("", "font"))],
        ..Spec::default()
    };
    assert_eq!(status(&s, Rule::FontsEmbedded), Status::Passed);
    let t3 = Spec { font: "<</Type/Font/Subtype/Type3/FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs <<>> /Encoding <</Type/Encoding /Differences []>>>>".into(), ..Spec::default() };
    assert_eq!(status(&t3, Rule::FontsEmbedded), Status::Passed, "Type3 glyphs live in the file");
}

#[test]
fn xfa_and_optional_content() {
    assert_eq!(status(&Spec::default(), Rule::Xfa), Status::Passed);
    // Dynamic XFA (the pages are drawn when the file opens) fails; static XFA has real pages.
    let xfa = |extra: &str| Spec { cat: format!("/AcroForm <</Fields [] /XFA [(a) 140 0 R] {extra}>>"), ..Spec::default() };
    assert_eq!(status(&xfa(""), Rule::Xfa), Status::Passed, "static XFA");
    assert_eq!(status(&xfa("/NeedsRendering true"), Rule::Xfa), Status::Failed);
    assert_eq!(status(&Spec { cat: "/NeedsRendering true /AcroForm <</Fields [] /XFA []>>".into(), ..Spec::default() }, Rule::Xfa), Status::Failed);
    assert_eq!(status(&Spec { cat: "/NeedsRendering true".into(), ..Spec::default() }, Rule::Xfa), Status::Passed, "no XFA");
    assert_eq!(status(&Spec::default(), Rule::OptionalContent), Status::Passed);
    let oc = |d: &str| Spec { cat: format!("/OCProperties <</OCGs [] /D <<{d}>>>>"), ..Spec::default() };
    assert_eq!(status(&oc("/Name (Layers)"), Rule::OptionalContent), Status::Passed);
    assert_eq!(status(&oc(""), Rule::OptionalContent), Status::Failed);
    assert_eq!(status(&oc("/Name (L) /AS [<</Event/View /OCGs [] /Category [/View]>>]"), Rule::OptionalContent), Status::Failed);
}

#[test]
fn parent_tree_rule_and_its_fix() {
    assert_eq!(status(&doc3(), Rule::ParentTree), Status::Passed);
    assert_eq!(status(&Spec::default(), Rule::ParentTree), Status::Skipped);
    let mut broken = doc3();
    broken.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 102 0 R 104 0 R]]>> /ParentTreeNextKey 1>>".into();
    let f = findings(&broken, Rule::ParentTree);
    assert_eq!(f.len(), 2, "{f:?}");
    assert!(f.iter().any(|f| f.page == Some(0) && f.xref == Some(103)), "the element the entry skips: {f:?}");
    assert!(f.iter().any(|f| f.xref == Some(102)), "the element it names wrongly: {f:?}");
    assert_eq!(report(&broken).result(Rule::ParentTree).fix, Some("parentTree"));
    fixes("parent-tree-fix", &broken, Fix::ParentTree, Rule::ParentTree);
}

#[test]
fn every_new_rule_is_listed_with_a_group_and_an_id() {
    let ids: HashSet<&str> = Rule::ALL.iter().map(|r| r.id()).collect();
    assert_eq!(ids.len(), Rule::ALL.len(), "ids are unique");
    for id in ["content-marking", "role-map", "language-values", "metadata", "accessibility-permission", "table-structure", "annotations-tagged", "fonts-embedded", "xfa", "optional-content", "parent-tree"] {
        assert!(ids.contains(id), "{id}");
    }
}

#[test]
fn role_map_uses_the_pdf_1_7_types_for_ua_1() {
    // Aside is a PDF 2.0 type: UA-1 wants it mapped to a 1.7 type.
    let r = report(&with_role_map("", "Aside"));
    assert_eq!(r.status(Rule::RoleMap), Status::Failed);
    assert!(r.result(Rule::RoleMap).findings[0].message.contains("PDF 2.0"), "{:?}", r.result(Rule::RoleMap).findings);
    assert_eq!(status(&with_role_map("/Aside /Sect", "Aside"), Rule::RoleMap), Status::Passed);
    assert_eq!(status(&with_role_map("", "Sect"), Rule::RoleMap), Status::Passed);
}

#[test]
fn table_scope() {
    let th = ("TH", "");
    let td = ("TD", "");
    let f = findings(&table(&[&[th, th], &[td, td]], &[]), Rule::TableScope);
    assert_eq!(f.len(), 2, "{f:?}");
    assert_eq!(f[0].page, Some(0));
    let scoped = ("TH", "/A <</O/Table /Scope/Column>>");
    assert_eq!(status(&table(&[&[scoped, scoped], &[td, td]], &[]), Rule::TableScope), Status::Passed);
    // Cells that name their headers don't need a scope.
    let named = table(&[&[("TH", "/ID (h1)")], &[("TD", "/A <</O/Table /Headers [(h1)]>>")]], &[]);
    assert_eq!(status(&named, Rule::TableScope), Status::Passed);
    assert_eq!(status(&Spec::default(), Rule::TableScope), Status::Skipped);
}

/// A page that draws a form XObject outside any marked content. `build` names object 90 as /Im1 (an
/// image); the later object 90 here replaces it, so /Im1 is the form.
fn with_form(body: &str) -> Spec {
    let mut s = one_p(&format!("{}/Im1 Do\n", marked("P", 0, 700)));
    s.objs.push((90, super::tests::stream("/Type/XObject /Subtype/Form /BBox [0 0 600 800] /Resources <</Font <</F1 3 0 R>>>>", body)));
    s
}

#[test]
fn content_marking_looks_inside_forms_drawn_outside_marked_content() {
    assert_eq!(status(&with_form("BT /F1 12 Tf 10 10 Td (loose) Tj ET"), Rule::ContentMarking), Status::Failed);
    assert_eq!(status(&with_form("/Artifact BMC BT /F1 12 Tf 10 10 Td (fine) Tj ET EMC"), Rule::ContentMarking), Status::Passed);
    // A form drawn inside tagged content is covered by it.
    let mut inside = with_form("BT /F1 12 Tf 10 10 Td (covered) Tj ET");
    inside.pages[0].1 = "/P <</MCID 0>> BDC /Im1 Do EMC\n".into();
    assert_eq!(status(&inside, Rule::ContentMarking), Status::Passed);
}

#[test]
fn fix_metadata_refuses_what_it_would_get_wrong() {
    // A packet that isn't UTF-8 (here a UTF-16 byte order mark) is refused, not misread.
    let mut s = Spec { cat: "/Metadata 130 0 R".into(), ..Spec::default() };
    s.objs.push((130, super::tests::stream("/Type/Metadata /Subtype/XML", "ABCD<x>")));
    let mut bytes = build(&s);
    let at = bytes.windows(4).position(|w| w == b"ABCD").unwrap();
    bytes[at..at + 4].copy_from_slice(&[0xFF, 0xFE, 0x3C, 0x00]);
    let path = super::tests::file("metadata-utf16", &Spec::default());
    std::fs::write(&path, bytes).unwrap();
    let (eng, id) = super::tests::open(&path);
    let e = fix_doc(&eng, id, &Fix::Metadata, EditSource::User).unwrap_err();
    assert!(e.to_string().contains("UTF-8"), "{e}");
    // Another PDF/UA part is not turned into part 1.
    let part2 = XMP_NO_ID.replace("</rdf:RDF>", "<rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"><pdfuaid:part>2</pdfuaid:part></rdf:Description></rdf:RDF>");
    let path = super::tests::file("metadata-part2", &with_xmp(Some(&part2)));
    let (eng, id) = super::tests::open(&path);
    let e = fix_doc(&eng, id, &Fix::Metadata, EditSource::User).unwrap_err();
    assert!(e.to_string().contains("other than 1"), "{e}");
}
