//! Autotag tables of contents and captions: what becomes TOC > TOCI and Caption, and what stays
//! paragraphs. Every case goes through `autotag`, which checks that the page looks and extracts the
//! same and that the tree passes the structure rules.

use super::tests::{Spec, check_eng, file, open};
use super::tests_tables::{at, autotag, grid, page};
use super::*;

/// An element and what is under it: `TOCI(Link*)`, `*` for an annotation and `#` for marked content.
fn outline(t: &Tree, i: usize) -> String {
    let e = &t.elems[i];
    let mut s = e.ty.clone();
    s += &"*".repeat(e.annots.len());
    s += &"#".repeat(e.mcids.len().min(1));
    if !e.children.is_empty() {
        s += &format!("({})", e.children.iter().map(|&c| outline(t, c)).collect::<Vec<_>>().join(","));
    }
    s
}

/// The first element of type `ty` and its outline.
fn find(t: &Tree, ty: &str) -> Option<usize> {
    (0..t.elems.len()).find(|&i| t.elems[i].ty == ty)
}

fn parent_type(t: &Tree, i: usize) -> Option<&str> {
    t.elems[i].parent.map(|p| t.elems[p].ty.as_str())
}

/// The x that puts the right edge of a page number (Helvetica, 12 pt) at 500.
fn right(no: &str) -> i32 {
    let w: f32 = no.chars().map(|c| match c {
        'i' => 2.7,
        'v' | 'x' => 6.0,
        _ => 6.7,
    }).sum();
    (500.0 - w).round() as i32
}

/// Entries "Chapter n" with a dot leader and a page number, all of one width so the right edges line up.
fn leader_page(y0: i32, pages: &[&str]) -> String {
    let mut c = String::new();
    for (i, p) in pages.iter().enumerate() {
        c += &at("F1", 12, 72, y0 - 18 * i as i32, &format!("Chapter {} ........................ {p}", i + 1));
    }
    c
}

#[test]
fn entries_with_a_dot_leader_and_a_page_number_are_a_toc() {
    let mut c = at("F1", 18, 72, 740, "Contents");
    c += &leader_page(700, &["3", "5", "7", "9"]);
    c += &at("F1", 12, 72, 520, "This report covers the year.");
    let t = autotag("toc-leader", &page(c));
    assert_eq!(t.types, ["Document", "H1", "TOC", "TOCI", "TOCI", "TOCI", "TOCI", "P"].map(String::from), "{:?}", t.types);
    // One run of marked content for each line, and each entry's text is in its own TOCI.
    assert_eq!(t.content.matches("/TOCI <</MCID").count(), 4, "{}", t.content);
    assert_eq!(t.content.matches("<</MCID").count(), 6);
    let toc = find(&t.tree, "TOC").unwrap();
    assert!(t.tree.elems[toc].children.iter().all(|&c| t.tree.elems[c].ty == "TOCI" && t.tree.elems[c].mcids.len() == 1));
}

#[test]
fn a_leader_and_a_page_number_set_as_their_own_lines_are_a_toc() {
    // Title, leader and page number are three text objects on one baseline; page numbers of one or two
    // digits, right aligned.
    let mut c = String::new();
    for (i, (title, no)) in [("Introduction", "1"), ("Background", "7"), ("Method", "12"), ("Results", "20")].iter().enumerate() {
        let y = 700 - 18 * i as i32;
        c += &at("F1", 12, 72, y, title);
        c += &at("F1", 12, 160, y, "......................");
        c += &at("F1", 12, right(no), y, no);
    }
    let t = autotag("toc-split", &page(c));
    assert_eq!((t.count("TOC"), t.count("TOCI"), t.count("Table")), (1, 4, 0), "{:?}", t.types);
    // Each entry's three lines are in its TOCI, in order.
    assert_eq!(outline(&t.tree, find(&t.tree, "TOCI").unwrap()), "TOCI#");
    assert_eq!(t.content.matches("/TOCI <</MCID").count(), 12);
}

#[test]
fn a_wide_gap_and_a_roman_or_arabic_page_number_make_a_toc() {
    let mut c = String::new();
    for (i, (title, no)) in [("Preface", "iii"), ("Foreword", "vii"), ("Introduction", "1"), ("Method", "9"), ("Results", "24")].iter().enumerate() {
        let y = 700 - 18 * i as i32;
        c += &at("F1", 12, 72, y, title);
        c += &at("F1", 12, right(no), y, no);
    }
    let t = autotag("toc-gap", &page(c));
    assert_eq!((t.count("TOC"), t.count("TOCI"), t.count("Table")), (1, 5, 0), "{:?}", t.types);
}

#[test]
fn a_numbered_entry_keeps_its_number_in_the_toci() {
    // Number, title and page number: three lines on a baseline.
    let mut c = String::new();
    for (i, (title, no)) in [("Introduction", "1"), ("Background", "7"), ("Method", "12"), ("Results", "20")].iter().enumerate() {
        let y = 700 - 18 * i as i32;
        c += &at("F1", 12, 72, y, &format!("{}.", i + 1));
        c += &at("F1", 12, 100, y, title);
        c += &at("F1", 12, right(no), y, no);
    }
    let t = autotag("toc-numbered", &page(c));
    assert_eq!((t.count("TOC"), t.count("TOCI"), t.count("L")), (1, 4, 0), "{:?}", t.types);
}

#[test]
fn a_roman_numeral_list_with_bold_first_and_last_rows_is_a_toc_not_a_table() {
    // The first and last rows are bold, so a table would have taken them: a header row, then a body.
    // The page numbers are roman, so the table check did not see a column of page numbers.
    let rows = [("Foreword", "i"), ("Acknowledgements", "iii"), ("Preface", "v"), ("Notes on the text", "ix"), ("Index of terms", "xii")];
    let mut c = String::new();
    for (i, (title, no)) in rows.iter().enumerate() {
        let (y, font) = (700 - 18 * i as i32, if i == 0 || i == rows.len() - 1 { "F2" } else { "F1" });
        c += &at(font, 12, 72, y, title);
        c += &at(font, 12, right(no), y, no);
    }
    let t = autotag("toc-roman", &page(c));
    assert_eq!((t.count("TOC"), t.count("TOCI"), t.count("Table"), t.count("TH")), (1, 5, 0, 0), "{:?}", t.types);
}

#[test]
fn a_two_column_table_with_a_rule_stays_a_table() {
    // The same shape as an entry list (a word and a small number, right aligned), under a ruling
    // line across both columns: the rule is the evidence of a table, so the table takes it.
    let rows: [&[&str]; 4] = [&["Pens", "12"], &["Ink", "4"], &["Paper", "130"], &["Tape", "9"]];
    let mut c = grid(&rows, &[72, 440], 700, 18, false);
    c += "0 g 72 666 400 0.8 re f\n";
    let t = autotag("toc-vs-table", &page(c));
    assert_eq!((t.count("Table"), t.count("TOC")), (1, 0), "{:?}", t.types);
}

#[test]
fn what_is_not_clearly_a_toc_stays_paragraphs() {
    let toc = |name: &str, c: String| {
        let t = autotag(name, &page(c));
        assert_eq!((t.count("TOC"), t.count("Table")), (0, 0), "{name}: {:?}", t.types);
    };
    // Two entries are not enough.
    toc("toc-two", leader_page(700, &["3", "5"]));
    // Page numbers that go back: a price list with a dot leader.
    toc("toc-prices", leader_page(700, &["25", "18", "30", "12"]));
    // Page numbers that do not line up in a column.
    let mut c = String::new();
    for (i, (title, x, no)) in [("Introduction", 500, "1"), ("Background", 430, "7"), ("Method", 510, "12"), ("Results", 470, "20")].iter().enumerate() {
        c += &at("F1", 12, 72, 700 - 18 * i as i32, title);
        c += &at("F1", 12, *x, 700 - 18 * i as i32, no);
    }
    toc("toc-ragged", c);
    // A number after a short gap and no leader: a title and a figure, not a page number.
    let mut c = String::new();
    for (i, (title, no)) in [("Room", "101"), ("Floor", "7"), ("Desk", "12")].iter().enumerate() {
        c += &at("F1", 12, 72, 700 - 18 * i as i32, &format!("{title} {no}"));
    }
    toc("toc-no-gap", c);
    // A word that looks like a roman numeral is not a page number.
    let mut c = String::new();
    for (i, (title, w)) in [("Introduction", "mix"), ("Background", "did"), ("Method", "mild")].iter().enumerate() {
        c += &at("F1", 12, 72, 700 - 18 * i as i32, title);
        c += &at("F1", 12, 500, 700 - 18 * i as i32, w);
    }
    toc("toc-words", c);
    // Entries drawn column by column (all titles, then all page numbers): tag order would not be draw order.
    let rows = [("Introduction", "1"), ("Background", "7"), ("Method", "12"), ("Results", "20")];
    let mut c = String::new();
    for (i, (title, _)) in rows.iter().enumerate() {
        c += &at("F1", 12, 72, 700 - 18 * i as i32, title);
    }
    for (i, (_, no)) in rows.iter().enumerate() {
        c += &at("F1", 12, right(no), 700 - 18 * i as i32, no);
    }
    toc("toc-columns", c);
}

/// Link annotations over entries 1 and 2 (a page that exists), 3 (a page that does not), and none over 4.
fn linked_toc() -> Spec {
    let mut c = at("F1", 18, 72, 740, "Contents");
    c += &leader_page(700, &["3", "5", "7", "9"]);
    let mut s = page(c);
    s.pages[0].0 = "/Annots [120 0 R 121 0 R 122 0 R]".into();
    for (n, y, dest) in [(120, 700, "10 0 R"), (121, 682, "10 0 R"), (122, 664, "999 0 R")] {
        s.objs.push((n, format!("<</Type/Annot /Subtype/Link /Rect [72 {} 330 {}] /Border [0 0 0] /Dest [{dest} /Fit]>>", y - 3, y + 11)));
    }
    s
}

#[test]
fn a_link_over_an_entry_becomes_a_link_in_its_toci_and_only_there() {
    let t = autotag("toc-links", &linked_toc());
    let toc = find(&t.tree, "TOC").unwrap();
    let entries: Vec<String> = t.tree.elems[toc].children.iter().map(|&c| outline(&t.tree, c)).collect();
    // Entries 1 and 2 hold a Link that has the text and the annotation; 3 (its page is not there) and 4
    // (no annotation) hold their text. Nothing was added to the annotations.
    assert_eq!(entries, ["TOCI(Link*#)", "TOCI(Link*#)", "TOCI#", "TOCI#"], "{entries:?}");
    // The annotation that goes nowhere is still tagged as a link, by itself, outside the TOC.
    let links: Vec<usize> = (0..t.tree.elems.len()).filter(|&i| t.tree.elems[i].ty == "Link").collect();
    assert_eq!(links.len(), 3);
    assert_eq!(links.iter().filter(|&&i| parent_type(&t.tree, i) == Some("TOCI")).count(), 2);
    assert_eq!(t.report.status(Rule::TaggedLinks), Status::Passed, "{:?}", t.report.result(Rule::TaggedLinks).findings);
    let doc = PdfDocument::open(&file_path("toc-links")).unwrap();
    let page = doc.find_page(0).unwrap();
    assert_eq!(page.get_dict("Annots").unwrap().unwrap().len().unwrap(), 3, "no annotation was invented");
}

#[test]
fn a_toc_without_any_annotation_gets_no_link() {
    let t = autotag("toc-nolinks", &page(leader_page(700, &["3", "5", "7"])));
    assert_eq!((t.count("TOC"), t.count("TOCI"), t.count("Link")), (1, 3, 0), "{:?}", t.types);
}

fn file_path(test: &str) -> String {
    super::tests::file_path_for(test)
}

// ---- captions ------------------------------------------------------------------------------------

/// An image 200 x 150 pt with its lower left corner at (72, 520).
const IMAGE: &str = "q 200 0 0 150 72 520 cm /Im1 Do Q\n";

#[test]
fn a_figure_label_under_an_image_is_its_caption() {
    let mut c = at("F1", 12, 72, 740, "Revenue grew in every region, and the");
    c += &at("F1", 12, 72, 726, "increase was largest in the north.");
    c += IMAGE;
    c += &at("F1", 10, 72, 500, "Figure 1. Quarterly revenue by region");
    let t = autotag("caption-figure", &page(c));
    assert_eq!(t.types, ["Document", "P", "Figure", "Caption"].map(String::from), "{:?}", t.types);
    let cap = find(&t.tree, "Caption").unwrap();
    assert_eq!(parent_type(&t.tree, cap), Some("Figure"));
    assert_eq!(t.content.matches("/Caption <</MCID").count(), 1, "{}", t.content);
    // The figure's own content comes first, the caption after it (it is drawn after).
    assert_eq!(outline(&t.tree, find(&t.tree, "Figure").unwrap()), "Figure#(Caption#)");
}

#[test]
fn a_table_label_above_a_table_is_its_first_kid_and_under_it_the_last() {
    let mut c = at("F1", 10, 72, 730, "Table 2. Sales by region");
    c += &grid(&[&["Region", "Units", "Revenue"], &["North", "120", "4 800"], &["South", "95", "3 750"]], &[72, 200, 330], 700, 18, true);
    let t = autotag("caption-table-above", &page(c));
    let table = find(&t.tree, "Table").unwrap();
    let kids: Vec<&str> = t.tree.elems[table].children.iter().map(|&c| t.tree.elems[c].ty.as_str()).collect();
    assert_eq!(kids, ["Caption", "TR", "TR", "TR"], "{:?}", t.types);
    assert_eq!(t.report.status(Rule::TableStructure), Status::Passed);

    let mut c = grid(&[&["Region", "Units", "Revenue"], &["North", "120", "4 800"], &["South", "95", "3 750"]], &[72, 200, 330], 700, 18, true);
    c += &at("F1", 10, 72, 640, "TABLE 3: Totals for the year");
    let t = autotag("caption-table-below", &page(c));
    let table = find(&t.tree, "Table").unwrap();
    let kids: Vec<&str> = t.tree.elems[table].children.iter().map(|&c| t.tree.elems[c].ty.as_str()).collect();
    assert_eq!(kids, ["TR", "TR", "TR", "Caption"], "{:?}", t.types);
}

#[test]
fn a_line_that_is_far_long_or_a_sentence_is_not_a_caption() {
    let no_caption = |name: &str, below: &str, y: i32| {
        let mut c = String::from(IMAGE);
        c += &at("F1", 10, 72, y, below);
        let t = autotag(name, &page(c));
        assert_eq!((t.count("Caption"), t.count("Figure")), (0, 1), "{name}: {:?}", t.types);
    };
    no_caption("caption-far", "Figure 1. Quarterly revenue by region", 440);
    let long = format!("Figure 1. Quarterly revenue by region, {}", "with the totals for every office shown in a lighter shade, ".repeat(3));
    assert!(long.chars().count() > captions::MAX_CHARS);
    no_caption("caption-long", &long, 500);
    no_caption("caption-sentence", "Figure 1 shows the quarterly revenue by region", 500);
    // A table label under a figure: the wrong kind.
    no_caption("caption-kind", "Table 1. Quarterly revenue by region", 500);

    // "Figure 3 shows ..." in the middle of a paragraph under the figure.
    let mut c = String::from(IMAGE);
    for (i, l) in ["Revenue is up in the north and in the south,", "and Figure 3 shows the split by quarter, which", "is the reason the board asked for the report."].iter().enumerate() {
        c += &at("F1", 10, 72, 500 - 12 * i as i32, l);
    }
    let t = autotag("caption-mid", &page(c));
    assert_eq!((t.count("Caption"), t.count("P")), (0, 1), "{:?}", t.types);
}

// ---- signed and certified documents --------------------------------------------------------------

const SIGNATURE: &str = "<</FT/Sig /T(Signature1) /V <</Type/Sig /Filter/Adobe.PPKLite /SubFilter/adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00>>>>>";

/// An untagged page with a table of contents, signed, and certified with DocMDP `/P` when given.
fn untagged(certified: Option<u8>) -> Spec {
    let mut s = page(leader_page(700, &["3", "5", "7", "9"]));
    s.cat = "/AcroForm <</SigFlags 3 /Fields [120 0 R]>>".into();
    s.objs.push((120, SIGNATURE.into()));
    if let Some(p) = certified {
        s.cat.push_str(&format!(" /Perms <</DocMDP <</Type/Sig /Reference [<</TransformMethod/DocMDP /TransformParams <</P {p}>>>>]>>>>"));
    }
    s
}

#[test]
fn autotag_refuses_a_certified_untagged_document_and_adds_no_step() {
    for p in 1..=3 {
        let path = file(&format!("toc-certified-{p}"), &untagged(Some(p)));
        let original = std::fs::read(&path).unwrap();
        let (eng, id) = open(&path);
        assert!(check_eng(&eng, id).certified, "P {p}");
        let steps = eng.history(id).unwrap().len();
        match fix_doc(&eng, id, &Fix::Autotag, EditSource::User) {
            Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
            other => panic!("P {p}: expected a refusal, got {other:?}"),
        }
        assert_eq!(eng.history(id).unwrap().len(), steps);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[test]
fn autotag_on_a_signed_untagged_document_is_saved_after_the_signed_bytes() {
    let path = file("toc-signed", &untagged(None));
    let original = std::fs::read(&path).unwrap();
    let (eng, id) = open(&path);
    let r = check_eng(&eng, id);
    assert!(r.signed && !r.certified);
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "an incremental save: the signed bytes are the start of the file");
    let (eng2, id2) = open(&path);
    let r2 = check_eng(&eng2, id2);
    assert!(r2.signed && r2.tagged);
}
