//! Autotag tables: what becomes Table > TR > TH/TD, and what must stay paragraphs.

use super::tests::{Spec, check_eng, file, open};
use super::*;

/// One text object: `(t)` in font `f` (F1 regular, F2 bold) at `(x, y)`.
fn at(f: &str, size: i32, x: i32, y: i32, t: &str) -> String {
    format!("BT /{f} {size} Tf {x} {y} Td ({t}) Tj ET\n")
}

/// A grid drawn row by row: the first row in bold when `bold_header`.
fn grid(rows: &[&[&str]], xs: &[i32], top: i32, pitch: i32, bold_header: bool) -> String {
    let mut c = String::new();
    for (r, row) in rows.iter().enumerate() {
        for (cell, x) in row.iter().zip(xs) {
            c += &at(if r == 0 && bold_header { "F2" } else { "F1" }, 12, *x, top - r as i32 * pitch, cell);
        }
    }
    c
}

fn page(content: String) -> Spec {
    Spec { pages: vec![(String::new(), content)], ..Spec::default() }
}

const SALES: [&[&str]; 3] = [&["Region", "Units", "Revenue"], &["North", "120", "4 800"], &["South", "95", "3 750"]];

struct Tagged {
    types: Vec<String>,
    report: Report,
    tree: Tree,
}

impl Tagged {
    fn count(&self, ty: &str) -> usize {
        self.types.iter().filter(|t| *t == ty).count()
    }
}

/// Runs Autotag on `s`, checks that the page looks and extracts the same, saves, reopens and
/// reads the tree and the report back.
fn autotag(test: &str, s: &Spec) -> Tagged {
    let path = file(test, s);
    let (eng, id) = open(&path);
    let before: Vec<String> = eng.page_text(id, 0).unwrap().into_iter().map(|l| l.t).collect();
    let png = eng.page_png(id, eng.edit_state(id).unwrap().rev, 0, 200).unwrap();
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    let after: Vec<String> = eng.page_text(id, 0).unwrap().into_iter().map(|l| l.t).collect();
    assert_eq!(after, before, "the text is unchanged");
    assert_eq!(png, eng.page_png(id, eng.edit_state(id).unwrap().rev, 0, 200).unwrap(), "the page looks the same");
    eng.save(id, None).unwrap();
    eng.close(id);
    let doc = PdfDocument::open(&path).unwrap();
    let tree = Tree::read(&doc, &tree::page_index(&doc).unwrap()).unwrap();
    let types = tree.preorder().into_iter().map(|i| tree.elems[i].ty.clone()).collect();
    let (eng2, id2) = open(&path);
    let report = check_eng(&eng2, id2);
    for rule in [Rule::TaggedPdf, Rule::ReadingOrder, Rule::TableStructure, Rule::TableScope, Rule::ListStructure] {
        assert_eq!(report.status(rule), Status::Passed, "{test}: {rule:?}: {:?}", report.result(rule).findings);
    }
    Tagged { types, report, tree }
}

fn row(n: usize, header: bool) -> Vec<&'static str> {
    let mut v = vec!["TR"];
    v.extend(std::iter::repeat_n(if header { "TH" } else { "TD" }, n));
    v
}

#[test]
fn a_bold_header_row_makes_a_table_with_th_and_td() {
    let mut c = at("F1", 12, 72, 740, "Sales by region.");
    c += &grid(&SALES, &[72, 200, 330], 700, 18, true);
    c += &at("F1", 12, 72, 560, "Figures are in thousands of SEK.");
    let t = autotag("table-bold", &page(c));
    let mut want = vec!["Document", "P", "Table"];
    for r in 0..3 {
        want.extend(row(3, r == 0));
    }
    want.push("P");
    assert_eq!(t.types, want, "{:?}", t.types);
    // Every header cell says what it heads, so Matterhorn 15-005 holds and the Tags panel shows it.
    let th: Vec<_> = (0..t.tree.elems.len()).filter(|&i| t.tree.elems[i].ty == "TH").collect();
    assert_eq!(th.len(), 3);
    assert!(th.iter().all(|&i| t.tree.elems[i].attrs.get("Scope").is_some_and(|s| s.contains("Column"))));
    assert_eq!(t.report.status(Rule::TableHeaders), Status::Passed, "{:?}", t.report.result(Rule::TableHeaders).findings);
}

#[test]
fn a_header_row_set_larger_or_under_a_rule_is_a_header() {
    // Larger.
    let mut c = String::new();
    for (x, h) in [(72, "Item"), (250, "Count")] {
        c += &at("F1", 16, x, 700, h);
    }
    c += &grid(&[&["Pens", "12"], &["Ink", "4"]], &[72, 250], 676, 18, false);
    let t = autotag("table-larger", &page(c));
    assert_eq!(t.types, ["Document", "Table", "TR", "TH", "TH", "TR", "TD", "TD", "TR", "TD", "TD"].map(String::from));

    // Ruled: same sizes and fonts, a rule between the first two rows (and a bottom one). The
    // rules stay artifacts.
    let mut c = grid(&[&["Item", "Count"], &["Pens", "12"], &["Ink", "4"]], &[72, 250], 700, 20, false);
    c += "0 g 72 694 m 330 694 l S\n72 650 258 0.8 re f\n";
    let t = autotag("table-ruled", &page(c));
    assert_eq!(t.types, ["Document", "Table", "TR", "TH", "TH", "TR", "TD", "TD", "TR", "TD", "TD"].map(String::from));
    assert_eq!(t.report.status(Rule::TableHeaders), Status::Passed);
}

#[test]
fn a_plain_grid_needs_three_rows_and_three_columns_and_then_has_no_header() {
    let t = autotag("table-plain", &page(grid(&SALES, &[72, 200, 330], 700, 18, false)));
    assert_eq!(t.count("Table"), 1, "{:?}", t.types);
    assert_eq!(t.count("TD"), 9);
    assert_eq!(t.count("TH"), 0);
    // The checker asks for the header, which the Tags panel can set.
    assert_eq!(t.report.status(Rule::TableHeaders), Status::Failed);

    // Two columns without a rule or a header row: pairs, not a table.
    let pairs = grid(&[&["Alpha", "1"], &["Beta", "2"], &["Gamma", "3"], &["Delta", "4"]], &[72, 250], 700, 18, false);
    assert_eq!(autotag("table-pairs", &page(pairs)).count("Table"), 0);
    // Three columns, two rows, nothing to confirm it.
    let small = grid(&[&["a", "b", "c"], &["d", "e", "f"]], &[72, 200, 330], 700, 18, false);
    assert_eq!(autotag("table-small", &page(small)).count("Table"), 0);
}

#[test]
fn right_aligned_numbers_under_a_left_aligned_header_still_line_up() {
    // Header text starts at the column's left edge; the numbers end at its right edge.
    let mut c = at("F2", 12, 72, 700, "Name");
    c += &at("F2", 12, 300, 700, "Amount");
    for (i, (n, a, x)) in [("Rent", "9 500", 336), ("Power", "420", 348), ("Water", "75", 354)].iter().enumerate() {
        c += &at("F1", 12, 72, 682 - 18 * i as i32, n);
        c += &at("F1", 12, *x, 682 - 18 * i as i32, a);
    }
    let t = autotag("table-right", &page(c));
    assert_eq!(t.count("Table"), 1, "{:?}", t.types);
    assert_eq!(t.count("TH"), 2);
}

#[test]
fn two_column_prose_stays_paragraphs_even_with_a_rule_across_it() {
    let left = ["Sales grew in every region this year,", "led by the north where two new offices", "opened in the spring and quickly reached", "the volumes of the older sites in the south.", "The board thanks every employee for the", "work they did through a long and busy year."];
    let right = ["Next year the company expects to open", "three more offices and to hire about forty", "people, mostly in product and support, while", "keeping the cost of each sale about level", "with this year thanks to the new tools that", "the sales teams have started to use already."];
    let mut c = String::from("0 g 72 737 468 0.8 re f\n");
    for i in 0..6 {
        c += &at("F1", 10, 72, 730 - 14 * i, left[i as usize]);
        c += &at("F1", 10, 320, 730 - 14 * i, right[i as usize]);
    }
    let t = autotag("table-article", &page(c));
    assert_eq!(t.count("Table"), 0, "{:?}", t.types);
    // The columns are one block in draw order (a P), as before: nothing in it is a cell.
    assert_eq!(t.count("P"), 1, "{:?}", t.types);
}

#[test]
fn lists_stay_lists_and_detached_markers_are_not_a_table() {
    let mut c = String::new();
    for i in 0..4 {
        c += &at("F1", 12, 72, 700 - 16 * i, &format!("- Item number {i}"));
    }
    let t = autotag("table-list", &page(c));
    assert_eq!((t.count("L"), t.count("LI"), t.count("Table")), (1, 4, 0), "{:?}", t.types);

    // The marker and the text are separate text objects at a hanging indent: two cells per row,
    // and a rule over them that would confirm a table. Only the column of markers rules it out.
    let mut c = String::from("0 g 72 712 300 0.8 re f\n");
    for i in 0..4 {
        c += &at("F1", 12, 72, 700 - 16 * i, &format!("{}.", i + 1));
        c += &at("F1", 12, 130, 700 - 16 * i, &format!("Step {i}"));
    }
    assert_eq!(autotag("table-markers", &page(c)).count("Table"), 0);
}

#[test]
fn a_form_is_not_a_table() {
    // Labels with colons: three columns and three rows, but no header and no rules.
    let rows: [&[&str]; 3] = [&["Name:", "Email:", "Phone:"], &["City:", "Street:", "Zip:"], &["Born:", "Role:", "Team:"]];
    let t = autotag("table-form-colons", &page(grid(&rows, &[72, 220, 370], 700, 28, false)));
    assert_eq!(t.count("Table"), 0, "{:?}", t.types);

    // Labels without colons under a bold row, with a field over the second row: the field keeps
    // its text out of a table.
    let rows: [&[&str]; 3] = [&["Name", "Email", "Phone"], &["City", "Street", "Country"], &["Born", "Role", "Team"]];
    let mut s = page(grid(&rows, &[72, 220, 370], 700, 28, true));
    s.pages[0].0 = "/Annots [120 0 R]".into();
    s.objs.push((120, "<</Type/Annot /Subtype/Widget /FT/Tx /T(city) /Rect [72 650 190 666]>>".into()));
    let t = autotag("table-form-widget", &s);
    assert_eq!(t.count("Table"), 0, "{:?}", t.types);
    // The same grid without the field is a table: the field is what stopped it.
    assert_eq!(autotag("table-form-none", &page(grid(&rows, &[72, 220, 370], 700, 28, true))).count("Table"), 1);
}

#[test]
fn an_irregular_table_falls_back_to_paragraphs() {
    // A cell is missing in the middle row.
    let mut c = grid(&[&["Region", "Units", "Revenue"]], &[72, 200, 330], 700, 18, true);
    c += &at("F1", 12, 72, 682, "North");
    c += &at("F1", 12, 330, 682, "4 800");
    c += &grid(&[&["South", "95", "3 750"]], &[72, 200, 330], 664, 18, false);
    let t = autotag("table-missing", &page(c));
    assert_eq!(t.count("Table"), 0, "{:?}", t.types);

    // A spanning row in the middle.
    let mut c = grid(&[&["Region", "Units", "Revenue"]], &[72, 200, 330], 700, 18, true);
    c += &at("F2", 12, 72, 682, "Northern sites");
    c += &grid(&[&["Lund", "95", "3 750"], &["Malmo", "80", "3 100"]], &[72, 200, 330], 664, 18, false);
    let t = autotag("table-span", &page(c));
    assert_eq!(t.count("Table"), 0, "{:?}", t.types);

    // A cell that wraps onto a second line.
    let mut c = grid(&[&["Item", "Notes", "Qty"], &["Pens", "Blue ink", "12"]], &[72, 200, 400], 700, 18, true);
    c += &at("F1", 12, 200, 668, "refills sold apart");
    c += &grid(&[&["Ink", "Black", "4"]], &[72, 200, 400], 650, 18, false);
    assert_eq!(autotag("table-wrap", &page(c)).count("Table"), 0);
}

#[test]
fn cells_drawn_column_by_column_stay_paragraphs() {
    // Tag order would be row by row against a content order of column by column.
    let mut c = String::new();
    for (j, x) in [72, 200, 330].into_iter().enumerate() {
        for (i, r) in SALES.iter().enumerate() {
            c += &at(if i == 0 { "F2" } else { "F1" }, 12, x, 700 - 18 * i as i32, r[j]);
        }
    }
    let t = autotag("table-columns", &page(c));
    assert_eq!(t.count("Table"), 0, "{:?}", t.types);
}

#[test]
fn a_table_header_is_not_a_heading() {
    // The table's 18 pt header row is larger than the 16 pt "Notes" heading. It is a table row, so
    // it takes no heading level: the 24 pt title is H1 and "Notes" is H2.
    let mut c = at("F1", 24, 72, 740, "Quarterly figures");
    c += &at("F1", 16, 72, 480, "Notes");
    for (r, row) in SALES.iter().enumerate() {
        for (x, t) in [72, 200, 330].iter().zip(row.iter()) {
            c += &at(if r == 0 { "F2" } else { "F1" }, if r == 0 { 18 } else { 12 }, *x, 700 - 22 * r as i32, t);
        }
    }
    c += &at("F1", 12, 72, 460, "All sums are before tax.");
    c += &at("F1", 12, 72, 444, "Rounded to the nearest hundred.");
    let t = autotag("table-headings", &page(c));
    assert_eq!(t.count("Table"), 1, "{:?}", t.types);
    assert_eq!((t.count("H1"), t.count("H2"), t.count("H3")), (1, 1, 0), "{:?}", t.types);
    assert_eq!(t.count("TH"), 3);
    assert_eq!(t.report.status(Rule::HeadingNesting), Status::Passed);
}
