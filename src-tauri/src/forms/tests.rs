//! Forms through the edit lane: every field type, filling, formats and calculations, authoring,
//! one undo step each, saved, reopened with a fresh MuPDF and read back.

use super::af::{ButtonAction, CalcOp, Calculate, Format, Validate};
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

// ---- review round: reset buttons, tab order, rename, export values, shared widgets, no-op fills ----

/// A PDF from object bodies (object `n` is `objs[n - 1]`; object 1 is the catalog).
fn pdf_of(objs: &[String]) -> Vec<u8> {
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

/// A form with fields of several widgets: a check box (`agree`: two widgets that turn on to Yes and one
/// to Oui), a calculated text field (`Sum`, two widgets, in /CO) and a plain one (`Qty`).
fn shared_widgets_form() -> Vec<u8> {
    let state = "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] /Length 0 >>\nstream\n\nendstream".to_string();
    let boxed = |rect: &str, on: &str| {
        format!("<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [{rect}] /F 4 /AS /Off /AP << /N << /{on} 14 0 R /Off 14 0 R >> >> >>")
    };
    let sum = |rect: &str| format!("<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [{rect}] /F 4 /DA (/Helv 0 Tf 0 g) >>");
    pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 10 0 R] /CO [6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /Helv 3 0 R >> >> /Annots [7 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R] >>".to_string(),
        "<< /FT /Btn /T (agree) /Kids [7 0 R 8 0 R 9 0 R] /V /Off >>".to_string(),
        "<< /FT /Tx /T (Sum) /Kids [11 0 R 12 0 R] /AA << /C << /S /JavaScript /JS (AFSimple_Calculate(\"SUM\", new Array(\"Qty\"));) >> >> >>".to_string(),
        boxed("72 700 86 714", "Yes"),
        boxed("100 700 114 714", "Yes"),
        boxed("130 700 144 714", "Oui"),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Qty) /Rect [72 600 200 620] /F 4 /DA (/Helv 0 Tf 0 g) /V (3) >>".to_string(),
        sum("72 500 200 520"),
        sum("72 460 200 480"),
        "null".to_string(),
        state,
    ])
}

fn widgets_named(eng: &Engine, id: DocId, name: &str) -> Vec<FieldDto> {
    list_all(eng, id).unwrap().fields.into_iter().filter(|w| w.name == name).collect()
}

fn calc_order_len(eng: &Engine, id: DocId) -> usize {
    eng.read_custom(id, |d| {
        let form = d.catalog()?.get_dict("AcroForm")?.expect("an AcroForm");
        Ok(form.get_dict("CO")?.map_or(0, |a| a.len().unwrap_or(0)))
    })
    .unwrap()
}

#[test]
fn a_check_box_with_several_widgets_turns_on_every_widget_with_that_state() {
    let (eng, id, path) = open_bytes("shared-check", shared_widgets_form());
    let agree = widgets_named(&eng, id, "agree");
    assert_eq!(agree.len(), 3);
    let on = |eng: &Engine, id| widgets_named(eng, id, "agree").iter().map(|w| w.checked).collect::<Vec<_>>();
    assert_eq!(on(&eng, id), [false, false, false]);

    set_value_one(&eng, id, agree[0].page, agree[0].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    assert_eq!(on(&eng, id), [true, true, false], "both widgets that turn on to Yes");
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(on(&eng2, id2), [true, true, false]);

    // The third widget turns on to a different name: only it is chosen.
    let agree = widgets_named(&eng2, id2, "agree");
    set_value_one(&eng2, id2, agree[2].page, agree[2].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    assert_eq!(on(&eng2, id2), [false, false, true]);
    set_value_one(&eng2, id2, agree[2].page, agree[2].xref, &FieldValue::Checked(false), EditSource::User).unwrap();
    assert_eq!(on(&eng2, id2), [false, false, false]);
}

#[test]
fn a_calculated_field_stays_in_the_calculation_order_until_its_last_widget_goes() {
    let (eng, id, path) = open_bytes("shared-calc", shared_widgets_form());
    assert_eq!(calc_order_len(&eng, id), 1);
    let sum = widgets_named(&eng, id, "Sum");
    assert_eq!(sum.len(), 2);

    delete_many(&eng, id, &[(0, sum[0].xref)], EditSource::User).unwrap();
    assert_eq!(widgets_named(&eng, id, "Sum").len(), 1);
    assert_eq!(calc_order_len(&eng, id), 1, "one widget is left, so the field keeps its place");
    // It still calculates.
    fill(&eng, id, "Qty", text("7")).unwrap();
    assert_eq!(field(&eng, id, "Sum").value, "7");

    delete_many(&eng, id, &[(0, sum[1].xref)], EditSource::User).unwrap();
    assert_eq!(calc_order_len(&eng, id), 0, "the last widget took it out");
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert!(widgets_named(&eng2, id2, "Sum").is_empty());
    assert_eq!(calc_order_len(&eng2, id2), 0);
}

#[test]
fn deleting_every_widget_of_a_field_in_one_step_clears_its_calculation_order_once() {
    let (eng, id, _) = open_bytes("shared-calc-both", shared_widgets_form());
    let sum = widgets_named(&eng, id, "Sum");
    delete_many(&eng, id, &[(0, sum[0].xref), (0, sum[1].xref)], EditSource::User).unwrap();
    assert_eq!(calc_order_len(&eng, id), 0);
}

#[test]
fn an_edit_that_changes_nothing_is_not_an_undo_step() {
    let (eng, id, _) = open("noop");
    add1(&eng, id, 0, nf(FieldKind::Text, "A", [72.0, 100.0, 200.0, 120.0]));
    add1(&eng, id, 0, nf(FieldKind::Radio, "Plan", [72.0, 140.0, 86.0, 154.0]));
    add1(&eng, id, 0, NewField { group: Some("Plan".into()), name: None, ..nf(FieldKind::Radio, "", [120.0, 140.0, 134.0, 154.0]) });
    let steps = || eng.history(id).unwrap().len();
    let radios: Vec<FieldDto> = widgets_named(&eng, id, "Plan");

    // Nothing to reset yet.
    let n = steps();
    reset_one(&eng, id, None, EditSource::User).unwrap();
    assert_eq!(steps(), n, "an empty reset");

    // The first choice is a step; choosing the same one again, or trying to switch a radio group off, is not.
    set_value_one(&eng, id, 0, radios[0].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    assert_eq!(steps(), n + 1);
    set_value_one(&eng, id, 0, radios[0].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    assert_eq!(steps(), n + 1, "the radio re-click");
    set_value_one(&eng, id, 0, radios[0].xref, &FieldValue::Checked(false), EditSource::User).unwrap();
    assert_eq!(steps(), n + 1, "a radio group cannot be switched off");
    assert!(widgets_named(&eng, id, "Plan")[0].checked);
    // Another button is a change.
    set_value_one(&eng, id, 0, radios[1].xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    assert_eq!(steps(), n + 2);
    assert!(widgets_named(&eng, id, "Plan")[1].checked && !widgets_named(&eng, id, "Plan")[0].checked);

    // The same text is not a change either.
    fill(&eng, id, "A", text("Alex")).unwrap();
    assert_eq!(steps(), n + 3);
    fill(&eng, id, "A", text("Alex")).unwrap();
    assert_eq!(steps(), n + 3, "the same text");

    // Resetting one field that is already empty changes nothing; the form as a whole does.
    let plan = widgets_named(&eng, id, "Plan");
    reset_one(&eng, id, Some(vec![plan[0].xref]), EditSource::User).unwrap();
    assert_eq!(steps(), n + 4, "the radio group has a choice to clear");
    reset_one(&eng, id, Some(vec![plan[0].xref]), EditSource::User).unwrap();
    assert_eq!(steps(), n + 4, "and then it has not");
    reset_one(&eng, id, None, EditSource::User).unwrap();
    assert_eq!(steps(), n + 5);
    assert_eq!(field(&eng, id, "A").value, "");
    reset_one(&eng, id, None, EditSource::User).unwrap();
    assert_eq!(steps(), n + 5, "clearing an empty form");
}

#[test]
fn a_tab_order_that_lists_a_field_twice_is_refused_and_changes_nothing() {
    let (eng, id, _) = open("tab-twice");
    for (i, n) in ["a", "b", "c"].into_iter().enumerate() {
        let y = 100.0 + 40.0 * i as f32;
        add1(&eng, id, 0, nf(FieldKind::Text, n, [72.0, y, 200.0, y + 20.0]));
    }
    let names = |eng: &Engine, id| list_all(eng, id).unwrap().fields.into_iter().map(|w| w.name).collect::<Vec<_>>();
    let x = |n: &str| field(&eng, id, n).xref;
    let steps = eng.history(id).unwrap().len();
    let e = tab_order_one(&eng, id, 0, &[x("a"), x("b"), x("a")], EditSource::User).unwrap_err().to_string();
    assert!(e.contains("twice"), "{e}");
    assert_eq!(names(&eng, id), ["a", "b", "c"], "no widget was dropped or moved");
    assert_eq!(eng.history(id).unwrap().len(), steps);
    tab_order_one(&eng, id, 0, &[x("c"), x("a"), x("b")], EditSource::User).unwrap();
    assert_eq!(names(&eng, id), ["c", "a", "b"]);
}

#[test]
fn renaming_checks_the_name_that_is_written_and_follows_calculations() {
    let (eng, id, path) = open("rename");
    add1(&eng, id, 0, nf(FieldKind::Text, "Total", [72.0, 100.0, 200.0, 120.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Tax", [72.0, 130.0, 200.0, 150.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Sum", [72.0, 160.0, 200.0, 180.0]));
    let rename = |from: &str, to: &str| {
        let w = field(&eng, id, from);
        edit_one(&eng, id, w.page, w.xref, &FieldPatch { name: Some(to.into()), ..Default::default() }, EditSource::User)
    };
    let sum = field(&eng, id, "Sum");
    edit_one(
        &eng,
        id,
        sum.page,
        sum.xref,
        &FieldPatch { calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Total".into(), "Tax".into()] }), ..Default::default() },
        EditSource::User,
    )
    .unwrap();

    // A dotted name writes only its last part, and that is what must be free.
    let e = rename("Tax", "a.Total").unwrap_err().to_string();
    assert!(e.contains("already exists"), "{e}");
    assert!(rename("Tax", "Sum").is_err());
    assert_eq!(field(&eng, id, "Tax").name, "Tax");

    rename("Total", "Subtotal").unwrap();
    assert_eq!(
        field(&eng, id, "Sum").calculate,
        Calculate::Simple { op: CalcOp::Sum, fields: vec!["Subtotal".into(), "Tax".into()] },
        "the calculation names the new name"
    );
    // And it still calculates, with the renamed field.
    fill(&eng, id, "Subtotal", text("10")).unwrap();
    fill(&eng, id, "Tax", text("2.5")).unwrap();
    assert_eq!(field(&eng, id, "Sum").value, "12.5");

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Sum").calculate, Calculate::Simple { op: CalcOp::Sum, fields: vec!["Subtotal".into(), "Tax".into()] });
}

#[test]
fn an_export_value_of_off_is_refused() {
    let (eng, id, _) = open("export-off");
    let mut radio = nf(FieldKind::Radio, "Plan", [72.0, 140.0, 86.0, 154.0]);
    radio.export = Some("Off".into());
    let e = add_one(&eng, id, 0, &radio, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("Off"), "{e}");
    let mut boxed = nf(FieldKind::Checkbox, "Agree", [72.0, 100.0, 86.0, 114.0]);
    boxed.export = Some("Off".into());
    assert!(add_one(&eng, id, 0, &boxed, EditSource::User).is_err());
    assert!(list_all(&eng, id).unwrap().fields.is_empty(), "nothing was added");

    let x = add1(&eng, id, 0, nf(FieldKind::Checkbox, "Agree", [72.0, 100.0, 86.0, 114.0]));
    let patch = |v: &str| FieldPatch { export: Some(v.into()), ..Default::default() };
    assert!(edit_one(&eng, id, 0, x, &patch("Off"), EditSource::User).is_err());
    assert!(edit_one(&eng, id, 0, x, &patch(""), EditSource::User).is_err());
    edit_one(&eng, id, 0, x, &patch("Oui"), EditSource::User).unwrap();
    assert_eq!(field(&eng, id, "Agree").export, "Oui");
}

#[test]
fn a_reset_button_keeps_its_exclude_flag_and_other_code_stays_a_script() {
    let (eng, id, path) = open("reset-button");
    add1(&eng, id, 0, nf(FieldKind::Text, "A", [72.0, 100.0, 200.0, 120.0]));
    let b1 = add1(&eng, id, 0, NewField { caption: Some("Clear".into()), ..nf(FieldKind::Button, "Clear", [72.0, 140.0, 150.0, 164.0]) });
    let b2 = add1(&eng, id, 0, NewField { caption: Some("Ask".into()), ..nf(FieldKind::Button, "Ask", [72.0, 180.0, 150.0, 204.0]) });
    let set = |x: i32, a: ButtonAction| {
        edit_one(&eng, id, 0, x, &FieldPatch { action: Some(a), ..Default::default() }, EditSource::User).unwrap();
    };
    set(b1, ButtonAction::Reset { fields: vec!["A".into()], exclude: true });
    assert_eq!(field(&eng, id, "Clear").action, Some(ButtonAction::Reset { fields: vec!["A".into()], exclude: true }));

    // A button written by another program: a reset behind a condition is not just a reset.
    let js = "if (app.alert(\"Clear the form?\", 2, 2) == 4) this.resetForm();";
    eng.apply_custom(id, "Script button".into(), EditSource::User, false, |d| {
        let e = scan(d)?.into_iter().find(|e| e.xref == b2).expect("the button");
        let mut act = d.new_dict()?;
        put_name(&mut act, "S", "JavaScript")?;
        put_str(&mut act, "JS", js)?;
        let mut o = e.obj.clone();
        o.dict_put("A", act)?;
        Ok(vec![0])
    })
    .unwrap();
    assert_eq!(field(&eng, id, "Ask").action, Some(ButtonAction::Script { source: js.into() }));

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Clear").action, Some(ButtonAction::Reset { fields: vec!["A".into()], exclude: true }));
    assert_eq!(field(&eng2, id2, "Ask").action, Some(ButtonAction::Script { source: js.into() }));
    // Without the flag it is the listed fields.
    let x = field(&eng2, id2, "Clear").xref;
    set_one_action(&eng2, id2, x, ButtonAction::Reset { fields: vec!["A".into()], exclude: false });
    assert_eq!(field(&eng2, id2, "Clear").action, Some(ButtonAction::Reset { fields: vec!["A".into()], exclude: false }));
}

fn set_one_action(eng: &Engine, id: DocId, xref: i32, a: ButtonAction) {
    edit_one(eng, id, 0, xref, &FieldPatch { action: Some(a), ..Default::default() }, EditSource::User).unwrap();
}

#[test]
fn a_default_appearance_never_carries_nan_or_an_absurd_size() {
    for (da, size, color) in [
        ("/Helv 12 Tf 0 0 1 rg", 12.0, [0.0, 0.0, 1.0]),
        ("/Helv NaN Tf 0 g", 0.0, [0.0; 3]),
        ("/Helv inf Tf 0 g", 0.0, [0.0; 3]),
        ("/Helv -4 Tf 0 g", 0.0, [0.0; 3]),
        ("/Helv 1e9 Tf 0 g", 300.0, [0.0; 3]),
        ("/Helv 10 Tf NaN 0.5 inf rg", 10.0, [0.0, 0.5, 0.0]),
    ] {
        let (_, s, c) = parse_da(da);
        assert_eq!((s, c), (size, color), "{da}");
    }

    // Editing the text colour of a field whose size is NaN in the file writes a finite size.
    let (eng, id, _) = open("da-nan");
    let x = add1(&eng, id, 0, nf(FieldKind::Text, "A", [72.0, 100.0, 200.0, 120.0]));
    eng.apply_custom(id, "Odd DA".into(), EditSource::User, false, |d| {
        let e = scan(d)?.into_iter().find(|e| e.xref == x).expect("the field");
        let mut o = e.obj.clone();
        put_str(&mut o, "DA", "/Helv NaN Tf 0 g")?;
        Ok(vec![0])
    })
    .unwrap();
    edit_one(&eng, id, 0, x, &FieldPatch { text_color: Some([1.0, f32::NAN, 0.0]), ..Default::default() }, EditSource::User).unwrap();
    let f = field(&eng, id, "A");
    assert_eq!(f.font_size, 0.0);
    assert_eq!(f.text_color, [1.0, 0.0, 0.0]);
    edit_one(&eng, id, 0, x, &FieldPatch { font_size: Some(f32::NAN), ..Default::default() }, EditSource::User).unwrap();
    assert_eq!(field(&eng, id, "A").font_size, 0.0);
}

/// A one-field form that asks viewers to rebuild every appearance (`/NeedAppearances true`), with a
/// money format on the field.
fn need_appearances_form() -> Vec<u8> {
    pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R] /NeedAppearances true /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /Helv 3 0 R >> >> /Annots [5 0 R 6 0 R] >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Price) /Rect [72 600 200 620] /F 4 /DA (/Helv 12 Tf 0 g) \
         /AA << /F << /S /JavaScript /JS (AFNumber_Format(2, 0, 0, 0, \"$\", true);) >> \
         /K << /S /JavaScript /JS (AFNumber_Keystroke(2, 0, 0, 0, \"$\", true);) >> >> >>"
            .to_string(),
        // A check box that is on and has no appearance at all: the form relied on the flag.
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Agree) /Rect [72 560 86 574] /F 4 /V /Yes /AS /Yes          /MK << /CA (4) >> /DA (/ZaDb 0 Tf 0 g) >>"
            .to_string(),
    ])
}

/// Whether the saved file's AcroForm still says `/NeedAppearances true`.
fn needs_appearances(path: &str) -> bool {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    let acro = doc.catalog().unwrap().get_dict("AcroForm").unwrap().expect("no /AcroForm");
    acro.get_dict("NeedAppearances").unwrap().is_some_and(|n| n.as_bool().unwrap_or(false))
}

#[test]
fn a_formatted_value_is_still_shown_after_save_and_reopen_when_the_form_asks_for_new_appearances() {
    let (eng, id, path) = open_bytes("need-appearances", need_appearances_form());
    // As given, the form makes MuPDF redraw every field from its raw value.
    assert!(needs_appearances(&path));
    fill(&eng, id, "Price", text("1234.5")).unwrap();
    assert_eq!(field(&eng, id, "Price").shown, "$1,234.50");
    eng.save(id, None).unwrap();
    // The formatted appearance is in the file, and nothing asks for it to be redrawn from /V.
    assert!(ap_text(&path, "Price", None).contains("$1,234.50"), "saved appearance: {}", ap_text(&path, "Price", None));
    assert!(!needs_appearances(&path), "/NeedAppearances is cleared once appearances are written");
    // The fields the form never gave an appearance get one, or they would draw nothing.
    assert!(!ap_text(&path, "Agree", Some("Yes")).trim().is_empty(), "the check box got no appearance");
    // And in a fresh engine too, the value is the raw one and the display the formatted one.
    let (eng2, id2) = reopen(&path);
    let f = field(&eng2, id2, "Price");
    assert_eq!((f.value.as_str(), f.shown.as_str()), ("1234.5", "$1,234.50"));
}
