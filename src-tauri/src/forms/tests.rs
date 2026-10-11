//! Forms through the edit lane: every field type, filling, formats and calculations, authoring,
//! one undo step each, saved, reopened with a fresh MuPDF and read back.

use super::af::{CalcOp, Calculate, Format, Validate};
use super::*;
use crate::engine::fixture::{self, Fixture};
use std::path::PathBuf;
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("forms-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open_bytes(test: &str, bytes: Vec<u8>) -> (Arc<Engine>, DocId, String) {
    let d = dir(test);
    let p = d.join("a.pdf");
    std::fs::write(&p, bytes).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

fn open(test: &str) -> (Arc<Engine>, DocId, String) {
    open_bytes(test, fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() }))
}

fn reopen(path: &str) -> (Arc<Engine>, DocId) {
    let e = Engine::new(2, 64 << 20);
    let i = e.open(path).unwrap().doc;
    (e, i)
}

fn nf(kind: FieldKind, name: &str, rect: [f32; 4]) -> NewField {
    NewField { kind, rect, name: Some(name.into()), group: None, export: None, caption: None, options: None }
}

fn add1(eng: &Engine, id: DocId, page: u32, what: NewField) -> i32 {
    add_one(eng, id, page, &what, EditSource::User).unwrap().1
}

fn fill(eng: &Engine, id: DocId, name: &str, v: FieldValue) -> Result<EditState> {
    let f = list_all(eng, id).unwrap();
    let w = f.fields.iter().find(|w| w.name == name).unwrap_or_else(|| panic!("no field {name}"));
    set_value_one(eng, id, w.page, w.xref, &v, EditSource::User)
}

fn text(s: &str) -> FieldValue {
    FieldValue::Text(s.into())
}

fn field(eng: &Engine, id: DocId, name: &str) -> FieldDto {
    list_all(eng, id).unwrap().fields.into_iter().find(|w| w.name == name).unwrap_or_else(|| panic!("no field {name}"))
}

/// The decoded appearance stream (`/AP /N`, or the `on` state of a button) of a widget.
fn ap_text(path: &str, name: &str, state: Option<&str>) -> String {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    for p in 0..doc.page_count().unwrap() {
        let page = doc.load_pdf_page(p).unwrap();
        for w in page.widgets() {
            if w.name().unwrap().as_deref() != Some(name) {
                continue;
            }
            let mut n = w.annotation().object().get_dict("AP").unwrap().expect("no /AP").get_dict("N").unwrap().expect("no /AP /N");
            if let Some(s) = state {
                n = n.get_dict(s).unwrap().unwrap_or_else(|| panic!("no state {s}"));
            }
            return String::from_utf8_lossy(&n.read_stream().unwrap()).into_owned();
        }
    }
    panic!("no widget {name}")
}

#[test]
fn every_field_type_is_one_undo_step_and_survives_save_and_reopen() {
    let (eng, id, path) = open("types");
    let opts = vec![Opt { export: "se".into(), label: "Sweden".into() }, Opt { export: "no".into(), label: "Norway".into() }];
    let all = vec![
        nf(FieldKind::Text, "Full name", [72.0, 100.0, 272.0, 120.0]),
        nf(FieldKind::Checkbox, "Agree", [72.0, 140.0, 86.0, 154.0]),
        nf(FieldKind::Radio, "Contact", [72.0, 170.0, 86.0, 184.0]),
        NewField { group: Some("Contact".into()), name: None, ..nf(FieldKind::Radio, "", [120.0, 170.0, 134.0, 184.0]) },
        NewField { options: Some(opts.clone()), ..nf(FieldKind::Combo, "Country", [72.0, 200.0, 220.0, 220.0]) },
        NewField { options: Some(opts), ..nf(FieldKind::List, "Pick", [72.0, 230.0, 220.0, 290.0]) },
        NewField { caption: Some("Reset".into()), ..nf(FieldKind::Button, "Reset", [72.0, 300.0, 150.0, 324.0]) },
        nf(FieldKind::Signature, "Signature", [72.0, 340.0, 272.0, 380.0]),
    ];
    let base = eng.edit_state(id).unwrap().rev;
    for (i, what) in all.iter().enumerate() {
        let before = eng.history(id).unwrap().len();
        add1(&eng, id, 0, what.clone());
        assert_eq!(eng.history(id).unwrap().len(), before + 1, "one step for {what:?}");
        assert_eq!(list_all(&eng, id).unwrap().fields.len(), i + 1);
    }
    assert_eq!(eng.edit_state(id).unwrap().rev, base + all.len() as u32);
    let kinds = |f: &FormDto| f.fields.iter().map(|w| w.kind).collect::<Vec<_>>();
    let want = vec![
        FieldKind::Text,
        FieldKind::Checkbox,
        FieldKind::Radio,
        FieldKind::Radio,
        FieldKind::Combo,
        FieldKind::List,
        FieldKind::Button,
        FieldKind::Signature,
    ];
    assert_eq!(kinds(&list_all(&eng, id).unwrap()), want);
    // The two radio buttons are one field with two exports.
    let f = list_all(&eng, id).unwrap();
    assert_eq!(f.fields[2].node, f.fields[3].node);
    assert_ne!(f.fields[2].export, f.fields[3].export);
    assert_eq!(f.fields[2].name, "Contact");
    assert_eq!(f.fields[4].options.len(), 2);

    eng.undo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().fields.len(), all.len() - 1);
    eng.redo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().fields.len(), all.len());

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let back = list_all(&eng2, id2).unwrap();
    assert_eq!(kinds(&back), want);
    for (a, b) in back.fields.iter().zip(&f.fields) {
        for i in 0..4 {
            assert!((a.rect[i] - b.rect[i]).abs() < 0.5, "{} rect {:?} vs {:?}", a.name, a.rect, b.rect);
        }
    }
    assert_eq!(back.fields[4].options, f.fields[4].options);
    assert_eq!(back.fields[6].caption, "Reset");

    // A fresh MuPDF sees every widget with an appearance stream.
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let mut n = 0;
    for w in page.widgets() {
        n += 1;
        assert!(w.annotation().object().get_dict("AP").unwrap().is_some(), "widget {n} has no /AP");
    }
    assert_eq!(n, all.len());
}

#[test]
fn filling_every_type_writes_values_and_appearances() {
    let (eng, id, path) = open("fill");
    let opts = vec![Opt { export: "se".into(), label: "Sweden".into() }, Opt { export: "no".into(), label: "Norway".into() }];
    add1(&eng, id, 0, nf(FieldKind::Text, "Full name", [72.0, 100.0, 272.0, 120.0]));
    add1(&eng, id, 0, nf(FieldKind::Checkbox, "Agree", [72.0, 140.0, 86.0, 154.0]));
    add1(&eng, id, 0, nf(FieldKind::Radio, "Contact", [72.0, 170.0, 86.0, 184.0]));
    add1(&eng, id, 0, NewField { group: Some("Contact".into()), ..nf(FieldKind::Radio, "", [120.0, 170.0, 134.0, 184.0]) });
    add1(&eng, id, 0, NewField { options: Some(opts), ..nf(FieldKind::Combo, "Country", [72.0, 200.0, 220.0, 220.0]) });

    let steps = eng.history(id).unwrap().len();
    fill(&eng, id, "Full name", text("Alice Åkesson")).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    assert_eq!(field(&eng, id, "Full name").value, "Alice Åkesson");
    fill(&eng, id, "Country", text("no")).unwrap();
    fill(&eng, id, "Agree", FieldValue::Checked(true)).unwrap();
    assert!(field(&eng, id, "Agree").checked);

    // Radio: choosing the second button turns the first off.
    let f = list_all(&eng, id).unwrap();
    let radios: Vec<&FieldDto> = f.fields.iter().filter(|w| w.name == "Contact").collect();
    assert_eq!(radios.len(), 2);
    set_value_one(&eng, id, 0, radios[0].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    set_value_one(&eng, id, 0, radios[1].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    let f = list_all(&eng, id).unwrap();
    let on: Vec<bool> = f.fields.iter().filter(|w| w.name == "Contact").map(|w| w.checked).collect();
    assert_eq!(on, vec![false, true]);

    // Undo takes one fill back.
    eng.undo(id).unwrap();
    assert!(field(&eng, id, "Contact").checked);
    eng.redo(id).unwrap();

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let back = list_all(&eng2, id2).unwrap();
    let get = |n: &str| back.fields.iter().find(|w| w.name == n).unwrap();
    assert_eq!(get("Full name").value, "Alice Åkesson");
    assert_eq!(get("Country").value, "no");
    assert!(get("Agree").checked);
    // Other viewers show the values: the appearance stream carries the text.
    assert!(ap_text(&path, "Full name", None).contains("Alice"), "text appearance: {}", ap_text(&path, "Full name", None));
}

#[test]
fn formats_validation_and_calculation_run_without_a_script_engine() {
    let (eng, id, path) = open("calc");
    add1(&eng, id, 0, nf(FieldKind::Text, "Rent", [72.0, 100.0, 200.0, 120.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Deposit", [72.0, 130.0, 200.0, 150.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Total", [72.0, 160.0, 200.0, 180.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Born", [72.0, 190.0, 200.0, 210.0]));
    let patch = |p: FieldPatch| p;
    let edit = |name: &str, p: FieldPatch| {
        let w = field(&eng, id, name);
        edit_one(&eng, id, w.page, w.xref, &patch(p), EditSource::User).unwrap();
    };
    edit("Rent", FieldPatch { validate: Some(Validate::Range { min: Some(0.0), max: Some(100000.0) }), ..Default::default() });
    edit(
        "Total",
        FieldPatch {
            format: Some(Format::Number { decimals: 2, sep: 0, neg: 0, currency: String::new(), prepend: true }),
            calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Rent".into(), "Deposit".into()] }),
            read_only: Some(true),
            ..Default::default()
        },
    );
    edit("Born", FieldPatch { format: Some(Format::Date("yyyy-mm-dd".into())), ..Default::default() });

    // The scripts are what Acrobat writes, and read back as the same settings.
    let t = field(&eng, id, "Total");
    assert!(matches!(t.format, Format::Number { decimals: 2, .. }));
    assert_eq!(t.calculate, Calculate::Simple { op: CalcOp::Sum, fields: vec!["Rent".into(), "Deposit".into()] });
    assert!(t.read_only);

    fill(&eng, id, "Rent", text("12 500")).unwrap();
    fill(&eng, id, "Deposit", text("1500.5")).unwrap();
    let t = field(&eng, id, "Total");
    assert_eq!(t.value, "14000.5", "the raw value is kept");
    assert_eq!(t.shown, "14,000.50", "the appearance shows the format");

    // Rejected values leave the document and the history alone.
    let steps = eng.history(id).unwrap().len();
    let e = fill(&eng, id, "Rent", text("9999999")).unwrap_err().to_string();
    assert!(e.contains("less than or equal to 100000"), "{e}");
    let e = fill(&eng, id, "Born", text("not a date")).unwrap_err().to_string();
    assert!(e.contains("Invalid date"), "{e}");
    assert!(fill(&eng, id, "Total", text("1")).is_err(), "read-only");
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert_eq!(field(&eng, id, "Rent").value, "12 500", "what was typed is kept");

    fill(&eng, id, "Born", text("1991-04-18")).unwrap();
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let t = field(&eng2, id2, "Total");
    assert_eq!(t.value, "14000.5");
    assert!(matches!(t.calculate, Calculate::Simple { .. }));
    assert!(ap_text(&path, "Total", None).contains("14,000.50"), "{}", ap_text(&path, "Total", None));
}

#[test]
fn clear_form_and_reset_fields() {
    let (eng, id, _) = open("reset");
    add1(&eng, id, 0, nf(FieldKind::Text, "A", [72.0, 100.0, 200.0, 120.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "B", [72.0, 130.0, 200.0, 150.0]));
    add1(&eng, id, 0, nf(FieldKind::Checkbox, "C", [72.0, 160.0, 86.0, 174.0]));
    let a = field(&eng, id, "A");
    edit_one(&eng, id, 0, a.xref, &FieldPatch { default_value: Some("start".into()), ..Default::default() }, EditSource::User).unwrap();
    fill(&eng, id, "A", text("changed")).unwrap();
    fill(&eng, id, "B", text("b")).unwrap();
    fill(&eng, id, "C", FieldValue::Checked(true)).unwrap();

    let steps = eng.history(id).unwrap().len();
    reset_one(&eng, id, None, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1, "clear form is one step");
    assert_eq!(field(&eng, id, "A").value, "start", "back to the default value");
    assert_eq!(field(&eng, id, "B").value, "");
    assert!(!field(&eng, id, "C").checked);

    eng.undo(id).unwrap();
    assert_eq!(field(&eng, id, "B").value, "b");
    assert!(field(&eng, id, "C").checked);

    // A listed field only.
    let b = field(&eng, id, "B");
    reset_one(&eng, id, Some(vec![b.xref]), EditSource::User).unwrap();
    assert_eq!(field(&eng, id, "B").value, "");
    assert_eq!(field(&eng, id, "A").value, "changed");
}

#[test]
fn fields_move_resize_delete_and_tab_order() {
    let (eng, id, path) = open("author");
    let a = add1(&eng, id, 0, nf(FieldKind::Text, "A", [300.0, 100.0, 400.0, 120.0]));
    let b = add1(&eng, id, 0, nf(FieldKind::Text, "B", [72.0, 100.0, 172.0, 120.0]));
    let c = add1(&eng, id, 0, nf(FieldKind::Text, "C", [72.0, 200.0, 172.0, 220.0]));
    let order = |eng: &Engine| list_all(eng, id).unwrap().fields.iter().map(|w| w.name.clone()).collect::<Vec<_>>();
    assert_eq!(order(&eng), ["A", "B", "C"], "creation order is the tab order");

    // Row order: B and A share a row, B is left of A.
    arrange_one(&eng, id, 0, true, EditSource::User).unwrap();
    assert_eq!(order(&eng), ["B", "A", "C"]);
    tab_order_one(&eng, id, 0, &[c, a, b], EditSource::User).unwrap();
    assert_eq!(order(&eng), ["C", "A", "B"]);
    eng.undo(id).unwrap();
    assert_eq!(order(&eng), ["B", "A", "C"]);

    move_one(&eng, id, 0, a, [100.0, 300.0, 260.0, 330.0], EditSource::User).unwrap();
    let w = field(&eng, id, "A");
    for (got, want) in w.rect.iter().zip([100.0, 300.0, 260.0, 330.0]) {
        assert!((got - want).abs() < 0.5, "{:?}", w.rect);
    }
    assert!(move_one(&eng, id, 0, a, [10.0, 10.0, 11.0, 11.0], EditSource::User).is_err());

    // Properties: rename, required, tooltip, multi-line, max length, font size.
    edit_one(
        &eng,
        id,
        0,
        b,
        &FieldPatch {
            name: Some("Notes".into()),
            required: Some(true),
            tooltip: Some("Anything to add".into()),
            multiline: Some(true),
            max_len: Some(40),
            font_size: Some(14.0),
            ..Default::default()
        },
        EditSource::User,
    )
    .unwrap();
    let n = field(&eng, id, "Notes");
    assert!(n.required && n.multiline);
    assert_eq!((n.max_len, n.font_size, n.tooltip.as_str()), (40, 14.0, "Anything to add"));
    let e = edit_one(&eng, id, 0, b, &FieldPatch { name: Some("C".into()), ..Default::default() }, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("already exists"), "{e}");
    let e = fill(&eng, id, "Notes", text(&"x".repeat(41))).unwrap_err().to_string();
    assert!(e.contains("at most 40"), "{e}");

    delete_many(&eng, id, &[(0, a)], EditSource::User).unwrap();
    assert_eq!(order(&eng), ["Notes", "C"]);
    eng.undo(id).unwrap();
    assert_eq!(order(&eng).len(), 3);
    eng.redo(id).unwrap();

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let back = list_all(&eng2, id2).unwrap();
    assert_eq!(back.fields.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Notes", "C"]);
    assert!(back.fields[0].required && back.fields[0].multiline);
    // The form's field list agrees with the page: the deleted field is gone from both.
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let fields = doc.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap().get_dict("Fields").unwrap().unwrap();
    assert_eq!(fields.len().unwrap(), 2);
}

/// A printed form with nothing fillable: a label with a run of underscores, a rule under a
/// label, a small square with a label, and an empty box under a heading.
fn flat_form() -> Vec<u8> {
    let content = "BT /F1 12 Tf 48 700 Td (Full name:) Tj ET\n\
                   BT /F1 12 Tf 130 700 Td (____________________) Tj ET\n\
                   BT /F1 12 Tf 48 660 Td (Email) Tj ET\n\
                   0 G 130 656 m 400 656 l S\n\
                   0 G 48 600 12 12 re S\n\
                   BT /F1 12 Tf 66 602 Td (I agree to the terms) Tj ET\n\
                   BT /F1 12 Tf 48 548 Td (Notes) Tj ET\n\
                   0 G 48 480 300 60 re S\n\
                   BT /F1 12 Tf 48 420 Td (Signature:) Tj ET\n\
                   0 G 130 416 m 400 416 l S\n\
                   BT /F1 12 Tf 48 380 Td (Signed) Tj ET\n\
                   0 G 130 376 m 400 376 l S\n\
                   BT /F1 12 Tf 48 340 Td (Design) Tj ET\n\
                   0 G 130 336 m 400 336 l S\n";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
    ];
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

#[test]
fn detect_fields_on_a_flat_form_then_add_them_in_one_step() {
    let (eng, id, path) = open_bytes("detect", flat_form());
    assert!(list_all(&eng, id).unwrap().fields.is_empty());
    let before = eng.history(id).unwrap().len();
    let found = detect_one(&eng, id, 0).unwrap();
    let names: Vec<&str> = found.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"Full name"), "{names:?}");
    assert!(names.contains(&"Email"), "{names:?}");
    assert!(names.contains(&"I agree to the terms"), "{names:?}");
    assert!(names.contains(&"Notes"), "{names:?}");
    let agree = found.iter().find(|d| d.name == "I agree to the terms").unwrap();
    assert_eq!(agree.kind, FieldKind::Checkbox);
    // A blank labelled "Signature" or "Signed" is a signature field, as in Acrobat; "Design" is not one.
    let kind_of = |n: &str| found.iter().find(|d| d.name == n).unwrap_or_else(|| panic!("{n} missing from {names:?}")).kind;
    assert_eq!(kind_of("Signature"), FieldKind::Signature);
    assert_eq!(kind_of("Signed"), FieldKind::Signature);
    assert_eq!(kind_of("Design"), FieldKind::Text);
    assert_eq!(kind_of("Full name"), FieldKind::Text);
    // Page space: the name field sits at the label's line (y 700 up = 92 down).
    let full = found.iter().find(|d| d.name == "Full name").unwrap();
    assert!(full.rect[0] > 120.0 && full.rect[1] > 70.0 && full.rect[1] < 110.0, "{:?}", full.rect);
    // Detection changes nothing.
    assert_eq!(eng.history(id).unwrap().len(), before);

    let items: Vec<NewField> = found
        .iter()
        .map(|d| NewField { kind: d.kind, rect: d.rect, name: Some(d.name.clone()), group: None, export: None, caption: None, options: None })
        .collect();
    add_many(&eng, id, 0, &items, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), before + 1, "all of them are one undo step");
    let listed = list_all(&eng, id).unwrap();
    assert_eq!(listed.fields.len(), found.len());
    assert_eq!(listed.fields.iter().filter(|f| f.kind == FieldKind::Signature).count(), 2);
    // Nothing more to find once they exist.
    assert!(detect_one(&eng, id, 0).unwrap().is_empty());
    eng.undo(id).unwrap();
    assert!(list_all(&eng, id).unwrap().fields.is_empty());
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(list_all(&eng2, id2).unwrap().fields.len(), found.len());
}

fn study_sample(name: &str) -> Option<Vec<u8>> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/study/samples").join(name)).ok()
}

/// The study sample Acrobat's auto-detection was recorded on: the labels it found are found here too.
#[test]
fn detection_finds_what_acrobat_found_on_the_study_flat_form() {
    let Some(bytes) = study_sample("flatform-source.pdf") else { return };
    let (eng, id, _) = open_bytes("study-flat", bytes);
    let found = detect_one(&eng, id, 0).unwrap();
    let names: Vec<String> = found.iter().map(|d| d.name.clone()).collect();
    for want in ["Full name", "Email", "Date of birth", "I agree to the terms", "Send me updates"] {
        assert!(names.iter().any(|n| n == want), "{want} missing from {names:?}");
    }
    assert!(found.iter().filter(|d| d.kind == FieldKind::Checkbox).count() >= 2, "{names:?}");
    // Its "Signature:" blank is a signature field, not a text field.
    assert!(found.iter().any(|d| d.kind == FieldKind::Signature), "{names:?}");
}

/// The study sample that already has widgets: every field is listed, filled and read back after a save.
#[test]
fn the_study_form_lists_fills_and_round_trips() {
    let Some(bytes) = study_sample("form-source.pdf") else { return };
    let (eng, id, path) = open_bytes("study-form", bytes);
    let before = list_all(&eng, id).unwrap();
    assert!(!before.fields.is_empty());
    let steps = eng.history(id).unwrap().len();
    let mut filled = 0;
    for f in before.fields.iter().filter(|f| f.kind == FieldKind::Text && !f.read_only) {
        set_value_one(&eng, id, f.page, f.xref, &text("Alex"), EditSource::User).unwrap();
        filled += 1;
    }
    assert!(filled > 0);
    let combo = before.fields.iter().find(|f| f.kind == FieldKind::Combo).expect("a drop-down");
    let choice = combo.options.last().unwrap().export.clone();
    set_value_one(&eng, id, combo.page, combo.xref, &text(&choice), EditSource::User).unwrap();
    let boxed = before.fields.iter().find(|f| f.kind == FieldKind::Checkbox).expect("a check box");
    set_value_one(&eng, id, boxed.page, boxed.xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    let radios: Vec<&FieldDto> = before.fields.iter().filter(|f| f.kind == FieldKind::Radio).collect();
    set_value_one(&eng, id, radios[1].page, radios[1].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + filled + 3, "one step per fill");

    // Radio export values are decoded for display (pdfTeX writes them as escaped UTF-16).
    assert!(radios.iter().all(|r| !r.export.contains('\u{5c}')), "{:?}", radios.iter().map(|r| &r.export).collect::<Vec<_>>());

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let after = list_all(&eng2, id2).unwrap();
    assert_eq!(after.fields.len(), before.fields.len());
    assert!(after.fields.iter().filter(|f| f.kind == FieldKind::Text && !f.read_only).all(|f| f.value == "Alex"));
    assert_eq!(after.fields.iter().find(|f| f.kind == FieldKind::Combo).unwrap().values, vec![choice]);
    assert!(after.fields.iter().find(|f| f.kind == FieldKind::Checkbox).unwrap().checked);
    let r: Vec<bool> = after.fields.iter().filter(|f| f.kind == FieldKind::Radio).map(|f| f.checked).collect();
    assert_eq!(r, vec![false, true]);
}

#[test]
fn pdftex_export_names_are_shown_decoded() {
    let bs = '\u{5c}';
    let name = |tail: &str| format!("{bs}376{bs}377{tail}");
    assert_eq!(shown_name(&name(&format!("{bs}000e"))), "e");
    assert_eq!(shown_name(&name(&format!("{bs}000P{bs}000r{bs}000o"))), "Pro");
    assert_eq!(shown_name("Yes"), "Yes");
    let odd = name(&format!("{bs}000"));
    assert_eq!(shown_name(&odd), odd);
}
