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

// ---- form JavaScript (js.rs, scripts.rs) ----------------------------------------------------------

/// Gives the field a script under `/AA /<key>` (what Acrobat's Actions tab writes), as one history step.
fn script(eng: &Engine, id: DocId, name: &str, key: &str, js: &str) {
    eng.apply_custom(id, "Add script".into(), EditSource::User, false, |d| {
        let entries = scan(d)?;
        let e = entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("no field {name}"));
        let mut f = e.field.clone();
        author::put_js(d, &mut f, key, Some(js.to_string()))?;
        Ok(vec![e.page])
    })
    .unwrap();
}

/// Document-level scripts (`/Names /JavaScript`). Call before the form is first listed: they are read once.
fn doc_scripts(eng: &Engine, id: DocId, list: &[(&str, &str)]) {
    eng.apply_custom(id, "Add document scripts".into(), EditSource::User, false, |d| {
        let mut arr = d.new_array()?;
        for (name, src) in list {
            arr.array_push(PdfObject::new_string(name)?)?;
            let mut a = d.new_dict()?;
            put_name(&mut a, "S", "JavaScript")?;
            put_str(&mut a, "JS", src)?;
            arr.array_push(a)?;
        }
        let mut js = d.new_dict()?;
        js.dict_put("Names", arr)?;
        let mut names = d.new_dict()?;
        names.dict_put("JavaScript", js)?;
        d.catalog()?.dict_put("Names", names)?;
        Ok(vec![])
    })
    .unwrap();
}

fn steps(eng: &Engine, id: DocId) -> usize {
    eng.history(id).unwrap().len()
}

fn script_form(test: &str, names: &[&str]) -> (Arc<Engine>, DocId, String) {
    let (eng, id, path) = open(test);
    for (i, n) in names.iter().enumerate() {
        add1(&eng, id, 0, nf(FieldKind::Text, n, [72.0, 60.0 + 30.0 * i as f32, 240.0, 80.0 + 30.0 * i as f32]));
    }
    (eng, id, path)
}

#[test]
fn a_calculate_script_that_is_not_an_af_call_updates_its_total_after_a_fill() {
    let (eng, id, path) = script_form("js-sum", &["Rent", "Deposit", "Total"]);
    script(&eng, id, "Total", "C", "var r = this.getField(\"Rent\").value, d = this.getField(\"Deposit\").value; event.value = (r || 0) + (d || 0);");
    assert!(!field(&eng, id, "Total").script, "scripts run, so the field is not marked as unrun");
    fill(&eng, id, "Rent", text("1200")).unwrap();
    assert_eq!(field(&eng, id, "Total").value, "1200");
    let before = steps(&eng, id);
    fill(&eng, id, "Deposit", text("300")).unwrap();
    assert_eq!(steps(&eng, id), before + 1, "the fill and the recalculation are one step");
    assert_eq!(field(&eng, id, "Total").value, "1500");
    eng.undo(id).unwrap();
    assert_eq!(field(&eng, id, "Total").value, "1200");
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Total").value, "1500", "saved and reopened");
}

#[test]
fn a_script_writing_other_fields_is_one_undo_step_and_saves() {
    let (eng, id, path) = script_form("js-write", &["Src", "Other", "Third"]);
    script(&eng, id, "Src", "V", "this.getField(\"Other\").value = event.value + \"!\"; this.getField(\"Third\").readonly = true;");
    let before = steps(&eng, id);
    fill(&eng, id, "Src", text("hello")).unwrap();
    assert_eq!(steps(&eng, id), before + 1, "one undo step");
    assert_eq!(field(&eng, id, "Other").value, "hello!");
    assert!(field(&eng, id, "Third").read_only);
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Other").value, "hello!");
    assert!(field(&eng2, id2, "Third").read_only);
    assert!(ap_text(&path, "Other", None).contains("hello!"));
    eng.undo(id).unwrap();
    assert_eq!(field(&eng, id, "Src").value, "");
    assert_eq!(field(&eng, id, "Other").value, "");
    assert!(!field(&eng, id, "Third").read_only);
}

#[test]
fn a_validate_script_rejects_with_its_alert_text_and_a_keystroke_script_changes_the_value() {
    let (eng, id, _) = script_form("js-reject", &["Age", "Code"]);
    script(&eng, id, "Age", "V", "if (event.value !== \"\" && Number(event.value) < 18) { app.alert(\"You must be at least 18.\"); event.rc = false; }");
    script(&eng, id, "Code", "K", "if (event.willCommit) { event.value = event.value.toUpperCase(); }");
    let steps_before = steps(&eng, id);
    let e = fill(&eng, id, "Age", text("12")).unwrap_err().to_string();
    assert!(e.contains("You must be at least 18."), "{e}");
    assert_eq!(steps(&eng, id), steps_before, "a rejected value changes nothing");
    assert_eq!(field(&eng, id, "Age").value, "");
    fill(&eng, id, "Age", text("30")).unwrap();
    fill(&eng, id, "Code", text("ab-12")).unwrap();
    assert_eq!(field(&eng, id, "Code").value, "AB-12");
    // A rejection without a reason uses a default message.
    script(&eng, id, "Age", "V", "event.rc = false;");
    let e = fill(&eng, id, "Age", text("44")).unwrap_err().to_string();
    assert!(e.contains("Invalid value entered for the field [ Age ]"), "{e}");
}

#[test]
fn alerts_and_requests_of_a_fill_come_back_to_the_ui() {
    let (eng, id, _) = script_form("js-alerts", &["Note"]);
    script(&eng, id, "Note", "K", "app.alert(\"Saved a note\"); app.launchURL(\"https://example.com/\"); app.launchURL(\"file:///etc/passwd\");");
    let w = field(&eng, id, "Note");
    let r = set_value_report(&eng, id, w.page, w.xref, &text("x"), EditSource::User).unwrap();
    assert_eq!(r.report.alerts.len(), 1);
    assert_eq!(r.report.alerts[0].message, "Saved a note");
    assert_eq!(r.report.requests, vec![js::Request::LaunchUrl { url: "https://example.com/".into() }]);
    let json = serde_json::to_value(&r).unwrap();
    assert!(json["alerts"].is_array() && json["requests"].is_array() && json["rev"].is_number(), "{json}");
}

#[test]
fn a_format_script_that_is_not_an_af_call_decides_what_is_shown() {
    let (eng, id, path) = script_form("js-format", &["Code"]);
    script(&eng, id, "Code", "F", "if (event.value !== \"\") { event.value = \"<\" + event.value + \">\"; }");
    fill(&eng, id, "Code", text("ab")).unwrap();
    let f = field(&eng, id, "Code");
    assert_eq!((f.value.as_str(), f.shown.as_str()), ("ab", "<ab>"));
    eng.save(id, None).unwrap();
    assert!(ap_text(&path, "Code", None).contains("<ab>"), "{}", ap_text(&path, "Code", None));
}

#[test]
fn scripts_turned_off_are_kept_and_not_run() {
    let (eng, id, path) = script_form("js-off", &["Src", "Other"]);
    script(&eng, id, "Src", "V", "this.getField(\"Other\").value = \"ran\";");
    let st = scripts_state(&eng, id).unwrap();
    assert!(st.has_scripts && st.enabled);
    assert!(!field(&eng, id, "Src").script);
    scripts_enable(&eng, id, false).unwrap();
    assert!(field(&eng, id, "Src").script, "off: the script is reported as one that is not run");
    fill(&eng, id, "Src", text("a")).unwrap();
    assert_eq!(field(&eng, id, "Other").value, "", "not run");
    scripts_enable(&eng, id, true).unwrap();
    fill(&eng, id, "Src", text("b")).unwrap();
    assert_eq!(field(&eng, id, "Other").value, "ran");
    // The script stayed in the file all along.
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert!(scripts_state(&eng2, id2).unwrap().has_scripts);
    assert!(scripts_state(&eng2, id2).unwrap().enabled, "on by default for every file");
}

#[test]
fn a_plain_af_call_takes_no_script_engine_and_a_form_without_scripts_has_none() {
    let (eng, id, _) = script_form("js-fast", &["Price", "Plain"]);
    let st = scripts_state(&eng, id).unwrap();
    assert!(!st.has_scripts);
    script(&eng, id, "Price", "F", "AFNumber_Format(2, 0, 0, 0, \"$\", true);");
    script(&eng, id, "Price", "K", "AFNumber_Keystroke(2, 0, 0, 0, \"$\", true);");
    assert!(!scripts_state(&eng, id).unwrap().has_scripts, "an AF call alone is not a script that needs the engine");
    // Turned off, the fast path still formats and checks, exactly as before.
    scripts_enable(&eng, id, false).unwrap();
    fill(&eng, id, "Price", text("$1,234.5")).unwrap();
    let f = field(&eng, id, "Price");
    assert_eq!((f.value.as_str(), f.shown.as_str()), ("1234.5", "$1,234.50"));
    assert!(fill(&eng, id, "Price", text("abc")).is_err());
    // On, the result is the same.
    scripts_enable(&eng, id, true).unwrap();
    fill(&eng, id, "Price", text("$99")).unwrap();
    let f = field(&eng, id, "Price");
    assert_eq!((f.value.as_str(), f.shown.as_str()), ("99", "$99.00"));
}

#[test]
fn a_document_that_redefines_an_af_function_gets_its_own_instead_of_the_fast_path() {
    let (eng, id, _) = open("js-override");
    doc_scripts(&eng, id, &[("fixes", "function AFNumber_Format(d, s, n, c, cur, p) { event.value = \"custom:\" + event.value; }")]);
    add1(&eng, id, 0, nf(FieldKind::Text, "Price", [72.0, 60.0, 240.0, 80.0]));
    script(&eng, id, "Price", "F", "AFNumber_Format(2, 0, 0, 0, \"$\", true);");
    fill(&eng, id, "Price", text("5")).unwrap();
    assert_eq!(field(&eng, id, "Price").shown, "custom:5");
    scripts_enable(&eng, id, false).unwrap();
    assert_eq!(field(&eng, id, "Price").shown, "$5.00", "scripts off: today's behaviour");
}

#[test]
fn a_document_level_function_serves_a_field_script_and_a_global_persists_between_fills() {
    let (eng, id, _) = open("js-doclevel");
    doc_scripts(&eng, id, &[("b second", "var fills = 0;"), ("a first", "function vat(x) { return Math.round(x * 125) / 100; }")]);
    add1(&eng, id, 0, nf(FieldKind::Text, "Net", [72.0, 60.0, 240.0, 80.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Gross", [72.0, 90.0, 240.0, 110.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Count", [72.0, 120.0, 240.0, 140.0]));
    script(&eng, id, "Gross", "C", "event.value = vat(this.getField(\"Net\").value);");
    script(&eng, id, "Count", "C", "event.value = String(++fills);");
    fill(&eng, id, "Net", text("200")).unwrap();
    assert_eq!(field(&eng, id, "Gross").value, "250");
    let first: u32 = field(&eng, id, "Count").value.parse().unwrap();
    fill(&eng, id, "Net", text("100")).unwrap();
    let second: u32 = field(&eng, id, "Count").value.parse().unwrap();
    assert!(second > first, "the global kept counting: {first} then {second}");
    assert_eq!(field(&eng, id, "Gross").value, "125");
}

#[test]
fn calculations_in_a_chain_settle_in_a_few_passes_and_a_runaway_one_stops() {
    let (eng, id, _) = script_form("js-chain", &["A", "B", "C", "Runaway"]);
    // Document order is the wrong order: C reads B reads A.
    script(&eng, id, "C", "C", "event.value = String(Number(this.getField(\"B\").value || 0) + 1);");
    script(&eng, id, "B", "C", "event.value = String(Number(this.getField(\"A\").value || 0) * 2);");
    script(&eng, id, "Runaway", "C", "event.value = String(Number(event.value || 0) + 1);");
    fill(&eng, id, "A", text("5")).unwrap();
    assert_eq!(field(&eng, id, "B").value, "10");
    assert_eq!(field(&eng, id, "C").value, "11");
    let n: u32 = field(&eng, id, "Runaway").value.parse().unwrap();
    assert!((1..=8).contains(&n), "at most eight passes: {n}");
}

#[test]
fn a_calculation_that_never_ends_times_out_and_the_fill_still_lands() {
    let (eng, id, _) = script_form("js-loop", &["In", "Hang", "Fine"]);
    script(&eng, id, "Hang", "C", "while (true) {}");
    script(&eng, id, "Fine", "C", "event.value = \"fine:\" + this.getField(\"In\").value;");
    let t = std::time::Instant::now();
    fill(&eng, id, "In", text("x")).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_millis(2500), "{:?}", t.elapsed());
    assert_eq!(field(&eng, id, "In").value, "x");
    assert_eq!(field(&eng, id, "Hang").value, "", "the field with the runaway script is unchanged");
    assert_eq!(field(&eng, id, "Fine").value, "fine:x", "the other calculations still run");
    let log = scripts_state(&eng, id).unwrap().log;
    assert!(log.iter().any(|l| l.field.as_deref() == Some("Hang") && l.message.contains("stopped")), "{log:?}");
    // The next fill does not wait for it again.
    let t = std::time::Instant::now();
    fill(&eng, id, "In", text("y")).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
}

#[test]
fn a_script_that_changes_a_field_colour_or_visibility_is_written_to_the_file() {
    let (eng, id, path) = script_form("js-look", &["Flag", "Target"]);
    script(
        &eng,
        id,
        "Flag",
        "V",
        "var t = this.getField(\"Target\"); t.fillColor = color.yellow; t.textColor = color.red; t.display = event.value === \"hide\" ? display.hidden : display.visible;",
    );
    fill(&eng, id, "Flag", text("hide")).unwrap();
    let t = field(&eng, id, "Target");
    assert_eq!(t.fill_color, Some([1.0, 1.0, 0.0]));
    assert_eq!(t.text_color, [1.0, 0.0, 0.0]);
    assert!(t.hidden);
    fill(&eng, id, "Flag", text("show")).unwrap();
    assert!(!field(&eng, id, "Target").hidden);
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Target").fill_color, Some([1.0, 1.0, 0.0]));
}

#[test]
fn a_script_with_non_ascii_text_is_read_whatever_way_it_is_encoded() {
    let (eng, id, path) = script_form("js-utf16", &["Src", "Other"]);
    script(&eng, id, "Src", "V", "this.getField(\"Other\").value = \"\u{e5}\u{e4}\u{f6} \u{20ac}\";");
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    fill(&eng2, id2, "Src", text("go")).unwrap();
    assert_eq!(field(&eng2, id2, "Other").value, "\u{e5}\u{e4}\u{f6} \u{20ac}");
}

// ---- authoring custom scripts, calculation order, format preview ------------------------------------

fn patch_field(eng: &Engine, id: DocId, name: &str, p: FieldPatch) -> usize {
    let before = steps(eng, id);
    let w = field(eng, id, name);
    edit_one(eng, id, w.page, w.xref, &p, EditSource::User).unwrap();
    steps(eng, id) - before
}

#[test]
fn custom_format_validate_and_calculate_scripts_are_written_read_back_and_run() {
    let (eng, id, path) = script_form("custom-round-trip", &["Rent", "Code", "Age", "Total"]);
    let fmt = Format::Custom { format: "if (event.value !== \"\") { event.value = \"<\" + event.value + \">\"; }".into(), keystroke: "event.value = event.value.toUpperCase();".into() };
    let val = Validate::Custom { source: "if (event.value !== \"\" && Number(event.value) < 18) { app.alert(\"You must be at least 18.\"); event.rc = false; }".into() };
    let calc = Calculate::Custom("event.value = Number(this.getField(\"Rent\").value || 0) * 2;".into());

    assert_eq!(patch_field(&eng, id, "Code", FieldPatch { format: Some(fmt.clone()), ..Default::default() }), 1, "one undo step");
    assert_eq!(patch_field(&eng, id, "Age", FieldPatch { validate: Some(val.clone()), ..Default::default() }), 1);
    assert_eq!(patch_field(&eng, id, "Total", FieldPatch { calculate: Some(calc.clone()), ..Default::default() }), 1);

    // Read back as the same custom settings (an unrecognised script parses as Custom for the editor).
    assert_eq!(field(&eng, id, "Code").format, fmt);
    assert_eq!(field(&eng, id, "Age").validate, val);
    assert_eq!(field(&eng, id, "Total").calculate, calc);
    assert_eq!(list_all(&eng, id).unwrap().calc_order, vec![field(&eng, id, "Total").node], "a custom calculation is in the order");
    assert!(scripts_state(&eng, id).unwrap().has_scripts);

    // And the scripts run.
    fill(&eng, id, "Code", text("ab")).unwrap();
    let c = field(&eng, id, "Code");
    assert_eq!((c.value.as_str(), c.shown.as_str()), ("AB", "<AB>"));
    assert!(fill(&eng, id, "Age", text("9")).unwrap_err().to_string().contains("at least 18"));
    fill(&eng, id, "Rent", text("21")).unwrap();
    assert_eq!(field(&eng, id, "Total").value, "42");

    // Saved and reopened.
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Code").format, fmt);
    assert_eq!(field(&eng2, id2, "Age").validate, val);
    assert_eq!(field(&eng2, id2, "Total").calculate, calc);
    assert_eq!(field(&eng2, id2, "Total").value, "42");

    // A format with only a keystroke script writes no format script; emptying every box removes the scripts.
    let keys = Format::Custom { format: String::new(), keystroke: "event.rc = true;".into() };
    patch_field(&eng, id, "Code", FieldPatch { format: Some(keys.clone()), ..Default::default() });
    assert_eq!(field(&eng, id, "Code").format, keys);
    assert_eq!(patch_field(&eng, id, "Code", FieldPatch { format: Some(Format::Custom { format: " ".into(), keystroke: String::new() }), ..Default::default() }), 1);
    assert_eq!(field(&eng, id, "Code").format, Format::None);
    patch_field(&eng, id, "Age", FieldPatch { validate: Some(Validate::Custom { source: String::new() }), ..Default::default() });
    assert_eq!(field(&eng, id, "Age").validate, Validate::None);
    patch_field(&eng, id, "Total", FieldPatch { calculate: Some(Calculate::Custom(String::new())), ..Default::default() });
    assert_eq!(field(&eng, id, "Total").calculate, Calculate::None);
    assert!(list_all(&eng, id).unwrap().calc_order.is_empty());
    assert_eq!(calc_order_len(&eng, id), 0, "the field left the calculation order");

    // Undo brings the custom script back.
    eng.undo(id).unwrap();
    assert!(matches!(field(&eng, id, "Total").calculate, Calculate::Custom(_)));
    assert_eq!(calc_order_len(&eng, id), 1);
}

#[test]
fn acrobat_calls_still_read_as_their_tab_and_a_mixed_pair_reads_as_custom() {
    let (eng, id, _) = script_form("custom-or-not", &["A", "B", "C"]);
    script(&eng, id, "A", "F", "AFNumber_Format(2, 0, 0, 0, \"$\", true);");
    script(&eng, id, "A", "K", "AFNumber_Keystroke(2, 0, 0, 0, \"$\", true);");
    assert!(matches!(field(&eng, id, "A").format, Format::Number { decimals: 2, .. }));
    // A format call next to a keystroke script of the author's own is not a tab setting: the editor shows both scripts.
    script(&eng, id, "B", "F", "AFNumber_Format(2, 0, 0, 0, \"$\", true);");
    script(&eng, id, "B", "K", "event.rc = event.value !== \"x\";");
    assert!(matches!(field(&eng, id, "B").format, Format::Custom { .. }));
    script(&eng, id, "C", "V", "AFRange_Validate(true, 1, true, 9);");
    assert_eq!(field(&eng, id, "C").validate, Validate::Range { min: Some(1.0), max: Some(9.0) });
}

#[test]
fn the_calculation_order_is_one_undo_step_and_survives_save_and_reopen() {
    let (eng, id, path) = script_form("calc-order", &["In", "A", "B", "C"]);
    for (n, src) in [("A", "event.value = 1;"), ("B", "event.value = 2;"), ("C", "event.value = 3;")] {
        patch_field(&eng, id, n, FieldPatch { calculate: Some(Calculate::Custom(src.into())), ..Default::default() });
    }
    let node = |n: &str| field(&eng, id, n).node;
    let (a, b, c) = (node("A"), node("B"), node("C"));
    assert_eq!(list_all(&eng, id).unwrap().calc_order, vec![a, b, c], "the order they were authored in");

    let before = steps(&eng, id);
    calc_order_one(&eng, id, &[c, a, b], EditSource::User).unwrap();
    assert_eq!(steps(&eng, id), before + 1, "one undo step");
    assert_eq!(list_all(&eng, id).unwrap().calc_order, vec![c, a, b]);
    assert_eq!(calc_order_len(&eng, id), 3);

    // The same order again is not a step; a field left out follows the listed ones.
    calc_order_one(&eng, id, &[c, a, b], EditSource::User).unwrap();
    assert_eq!(steps(&eng, id), before + 1);
    calc_order_one(&eng, id, &[b], EditSource::User).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().calc_order, vec![b, c, a]);

    // Bad lists change nothing.
    let n = steps(&eng, id);
    assert!(calc_order_one(&eng, id, &[a, a], EditSource::User).is_err(), "listed twice");
    assert!(calc_order_one(&eng, id, &[node("In")], EditSource::User).is_err(), "a field that does not calculate");
    assert!(calc_order_one(&eng, id, &[9999], EditSource::User).is_err(), "no such field");
    assert_eq!(steps(&eng, id), n);

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let node2 = |n: &str| field(&eng2, id2, n).node;
    assert_eq!(list_all(&eng2, id2).unwrap().calc_order, vec![node2("B"), node2("C"), node2("A")]);

    eng.undo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().calc_order, vec![c, a, b]);
}

#[test]
fn calculations_that_read_each_other_settle_whatever_the_order() {
    let (eng, id, _) = script_form("calc-order-runs", &["In", "Double", "Plus"]);
    // Plus reads Double; the passes settle it even when Plus runs first.
    patch_field(&eng, id, "Plus", FieldPatch { calculate: Some(Calculate::Custom("event.value = Number(this.getField(\"Double\").value || 0) + 1;".into())), ..Default::default() });
    patch_field(&eng, id, "Double", FieldPatch { calculate: Some(Calculate::Custom("event.value = Number(this.getField(\"In\").value || 0) * 2;".into())), ..Default::default() });
    fill(&eng, id, "In", text("5")).unwrap();
    assert_eq!(field(&eng, id, "Plus").value, "11");
    assert_eq!(list_all(&eng, id).unwrap().calc_order.len(), 2);
}

#[test]
fn the_format_preview_shows_what_a_format_makes_of_a_sample() {
    use super::af::format_preview;
    let num = Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true };
    assert_eq!(format_preview(&num, ""), Some(("1234.5".into(), "$1,234.50".into())));
    assert_eq!(format_preview(&num, " 99 "), Some(("99".into(), "$99.00".into())));
    assert_eq!(format_preview(&Format::Percent { decimals: 1, sep: 0 }, ""), Some(("0.256".into(), "25.6%".into())));
    let (entry, shown) = format_preview(&Format::Date("dd/mm/yyyy".into()), "").unwrap();
    assert_eq!((entry.as_str(), shown.as_str()), ("11/10/2026", "11/10/2026"));
    let (_, shown) = format_preview(&Format::Date("mmmm d, yyyy".into()), "").unwrap();
    assert_eq!(shown, "October 11, 2026");
    let (_, shown) = format_preview(&Format::Time("h:MM tt".into()), "").unwrap();
    assert_eq!(shown, "2:05 pm");
    assert_eq!(format_preview(&Format::Special(2), "").unwrap().1, "(555) 123-4567");
    assert_eq!(format_preview(&Format::Special(3), "").unwrap().1, "123-45-6789");
    assert!(format_preview(&Format::Mask("99-AA".into()), "").is_some());
    assert_eq!(format_preview(&Format::None, ""), None);
    assert_eq!(format_preview(&Format::Custom { format: "x".into(), keystroke: String::new() }, ""), None, "a script is not previewed");
    // The wire shape of the new variants, as the UI sends them.
    let f: Format = serde_json::from_str(r#"{"kind":"custom","value":{"format":"a","keystroke":"b"}}"#).unwrap();
    assert_eq!(f, Format::Custom { format: "a".into(), keystroke: "b".into() });
    let v: Validate = serde_json::from_str(r#"{"kind":"custom","source":"s"}"#).unwrap();
    assert_eq!(v, Validate::Custom { source: "s".into() });
    let c: Calculate = serde_json::from_str(r#"{"kind":"custom","value":"s"}"#).unwrap();
    assert_eq!(c, Calculate::Custom("s".into()));
}

// ---- the form JavaScript sample (docs/study/samples/_gen/formsjs-sample.pdf) -----------------------------------

/// A one-page form for the form JavaScript checks: a number format, a date with `AFDate_KeystrokeEx`, three numbers with a
/// SUM total, a range-validated age, a custom calculation that calls a document-level function, a field whose /C is
/// `while(true){}`, a custom format with a keystroke script, and two fields whose scripts raise an alert and a link.
fn formsjs_sample() -> Vec<u8> {
    const FIRST: usize = 6;
    // (name, label, extra entries)
    let rows: [(&str, &str, &str); 12] = [
        ("Name", "Name (alert on commit)", "/AA << /V << /S /JavaScript /JS (if (event.value !== \"\") { app.alert(\"Welcome, \" + event.value + \".\"); }) >> >>"),
        ("Website", "Page to visit (link on commit)", "/AA << /V << /S /JavaScript /JS (if (event.value !== \"\") { app.launchURL(\"https://example.com/\" + event.value); }) >> >>"),
        (
            "Price",
            "Price (number format)",
            "/AA << /F << /S /JavaScript /JS (AFNumber_Format(2, 0, 0, 0, \"$\", true);) >> /K << /S /JavaScript /JS (AFNumber_Keystroke(2, 0, 0, 0, \"$\", true);) >> >>",
        ),
        (
            "Born",
            "Date of birth (dd/mm/yyyy)",
            "/AA << /F << /S /JavaScript /JS (AFDate_FormatEx(\"dd/mm/yyyy\");) >> /K << /S /JavaScript /JS (AFDate_KeystrokeEx(\"dd/mm/yyyy\");) >> >>",
        ),
        ("Rent", "Rent", ""),
        ("Fees", "Fees", ""),
        ("Other", "Other", ""),
        (
            "Total",
            "Total (sum of the three)",
            "/Ff 1 /AA << /F << /S /JavaScript /JS (AFNumber_Format(2, 0, 0, 0, \"$\", true);) >> /C << /S /JavaScript /JS (AFSimple_Calculate(\"SUM\", new Array(\"Rent\", \"Fees\", \"Other\"));) >> >>",
        ),
        (
            "Tax",
            "Tax (custom script, document function)",
            "/Ff 1 /AA << /C << /S /JavaScript /JS (event.value = tax(Number(this.getField(\"Total\").value) || 0);) >> >>",
        ),
        ("Age", "Age (18 to 65)", "/AA << /V << /S /JavaScript /JS (AFRange_Validate(true, 18, true, 65);) >> >>"),
        (
            "Code",
            "Code (custom format + keystroke)",
            "/AA << /F << /S /JavaScript /JS (if (event.value !== \"\") {\\n  event.value = \"#\" + event.value;\\n}) >> /K << /S /JavaScript /JS (if (event.willCommit) {\\n  event.value = event.value.toUpperCase();\\n}) >> >>",
        ),
        ("Spin", "Never finishes (while(true){})", "/Ff 1 /AA << /C << /S /JavaScript /JS (while(true){}) >> >>"),
    ];
    let fields: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, (name, _, extra))| {
            let top = 110 + 40 * i as i32;
            let (y1, y0) = (792 - top, 792 - top - 22);
            format!(
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [230 {y0} 430 {y1}] /F 4 /P 4 0 R /DA (/Helv 11 Tf 0 g) /MK << /BC [0.6 0.6 0.6] /BG [1 1 1] >> {extra} >>"
            )
        })
        .collect();
    let mut content = String::from("BT /Helv 18 Tf 72 720 Td (Form scripts sample) Tj ET\n");
    for (i, (_, label, _)) in rows.iter().enumerate() {
        let top = 110 + 40 * i as i32;
        content.push_str(&format!("BT /Helv 10 Tf 0.25 g 72 {} Td ({label}) Tj ET\n", 792 - top - 15));
    }
    let refs = |names: &[&str]| names.iter().map(|n| format!("{} 0 R", FIRST + rows.iter().position(|r| r.0 == *n).unwrap())).collect::<Vec<_>>().join(" ");
    let all: Vec<String> = (0..rows.len()).map(|i| format!("{} 0 R", FIRST + i)).collect();
    let mut objs = vec![
        format!(
            "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript << /Names [(lib) {} 0 R] >> >> /AcroForm << /Fields [{}] /CO [{}] /NeedAppearances true /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>",
            FIRST + rows.len(),
            all.join(" "),
            refs(&["Total", "Tax", "Spin"])
        ),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /Helv 3 0 R >> >> /Contents 5 0 R /Annots [{}] >>",
            all.join(" ")
        ),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
    ];
    objs.extend(fields);
    objs.push("<< /S /JavaScript /JS (function tax(x) { return Math.round(x * 25) / 100; }) >>".to_string());
    pdf_of(&objs)
}

#[test]
fn the_form_javascript_sample_behaves_like_the_checklist() {
    let (eng, id, path) = open_bytes("formsjs-sample", formsjs_sample());
    assert_eq!(list_all(&eng, id).unwrap().fields.len(), 12);
    assert!(scripts_state(&eng, id).unwrap().has_scripts);

    // number format
    fill(&eng, id, "Price", text("1234.5")).unwrap();
    assert_eq!(field(&eng, id, "Price").shown, "$1,234.50");
    // bad date rejected, a good one kept
    assert!(fill(&eng, id, "Born", text("31/02/2000")).is_err());
    assert!(fill(&eng, id, "Born", text("soon")).is_err());
    fill(&eng, id, "Born", text("18/04/1991")).unwrap();
    assert_eq!(field(&eng, id, "Born").shown, "18/04/1991");
    // range error
    let e = fill(&eng, id, "Age", text("12")).unwrap_err().to_string();
    assert!(e.contains("greater than or equal to 18"), "{e}");
    fill(&eng, id, "Age", text("30")).unwrap();
    // an alert and a link come back to the UI
    let w = field(&eng, id, "Name");
    let r = set_value_report(&eng, id, w.page, w.xref, &text("Ada"), EditSource::User).unwrap();
    assert_eq!(r.report.alerts[0].message, "Welcome, Ada.");
    let w = field(&eng, id, "Website");
    let r = set_value_report(&eng, id, w.page, w.xref, &text("help"), EditSource::User).unwrap();
    assert_eq!(r.report.requests, vec![js::Request::LaunchUrl { url: "https://example.com/help".into() }]);
    // custom format + keystroke
    fill(&eng, id, "Code", text("ab1")).unwrap();
    assert_eq!((field(&eng, id, "Code").value.as_str(), field(&eng, id, "Code").shown.as_str()), ("AB1", "#AB1"));
    assert!(matches!(field(&eng, id, "Code").format, Format::Custom { .. }));
    // the total follows; the document function serves the tax; the runaway script times out and the rest still works
    let t0 = std::time::Instant::now();
    fill(&eng, id, "Rent", text("1000")).unwrap();
    fill(&eng, id, "Fees", text("200")).unwrap();
    fill(&eng, id, "Other", text("50.5")).unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_secs(20), "{:?}", t0.elapsed());
    assert_eq!(field(&eng, id, "Total").value, "1250.5");
    assert_eq!(field(&eng, id, "Total").shown, "$1,250.50");
    assert_eq!(field(&eng, id, "Tax").value, "312.63");
    assert!(scripts_state(&eng, id).unwrap().log.iter().any(|l| l.message.contains("longer than") || l.message.contains("timed out") || l.message.contains("time")), "{:?}", scripts_state(&eng, id).unwrap().log);
    assert_eq!(list_all(&eng, id).unwrap().calc_order.len(), 3);

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Total").value, "1250.5");
    assert_eq!(field(&eng2, id2, "Born").value, "18/04/1991");
    assert_eq!(field(&eng2, id2, "Code").shown, "#AB1");

    // scripts off: the script engine does not run (the custom Tax keeps its value), Acrobat's own calls still do
    scripts_enable(&eng2, id2, false).unwrap();
    fill(&eng2, id2, "Rent", text("5")).unwrap();
    assert_eq!(field(&eng2, id2, "Total").value, "255.5", "AFSimple_Calculate takes the native fast path");
    assert_eq!(field(&eng2, id2, "Tax").value, "312.63", "the custom calculation did not run");
}

#[test]
fn a_validate_script_that_recolours_its_own_field_keeps_the_typed_value() {
    let (eng, id, path) = script_form("js-self-style", &["Amount"]);
    script(&eng, id, "Amount", "V", "event.target.textColor = Number(event.value) < 0 ? color.red : color.black;");
    fill(&eng, id, "Amount", text("-5")).unwrap();
    let f = field(&eng, id, "Amount");
    assert_eq!((f.value.as_str(), f.text_color), ("-5", [1.0, 0.0, 0.0]), "the entry stays and the script's colour is written");
    fill(&eng, id, "Amount", text("7")).unwrap();
    let f = field(&eng, id, "Amount");
    assert_eq!((f.value.as_str(), f.text_color), ("7", [0.0, 0.0, 0.0]));
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Amount").value, "7");
}

#[test]
fn keystroke_and_validate_scripts_that_both_style_the_same_field_keep_the_value_and_the_later_style() {
    let (eng, id, _) = script_form("js-self-style-kv", &["Code"]);
    script(&eng, id, "Code", "K", "event.target.fillColor = color.yellow; event.target.textColor = color.red;");
    script(&eng, id, "Code", "V", "event.target.textColor = color.blue;");
    fill(&eng, id, "Code", text("abc")).unwrap();
    let f = field(&eng, id, "Code");
    assert_eq!(f.value, "abc", "V does not put back the value K saw");
    assert_eq!(f.fill_color, Some([1.0, 1.0, 0.0]), "K's fill stays");
    assert_eq!(f.text_color, [0.0, 0.0, 1.0], "V's colour wins over K's");
}

#[test]
fn a_keystroke_or_validate_script_sees_the_typed_value_as_its_target_value() {
    let (eng, id, _) = script_form("js-target-value", &["Code"]);
    script(&eng, id, "Code", "V", "if (String(event.target.value) !== event.value) { app.alert(\"stale\"); event.rc = false; }");
    fill(&eng, id, "Code", text("abc")).unwrap();
    assert_eq!(field(&eng, id, "Code").value, "abc");
}

/// Marks the signature field `Sig` as signed (a `/V` dictionary), with an optional `/Lock` of `(action, fields)` and an
/// optional DocMDP `/P` in the signature's `/Reference`.
fn sign(eng: &Engine, id: DocId, lock: Option<(&str, &[&str])>, docmdp: Option<i32>) {
    eng.apply_custom(id, "Sign".into(), EditSource::User, false, |d| {
        let entries = scan(d)?;
        let e = entries.iter().find(|e| e.name == "Sig").expect("a signature field");
        let mut f = e.field.clone();
        let mut v = d.new_dict()?;
        put_name(&mut v, "Type", "Sig")?;
        if let Some(p) = docmdp {
            let mut params = d.new_dict()?;
            params.dict_put("P", PdfObject::new_int(p)?)?;
            let mut r = d.new_dict()?;
            put_name(&mut r, "TransformMethod", "DocMDP")?;
            r.dict_put("TransformParams", params)?;
            let mut a = d.new_array()?;
            a.array_push(r)?;
            v.dict_put("Reference", a)?;
        }
        f.dict_put("V", v)?;
        if let Some((action, names)) = lock {
            let mut l = d.new_dict()?;
            put_name(&mut l, "Action", action)?;
            let mut a = d.new_array()?;
            for n in names {
                a.array_push(PdfObject::new_string(n)?)?;
            }
            l.dict_put("Fields", a)?;
            f.dict_put("Lock", l)?;
        }
        Ok(vec![e.page])
    })
    .unwrap();
}

#[test]
fn scripts_write_nothing_to_a_signed_document_or_to_a_signature_widget() {
    let (eng, id, _) = script_form("js-signed", &["Src", "Other"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    script(&eng, id, "Src", "V", "var o = this.getField(\"Other\"); o.value = \"x\"; o.readonly = true; o.fillColor = color.red; this.getField(\"Sig\").readonly = true;");
    // Not signed yet: the script writes, but never to the signature widget.
    fill(&eng, id, "Src", text("a")).unwrap();
    assert_eq!(field(&eng, id, "Other").value, "x");
    assert!(!field(&eng, id, "Sig").read_only, "a signature widget is never written by a script");
    eng.undo(id).unwrap();
    assert_eq!(field(&eng, id, "Other").value, "");
    // Signed (covering Other): nothing is written to it either, and the typed value still lands.
    sign(&eng, id, Some(("Include", &["Other"])), None);
    fill(&eng, id, "Src", text("b")).unwrap();
    let o = field(&eng, id, "Other");
    assert_eq!((o.value.as_str(), o.read_only, o.fill_color), ("", false, None));
    assert_eq!(field(&eng, id, "Src").value, "b");
    assert!(scripts_state(&eng, id).unwrap().log.iter().any(|l| l.message.contains("signed")));
}

#[test]
fn a_signature_lock_covers_only_the_fields_it_lists() {
    let (eng, id, _) = script_form("js-signed-lock", &["Src", "Locked", "Free"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    sign(&eng, id, Some(("Include", &["Locked"])), None);
    script(&eng, id, "Src", "V", "this.getField(\"Locked\").value = \"x\"; this.getField(\"Free\").value = \"y\";");
    fill(&eng, id, "Src", text("a")).unwrap();
    assert_eq!(field(&eng, id, "Locked").value, "");
    assert_eq!(field(&eng, id, "Free").value, "y");
}

#[test]
fn an_approval_signature_with_no_lock_leaves_the_form_to_be_filled_and_a_docmdp_one_does_not() {
    let (eng, id, _) = script_form("js-signed-approval", &["Src", "Other"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    script(&eng, id, "Src", "V", "this.getField(\"Other\").value = \"x\";");
    sign(&eng, id, None, None);
    fill(&eng, id, "Src", text("a")).unwrap();
    assert_eq!(field(&eng, id, "Other").value, "x", "a plain approval signature locks no field");
    eng.undo(id).unwrap();
    sign(&eng, id, None, Some(1));
    assert!(fill(&eng, id, "Src", text("b")).is_err(), "DocMDP 1 allows no change, not even typing");
    assert_eq!(field(&eng, id, "Other").value, "", "DocMDP 1 allows no change");
}

#[test]
fn a_reset_leaves_a_field_a_signature_covers_alone() {
    let (eng, id, _) = script_form("js-signed-reset", &["Locked", "Free"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    fill(&eng, id, "Locked", text("keep")).unwrap();
    fill(&eng, id, "Free", text("drop")).unwrap();
    sign(&eng, id, Some(("Include", &["Locked"])), None);
    // What `this.resetForm()` asks for is this reset, of every field.
    reset_one(&eng, id, None, EditSource::User).unwrap();
    assert_eq!(field(&eng, id, "Locked").value, "keep", "the signature covers it");
    assert_eq!(field(&eng, id, "Free").value, "");
    // With nothing but covered fields left to reset, there is no step to take.
    let before = steps(&eng, id);
    reset_one(&eng, id, None, EditSource::User).unwrap();
    assert_eq!(steps(&eng, id), before);
}

#[test]
fn typing_into_a_field_a_signature_covers_is_refused_with_a_message() {
    let (eng, id, _) = script_form("js-signed-typing", &["Locked", "Free"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    sign(&eng, id, Some(("Include", &["Locked"])), None);
    let err = fill(&eng, id, "Locked", text("x")).unwrap_err().to_string();
    assert!(err.contains("locked by a signature") && err.contains("Locked"), "{err}");
    assert_eq!(field(&eng, id, "Locked").value, "");
    fill(&eng, id, "Free", text("y")).unwrap();
    assert_eq!(field(&eng, id, "Free").value, "y");
}

#[test]
fn a_docmdp_one_listed_after_a_fieldmdp_with_a_lock_still_locks_every_field() {
    // What Acrobat writes when you certify with locked fields: a /Lock, and a /Reference of FieldMDP then DocMDP P=1.
    let (eng, id, _) = script_form("js-signed-fieldmdp-docmdp", &["Src", "Locked", "Other"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    script(&eng, id, "Src", "V", "this.getField(\"Other\").value = \"x\";");
    fill(&eng, id, "Locked", text("keep")).unwrap();
    eng.apply_custom(id, "Sign".into(), EditSource::User, false, |d| {
        let entries = scan(d)?;
        let e = entries.iter().find(|e| e.name == "Sig").expect("a signature field");
        let mut f = e.field.clone();
        let include = |d: &mupdf::pdf::PdfDocument| -> Result<PdfObject> {
            let mut l = d.new_dict()?;
            put_name(&mut l, "Action", "Include")?;
            let mut a = d.new_array()?;
            a.array_push(PdfObject::new_string("Src")?)?;
            l.dict_put("Fields", a)?;
            Ok(l)
        };
        let mut field_mdp = d.new_dict()?;
        put_name(&mut field_mdp, "TransformMethod", "FieldMDP")?;
        field_mdp.dict_put("TransformParams", include(d)?)?;
        let mut params = d.new_dict()?;
        params.dict_put("P", PdfObject::new_int(1)?)?;
        let mut doc_mdp = d.new_dict()?;
        put_name(&mut doc_mdp, "TransformMethod", "DocMDP")?;
        doc_mdp.dict_put("TransformParams", params)?;
        let mut refs = d.new_array()?;
        refs.array_push(field_mdp)?;
        refs.array_push(doc_mdp)?;
        let mut v = d.new_dict()?;
        put_name(&mut v, "Type", "Sig")?;
        v.dict_put("Reference", refs)?;
        f.dict_put("V", v)?;
        f.dict_put("Lock", include(d)?)?;
        Ok(vec![e.page])
    })
    .unwrap();
    for name in ["Src", "Locked", "Other"] {
        assert!(fill(&eng, id, name, text("y")).is_err(), "{name}: DocMDP 1 refuses typing");
    }
    assert_eq!(field(&eng, id, "Other").value, "", "no script ran to write it");
    let before = steps(&eng, id);
    reset_one(&eng, id, None, EditSource::User).unwrap();
    assert_eq!(field(&eng, id, "Locked").value, "keep", "DocMDP 1 refuses the reset");
    assert_eq!(steps(&eng, id), before);
}

#[test]
fn a_certified_document_takes_scripted_values_but_not_other_properties() {
    for p in [2, 3] {
        let (eng, id, _) = script_form(&format!("js-certified-{p}"), &["Src", "Other"]);
        add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
        script(
            &eng,
            id,
            "Src",
            "V",
            "var o = this.getField(\"Other\"); o.value = \"x\"; o.readonly = true; o.required = true; o.fillColor = color.red; o.textColor = color.blue; o.display = display.hidden;",
        );
        sign(&eng, id, None, Some(p));
        fill(&eng, id, "Src", text("a")).unwrap();
        let o = field(&eng, id, "Other");
        assert_eq!(o.value, "x", "P {p}: a value is what a certified form takes");
        assert!(!o.read_only && !o.required && !o.hidden, "P {p}: {o:?}");
        assert_eq!((o.fill_color, o.text_color), (None, [0.0, 0.0, 0.0]), "P {p}");
        assert!(scripts_state(&eng, id).unwrap().log.iter().any(|l| l.message.contains("certified")), "P {p}");
    }
}

/// Records a DocMDP certification of `/P` `p` in the catalog's `/Perms` (the field tree is left as it is).
fn certify_catalog(eng: &Engine, id: DocId, p: i32) {
    eng.apply_custom(id, "Certify".into(), EditSource::User, false, |d| {
        let mut params = d.new_dict()?;
        params.dict_put("P", PdfObject::new_int(p)?)?;
        let mut r = d.new_dict()?;
        put_name(&mut r, "TransformMethod", "DocMDP")?;
        r.dict_put("TransformParams", params)?;
        let mut refs = d.new_array()?;
        refs.array_push(r)?;
        let mut sig = d.new_dict()?;
        put_name(&mut sig, "Type", "Sig")?;
        sig.dict_put("Reference", refs)?;
        let mut perms = d.new_dict()?;
        perms.dict_put("DocMDP", sig)?;
        d.catalog()?.dict_put("Perms", perms)?;
        Ok(vec![0])
    })
    .unwrap();
}

#[test]
fn a_document_certified_in_the_catalog_takes_scripted_values_only() {
    // The catalog certifies with P 2 and no signature field has a value; then over a signature that says P 3.
    for (name, field_p) in [("catalog-only", None), ("catalog-over-field", Some(3))] {
        let (eng, id, _) = script_form(&format!("js-certified-{name}"), &["Src", "Other"]);
        add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
        script(&eng, id, "Src", "V", "var o = this.getField(\"Other\"); o.value = \"x\"; o.readonly = true; o.required = true; o.display = display.hidden;");
        if let Some(p) = field_p {
            sign(&eng, id, None, Some(p));
        }
        certify_catalog(&eng, id, 2);
        fill(&eng, id, "Src", text("a")).unwrap();
        let o = field(&eng, id, "Other");
        assert_eq!(o.value, "x", "{name}: a value is what a certified form takes");
        assert!(!o.read_only && !o.required && !o.hidden, "{name}: {o:?}");
        assert!(scripts_state(&eng, id).unwrap().log.iter().any(|l| l.message.contains("certified")), "{name}");
    }
}

#[test]
fn a_catalog_certification_that_allows_no_change_refuses_typing_scripts_resets_and_calculations() {
    // P 1 in the catalog alone, and over a signature field that says P 3.
    for (name, field_p) in [("catalog-only", None), ("catalog-over-field", Some(3))] {
        let (eng, id, _) = script_form(&format!("js-certified-p1-{name}"), &["Src", "Other", "Total"]);
        add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
        fill(&eng, id, "Other", text("keep")).unwrap();
        script(&eng, id, "Src", "V", "this.getField(\"Other\").value = \"x\";");
        script(&eng, id, "Total", "C", "event.value = 7;");
        if let Some(p) = field_p {
            sign(&eng, id, None, Some(p));
        }
        certify_catalog(&eng, id, 1);
        let err = fill(&eng, id, "Src", text("a")).unwrap_err().to_string();
        assert!(err.contains("locked by a signature"), "{name}: {err}");
        assert!(fill(&eng, id, "Other", text("typed")).is_err(), "{name}: typing into a field is refused");
        reset_one(&eng, id, None, EditSource::User).unwrap();
        for (f, want) in [("Src", ""), ("Other", "keep"), ("Total", "")] {
            assert_eq!(field(&eng, id, f).value, want, "{name}: {f}");
        }
    }
}

#[test]
fn a_certified_document_refuses_every_authoring_step_and_leaves_no_history() {
    // Certified in the catalog (P 1, 2 and 3, the loosest), and on a signature field (P 2 and 3).
    for (name, catalog_p, field_p) in [
        ("catalog", Some(3), None),
        ("catalog-p1", Some(1), None),
        ("catalog-p2", Some(2), None),
        ("signature", None, Some(2)),
        ("signature-p3", None, Some(3)),
    ] {
        let (eng, id, _) = script_form(&format!("author-certified-{name}"), &["A", "B"]);
        add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
        script(&eng, id, "A", "C", "event.value = 1;");
        script(&eng, id, "B", "C", "event.value = 2;");
        let (a, b) = (field(&eng, id, "A").xref, field(&eng, id, "B").xref);
        let nodes = eng.read_custom(id, |d| Ok(scan(d)?.iter().filter(|e| e.name == "A" || e.name == "B").map(|e| e.node).collect::<Vec<_>>())).unwrap();
        if let Some(p) = field_p {
            sign(&eng, id, None, Some(p));
        }
        if let Some(p) = catalog_p {
            certify_catalog(&eng, id, p);
        }
        let before = (steps(&eng, id), list_all(&eng, id).unwrap().fields.into_iter().map(|w| (w.name, w.xref)).collect::<Vec<_>>());
        let refused = [
            ("add", add_one(&eng, id, 0, &nf(FieldKind::Text, "New", [72.0, 300.0, 200.0, 320.0]), EditSource::User).map(|_| ())),
            ("add many", add_many(&eng, id, 0, &[nf(FieldKind::Text, "N1", [72.0, 300.0, 200.0, 320.0])], EditSource::User).map(|_| ())),
            ("move", move_one(&eng, id, 0, a, [100.0, 300.0, 260.0, 330.0], EditSource::User).map(|_| ())),
            ("edit", edit_one(&eng, id, 0, a, &FieldPatch { tooltip: Some("t".into()), ..Default::default() }, EditSource::User).map(|_| ())),
            ("delete", delete_many(&eng, id, &[(0, b)], EditSource::User).map(|_| ())),
            ("calc order", calc_order_one(&eng, id, &[nodes[1], nodes[0]], EditSource::User).map(|_| ())),
            ("tab order", tab_order_one(&eng, id, 0, &[b, a], EditSource::User).map(|_| ())),
            ("arrange", arrange_one(&eng, id, 0, false, EditSource::User).map(|_| ())),
        ];
        for (what, r) in refused {
            let e = r.expect_err(&format!("{name}: {what} must be refused")).to_string();
            assert!(e.contains("certified"), "{name}: {what}: {e}");
        }
        let after = (steps(&eng, id), list_all(&eng, id).unwrap().fields.into_iter().map(|w| (w.name, w.xref)).collect::<Vec<_>>());
        assert_eq!(before, after, "{name}: nothing changed and no step was made");
    }
}

#[test]
fn authoring_in_a_signed_document_is_saved_after_the_signed_bytes() {
    let (eng, id, path) = script_form("author-signed", &["A"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    sign(&eng, id, None, None);
    eng.save(id, None).unwrap();
    let original = std::fs::read(&path).unwrap();
    let (eng2, id2) = reopen(&path);
    assert!(eng2.read_custom(id2, |d| Ok(crate::outline::signed(d))).unwrap(), "the signature is seen");
    add1(&eng2, id2, 0, nf(FieldKind::Text, "B", [72.0, 300.0, 200.0, 320.0]));
    let a = field(&eng2, id2, "A").xref;
    delete_many(&eng2, id2, &[(0, a)], EditSource::User).unwrap();
    eng2.save(id2, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the file was rewritten: the signature would no longer verify");
    let (eng3, id3) = reopen(&path);
    let names: Vec<String> = list_all(&eng3, id3).unwrap().fields.into_iter().map(|w| w.name).collect();
    assert!(names.contains(&"B".to_string()) && !names.contains(&"A".to_string()), "{names:?}");
}

#[test]
fn a_calculated_result_is_not_written_into_a_field_a_signature_locks() {
    let (eng, id, _) = script_form("js-signed-calc", &["Rent", "Deposit", "Total"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 200.0, 240.0, 240.0]));
    script(&eng, id, "Total", "C", "event.value = 7;");
    sign(&eng, id, Some(("Include", &["Total"])), None);
    fill(&eng, id, "Rent", text("1")).unwrap();
    assert_eq!(field(&eng, id, "Total").value, "", "Total is covered by the signature");
}

#[test]
fn a_later_script_of_the_same_step_sees_what_an_earlier_one_wrote_and_a_no_op_write_does_not_undo_it() {
    let (eng, id, _) = script_form("js-kv-chain", &["Code", "Other"]);
    script(&eng, id, "Code", "K", "this.getField(\"Other\").value = \"x\";");
    script(&eng, id, "Code", "V", "var o = this.getField(\"Other\"); o.value = o.value + \"!\"; this.getField(\"Code\").value = this.getField(\"Code\").value;");
    fill(&eng, id, "Code", text("abc")).unwrap();
    assert_eq!(field(&eng, id, "Other").value, "x!", "V sees K's write and builds on it");
    assert_eq!(field(&eng, id, "Code").value, "abc");
    // A write that changes nothing is not a change: it must not replay a stale snapshot.
    script(&eng, id, "Code", "V", "var o = this.getField(\"Other\"); o.value = o.value;");
    fill(&eng, id, "Code", text("abd")).unwrap();
    assert_eq!(field(&eng, id, "Other").value, "x");
}

#[test]
fn a_document_that_assigns_an_af_function_by_a_computed_name_gets_its_own() {
    let (eng, id, _) = open("js-override-computed");
    doc_scripts(&eng, id, &[("fixes", "this['AFNumber' + '_Format'] = function (d, s, n, c, cur, p) { event.value = 'custom:' + event.value; };")]);
    add1(&eng, id, 0, nf(FieldKind::Text, "Price", [72.0, 60.0, 240.0, 80.0]));
    script(&eng, id, "Price", "F", "AFNumber_Format(2, 0, 0, 0, \"$\", true);");
    fill(&eng, id, "Price", text("5")).unwrap();
    assert_eq!(field(&eng, id, "Price").shown, "custom:5");
}

#[test]
fn a_script_that_floods_alerts_shows_five_and_says_how_many_it_held_back() {
    let (eng, id, _) = script_form("js-flood", &["Note"]);
    script(&eng, id, "Note", "K", "for (var i = 0; i < 30; i++) { app.alert('message ' + i); }");
    let w = field(&eng, id, "Note");
    let r = set_value_report(&eng, id, w.page, w.xref, &text("x"), EditSource::User).unwrap();
    assert_eq!(r.report.alerts.len(), 5, "{:?}", r.report.alerts);
    assert_eq!(r.report.alerts[0].message, "message 0");
    let log = scripts_state(&eng, id).unwrap().log;
    assert!(log.iter().any(|l| l.message.contains("more alerts from scripts were not shown")), "{log:?}");
}

#[test]
fn a_step_stops_running_scripts_when_it_has_used_its_time_and_counts_a_strike() {
    let names: Vec<String> = (0..16).map(|i| format!("F{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (eng, id, _) = script_form("js-budget", &refs);
    for n in &names {
        fill(&eng, id, n, text("v")).unwrap();
    }
    // Each Format script is slow, but well within its own limit: it is the step as a whole that runs long.
    for n in &names {
        script(&eng, id, n, "F", "var t = Date.now(); while (Date.now() - t < 200) {} event.value = '<' + event.value + '>';");
    }
    let t = std::time::Instant::now();
    let shown = list_all(&eng, id).unwrap().fields.iter().filter(|f| f.shown.starts_with('<')).count();
    let took = t.elapsed();
    assert!(took < std::time::Duration::from_millis(3600), "{took:?}");
    assert!((8..16).contains(&shown), "the first fields were formatted, the rest were left raw: {shown}");
    let log = scripts_state(&eng, id).unwrap().log;
    assert!(log.iter().any(|l| l.message.contains("of script time")), "{log:?}");
    // It counted as one strike, not a timeout of any field: the scripts still run.
    let st = scripts_state(&eng, id).unwrap();
    assert!(st.usable && st.enabled);
}

#[test]
fn the_scripts_banner_does_not_wait_for_a_step_that_holds_the_edit_lane() {
    let (eng, id, _) = script_form("js-lane", &["Src"]);
    script(&eng, id, "Src", "V", "event.rc = true;");
    field(&eng, id, "Src");
    let (tx, rx) = std::sync::mpsc::channel();
    let mut got = None;
    eng.apply_custom(id, "Hold the lane".into(), EditSource::User, false, |_| {
        // Asked from another thread while this step holds the lane; if it needed the lane it would wait for this to end.
        let e2 = eng.clone();
        std::thread::spawn(move || {
            let _ = tx.send(scripts_state(&e2, id).map(|s| s.has_scripts));
        });
        got = Some(rx.recv_timeout(std::time::Duration::from_secs(3)));
        Ok(vec![])
    })
    .unwrap();
    assert!(matches!(got, Some(Ok(Ok(true)))), "scripts_state needed the lane: {got:?}");
}

/// Writes the sample for the runtime checks: `cargo test --lib write_the_form_javascript_sample -- --ignored`.
#[test]
#[ignore = "writes docs/study/samples/_gen/formsjs-sample.pdf"]
fn write_the_form_javascript_sample() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("docs").join("study").join("samples").join("_gen").join("formsjs-sample.pdf");
    std::fs::write(out, formsjs_sample()).unwrap();
}

/// What `scan` was before it skipped pages: every page loaded, every widget listed.
fn scan_by_loading_every_page(doc: &PdfDocument) -> Vec<Entry> {
    let mut out = Vec::new();
    for p in 0..doc.page_count().unwrap() as u32 {
        let page = doc.load_pdf_page(p as i32).unwrap();
        for (i, w) in page.widgets().enumerate() {
            let Some(kind) = w.r#type().ok().and_then(kind_of) else { continue };
            let a = w.annotation();
            let (obj, xref, r) = (a.object(), a.xref().unwrap(), a.rect().unwrap());
            let field = field_of(&obj);
            let own = get(&obj, "T").is_some() || get(&obj, "Parent").is_none();
            let node = if own { xref } else { field.as_indirect().unwrap_or(xref) };
            out.push(Entry { page: p, xref, node, tab: i as u32, kind, name: full_name(&obj), rect: [r.x0, r.y0, r.x1, r.y1], obj, field });
        }
    }
    out
}

fn key(e: &Entry) -> (u32, i32, i32, u32, FieldKind, String, [f32; 4]) {
    (e.page, e.xref, e.node, e.tab, e.kind, e.name.clone(), e.rect)
}

fn scans_agree(eng: &Engine, id: DocId) -> usize {
    let (want, got) = eng
        .read_custom(id, |d| Ok((scan_by_loading_every_page(d).iter().map(key).collect::<Vec<_>>(), scan(d)?.iter().map(key).collect::<Vec<_>>())))
        .unwrap();
    assert_eq!(got, want);
    got.len()
}

#[test]
fn the_field_scan_equals_loading_every_page_on_normal_forms() {
    // Fields of every kind added across both pages of a plain file
    let (eng, id, _) = open("scan-equal");
    let kinds = [FieldKind::Text, FieldKind::Checkbox, FieldKind::Radio, FieldKind::Combo, FieldKind::List, FieldKind::Button, FieldKind::Signature];
    for (i, kind) in kinds.into_iter().enumerate() {
        let y = 100.0 + 40.0 * i as f32;
        add1(&eng, id, (i % 2) as u32, nf(kind, &format!("f{i}"), [72.0, y, 200.0, y + 20.0]));
    }
    assert_eq!(scans_agree(&eng, id), kinds.len());
    for (name, bytes) in [("shared", shared_widgets_form()), ("need", need_appearances_form())] {
        let (eng, id, _) = open_bytes(&format!("scan-{name}"), bytes);
        assert!(scans_agree(&eng, id) >= 2);
    }
    if let Some(bytes) = study_sample("form-source.pdf") {
        let (eng, id, _) = open_bytes("scan-study", bytes);
        assert!(scans_agree(&eng, id) > 0);
    }
}

#[test]
fn two_thousand_pages_sharing_twenty_thousand_annotations_list_their_fields_fast() {
    let list: String = (0..20_000).map(|_| "4 0 R ").collect();
    let widget = |t: &str, y: u32| format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({t}) /Rect [72 {y} 200 {}] /F 4 >>", y + 20);
    // 3 note, 4 square, 5 the shared array, 6 and 7 widgets, 8 their array; pages from 9
    let rest = vec![
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] >>".to_string(),
        "<< /Type /Annot /Subtype /Square /Rect [40 40 90 90] >>".to_string(),
        format!("[3 0 R {list}]"),
        widget("A", 100),
        widget("B", 140),
        "[6 0 R 7 0 R]".to_string(),
    ];
    let first = 3 + rest.len();
    let kids: String = (0..2000).map(|i| format!("{} 0 R ", first + i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R] >> >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count 2000 >>"),
    ];
    objs.extend(rest);
    objs.extend((0..2000).map(|i| {
        let annots = if i == 1234 { 8 } else { 5 };
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots {annots} 0 R >>")
    }));
    let (eng, id, _) = open_bytes("scan-hostile", pdf_of(&objs));
    let t = std::time::Instant::now();
    let form = list_all(&eng, id).unwrap();
    let took = t.elapsed();
    eprintln!("form list, 2000 pages x 20 000 shared annotations: {took:?}");
    assert_eq!(form.fields.iter().map(|f| (f.page, f.name.as_str())).collect::<Vec<_>>(), vec![(1234, "A"), (1234, "B")]);
    assert!(took < std::time::Duration::from_secs(30), "{took:?}");
}

#[test]
fn a_widget_whose_p_names_a_page_sharing_the_array_is_still_listed() {
    // 3 widget whose /P is the last page (object 6), 4 the array both pages share; pages are objects 5, 6
    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (A) /Rect [72 100 200 120] /F 4 /P 6 0 R >>";
    let (eng, id, _) = open_bytes(
        "scan-shared-p",
        pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R] >> >>".to_string(),
            "<< /Type /Pages /Kids [5 0 R 6 0 R] /Count 2 >>".to_string(),
            widget.to_string(),
            "[3 0 R]".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots 4 0 R >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots 4 0 R >>".to_string(),
        ]),
    );
    assert_eq!(list_all(&eng, id).unwrap().fields.iter().map(|f| (f.page, f.name.as_str())).collect::<Vec<_>>(), vec![(0, "A")]);
}

#[test]
fn a_reversed_form_whose_widgets_keep_their_old_p_lists_every_field_fast() {
    // 100 pages of 250 widgets each, every /P naming the page at the mirrored position (a later page that
    // does not list it for the first half): asking that page for its array per widget used to cost
    // widgets x array length and went past the scan cap.
    let (pages, per) = (100usize, 250usize);
    let first = 3 + pages * per;
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", (0..pages).map(|i| format!("{} 0 R ", first + i)).collect::<String>()),
    ];
    for i in 0..pages {
        for j in 0..per {
            objs.push(format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f{i}_{j}) /Rect [72 100 200 120] /F 4 /P {} 0 R >>", first + pages - 1 - i));
        }
    }
    for i in 0..pages {
        let list: String = (0..per).map(|j| format!("{} 0 R ", 3 + i * per + j)).collect();
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{list}] >>"));
    }
    let (eng, id, _) = open_bytes("scan-stale-p", pdf_of(&objs));
    let t = std::time::Instant::now();
    let found = eng.read_custom(id, |d| Ok(scan(d)?.len())).unwrap();
    let took = t.elapsed();
    eprintln!("form scan, {pages} pages x {per} widgets with stale /P: {took:?}");
    assert_eq!(found, pages * per);
    assert!(took < std::time::Duration::from_secs(30), "{took:?}");
    let form = list_all(&eng, id).unwrap();
    assert_eq!(form.fields.len(), pages * per);
    assert!(form.fields.iter().enumerate().all(|(n, f)| f.page == (n / per) as u32));
}

/// 2000 pages whose direct `/Annots` list object 3 (a widget) 1001 times: past the scan budget.
fn repeated_widget_form() -> Vec<u8> {
    let refs = "3 0 R ".repeat(1001);
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R] >> >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count 2000 >>", (0..2000).map(|i| format!("{} 0 R ", 4 + i)).collect::<String>()),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (A) /Rect [72 100 200 120] /F 4 >>".to_string(),
    ];
    objs.extend((0..2000).map(|_| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{refs}] >>")));
    pdf_of(&objs)
}

#[test]
fn a_form_past_the_scan_budget_is_refused_not_listed_with_fields_missing() {
    let (eng, id, _) = open_bytes("scan-repeats", repeated_widget_form());
    let err = match list_all(&eng, id) {
        Err(e) => e.to_string(),
        Ok(f) => panic!("{} fields listed", f.fields.len()),
    };
    assert!(err.contains("too large to edit safely"), "{err}");
}

#[test]
fn a_form_whose_pages_carry_many_unshared_links_lists_every_field() {
    // 300 pages x 200 links of their own (60 000 entries), widgets A and B on page 150
    let (pages, per) = (300usize, 200usize);
    let first = 3 + 2 + pages * per;
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R 4 0 R] >> >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", (0..pages).map(|i| format!("{} 0 R ", first + i)).collect::<String>()),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (A) /Rect [72 100 200 120] /F 4 >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (B) /Rect [72 140 200 160] /F 4 >>".to_string(),
    ];
    objs.extend((0..pages * per).map(|_| "<< /Type /Annot /Subtype /Link /Rect [0 0 20 20] >>".to_string()));
    for i in 0..pages {
        let links: String = (0..per).map(|j| format!("{} 0 R ", 5 + i * per + j)).collect();
        let widgets = if i == 150 { "3 0 R 4 0 R " } else { "" };
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{widgets}{links}] >>"));
    }
    let (eng, id, _) = open_bytes("scan-links", pdf_of(&objs));
    let form = list_all(&eng, id).unwrap();
    assert_eq!(form.fields.iter().map(|f| (f.page, f.name.as_str())).collect::<Vec<_>>(), vec![(150, "A"), (150, "B")]);
}

#[test]
fn a_page_listed_many_times_in_kids_is_scanned_once_and_direct_entries_are_charged() {
    // 2000 /Kids entries naming one page object whose direct /Annots holds 2000 inline widgets
    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (A) /Rect [72 100 200 120] /F 4 >>";
    let kids = "3 0 R ".repeat(2000);
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count 2000 >>"),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{}] >>", vec![widget; 2000].join(" ")),
    ];
    // MuPDF refuses a /Count that is not below the number of objects
    objs.extend((0..2000).map(|_| "null".to_string()));
    let (eng, id, _) = open_bytes("scan-dup-kids", pdf_of(&objs));
    let t = std::time::Instant::now();
    let found = eng.read_custom(id, |d| Ok(scan(d)?.len())).unwrap();
    let took = t.elapsed();
    eprintln!("form scan, one page listed 2000 times with 2000 inline widgets: {took:?}");
    assert_eq!(found, 2000);
    assert!(took < std::time::Duration::from_secs(30), "{took:?}");

    // Inline entries on distinct pages are charged one each: past the budget the form is refused
    let page = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{}] >>", "0 ".repeat(1001));
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count 2000 >>", (0..2000).map(|i| format!("{} 0 R ", 3 + i)).collect::<String>()),
    ];
    objs.extend((0..2000).map(|_| page.clone()));
    let (eng, id, _) = open_bytes("scan-direct", pdf_of(&objs));
    let err = match list_all(&eng, id) {
        Err(e) => e.to_string(),
        Ok(f) => panic!("{} fields listed", f.fields.len()),
    };
    assert!(err.contains("too large to edit safely"), "{err}");
}

// ---- layout tools (layout.rs) -------------------------------------------------------------------

/// Page 1 is rotated a quarter turn and cropped (the crop box does not start at the origin): 732 x 572 points as
/// shown. Page 2 is small (200 x 100).
fn rotated_cropped_form() -> Vec<u8> {
    pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [20 30 592 762] /Rotate 90 /Resources << /Font << /Helv 3 0 R >> >> >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /Helv 3 0 R >> >> >>".to_string(),
    ])
}

fn rotated_form(test: &str, fields: &[(&str, [f32; 4])]) -> (Arc<Engine>, DocId, String) {
    let (eng, id, path) = open_bytes(test, rotated_cropped_form());
    for (n, r) in fields {
        add1(&eng, id, 0, nf(FieldKind::Text, n, *r));
    }
    (eng, id, path)
}

fn items_of(eng: &Engine, id: DocId, names: &[&str]) -> Vec<Item> {
    names
        .iter()
        .map(|n| {
            let f = field(eng, id, n);
            (f.page, f.xref)
        })
        .collect()
}

fn rect_of_name(eng: &Engine, id: DocId, name: &str) -> [f32; 4] {
    field(eng, id, name).rect
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.5
}

fn all_rects(eng: &Engine, id: DocId) -> Vec<(String, [f32; 4])> {
    list_all(eng, id).unwrap().fields.into_iter().map(|f| (f.name, f.rect)).collect()
}

fn same_rects(a: &[(String, [f32; 4])], b: &[(String, [f32; 4])]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.0 == y.0 && (0..4).all(|i| near(x.1[i], y.1[i])))
}

const THREE: [(&str, [f32; 4]); 3] = [("A", [100.0, 100.0, 180.0, 120.0]), ("B", [300.0, 160.0, 340.0, 200.0]), ("C", [500.0, 300.0, 600.0, 330.0])];

#[test]
fn align_centre_distribute_and_match_size_work_in_page_space_each_one_undo_step() {
    let (eng, id, path) = rotated_form("layout-geo", &THREE);
    let ab = items_of(&eng, id, &["A", "B"]);
    let abc = items_of(&eng, id, &["A", "B", "C"]);
    let start = all_rects(&eng, id);

    // The anchor is the last field picked.
    let n = steps(&eng, id);
    align_one(&eng, id, &abc, Edge::Left, EditSource::User).unwrap();
    assert_eq!(steps(&eng, id), n + 1);
    let c = rect_of_name(&eng, id, "C");
    for name in ["A", "B"] {
        let r = rect_of_name(&eng, id, name);
        assert!(near(r[0], c[0]), "{name}: left edge {r:?} vs anchor {c:?}");
    }
    assert!(near(rect_of_name(&eng, id, "A")[2] - rect_of_name(&eng, id, "A")[0], 80.0), "the width is kept");
    assert!(near(c[0], 500.0) && near(c[1], 300.0), "the anchor stays: {c:?}");
    eng.undo(id).unwrap();
    assert!(same_rects(&all_rects(&eng, id), &start), "undo puts every field back exactly");
    eng.redo(id).unwrap();

    // Another anchor, another result (B last): right edges.
    let bac = items_of(&eng, id, &["C", "A", "B"]);
    align_one(&eng, id, &bac, Edge::Right, EditSource::User).unwrap();
    let b = rect_of_name(&eng, id, "B");
    assert!(near(rect_of_name(&eng, id, "A")[2], b[2]) && near(rect_of_name(&eng, id, "C")[2], b[2]));
    // Top, bottom and the two centre lines.
    align_one(&eng, id, &abc, Edge::Top, EditSource::User).unwrap();
    assert!(near(rect_of_name(&eng, id, "A")[1], rect_of_name(&eng, id, "C")[1]));
    align_one(&eng, id, &abc, Edge::Bottom, EditSource::User).unwrap();
    assert!(near(rect_of_name(&eng, id, "B")[3], rect_of_name(&eng, id, "C")[3]));
    align_one(&eng, id, &abc, Edge::HCenter, EditSource::User).unwrap();
    let mid = |n: &str| (rect_of_name(&eng, id, n)[0] + rect_of_name(&eng, id, n)[2]) / 2.0;
    assert!(near(mid("A"), mid("C")) && near(mid("B"), mid("C")));
    align_one(&eng, id, &abc, Edge::VCenter, EditSource::User).unwrap();
    let midy = |n: &str| (rect_of_name(&eng, id, n)[1] + rect_of_name(&eng, id, n)[3]) / 2.0;
    assert!(near(midy("A"), midy("C")) && near(midy("B"), midy("C")));

    // Aligned already: not a step.
    let n = steps(&eng, id);
    align_one(&eng, id, &abc, Edge::VCenter, EditSource::User).unwrap();
    assert_eq!(steps(&eng, id), n);
    assert!(align_one(&eng, id, &abc[..1], Edge::Left, EditSource::User).unwrap_err().to_string().contains("at least 2"));

    // Same size as the anchor, then centred on the page (732 x 572).
    match_size_one(&eng, id, &abc, Dim::Both, EditSource::User).unwrap();
    for name in ["A", "B"] {
        let (r, c) = (rect_of_name(&eng, id, name), rect_of_name(&eng, id, "C"));
        assert!(near(r[2] - r[0], c[2] - c[0]) && near(r[3] - r[1], c[3] - c[1]), "{name}: {r:?} vs {c:?}");
    }
    center_one(&eng, id, &abc, Axis::Both, EditSource::User).unwrap();
    let rs: Vec<[f32; 4]> = ["A", "B", "C"].iter().map(|n| rect_of_name(&eng, id, n)).collect();
    let (x0, x1) = (rs.iter().map(|r| r[0]).fold(f32::MAX, f32::min), rs.iter().map(|r| r[2]).fold(f32::MIN, f32::max));
    let (y0, y1) = (rs.iter().map(|r| r[1]).fold(f32::MAX, f32::min), rs.iter().map(|r| r[3]).fold(f32::MIN, f32::max));
    let b = eng.read_custom(id, |d| { let b = d.load_pdf_page(0)?.bounds()?; Ok([b.x0, b.y0, b.x1, b.y1]) }).unwrap();
    assert!(near((x0 + x1) / 2.0, (b[0] + b[2]) / 2.0) && near((y0 + y1) / 2.0, (b[1] + b[3]) / 2.0), "centred in {b:?}: {x0} {x1} {y0} {y1}");
    assert!(near(b[2] - b[0], 732.0) && near(b[3] - b[1], 572.0), "the page as shown: {b:?}");

    // A nudge moves the group and stops at the page edge.
    let before = rect_of_name(&eng, id, "A");
    nudge_one(&eng, id, &ab, 5.0, -3.0, EditSource::User).unwrap();
    let after = rect_of_name(&eng, id, "A");
    assert!(near(after[0], before[0] + 5.0) && near(after[1], before[1] - 3.0), "{before:?} {after:?}");
    nudge_one(&eng, id, &abc, 100_000.0 / 20.0, 0.0, EditSource::User).unwrap();
    let r = rect_of_name(&eng, id, "A");
    assert!(r[2] <= b[2] + 0.5, "kept on the page: {r:?}");
    let far = ["A", "B", "C"].iter().map(|n| rect_of_name(&eng, id, n)[2]).fold(f32::MIN, f32::max);
    assert!(near(far, b[2]), "the group reached the edge: {far}");

    // Saved, reopened with a fresh MuPDF: the same rectangles, and the raw /Rect is not the page-space one.
    let now = all_rects(&eng, id);
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert!(same_rects(&all_rects(&eng2, id2), &now));
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let raw: Vec<Vec<f32>> = page.widgets().map(|w| nums(&w.annotation().object().get_dict("Rect").unwrap().unwrap())).collect();
    assert_eq!(raw.len(), 3);
    for (r, (_, shown)) in raw.iter().zip(&now) {
        assert!(r.iter().all(|v| v.is_finite()));
        assert!((r[2] - r[0] - (shown[3] - shown[1])).abs() < 0.5, "a quarter turn swaps width and height: {r:?} vs {shown:?}");
    }
}

#[test]
fn every_geometry_tool_is_one_step_that_undoes_exactly_and_survives_save_and_reopen() {
    type Op = fn(&Engine, DocId, &[Item]) -> Result<EditState>;
    let ops: [(&str, Op); 8] = [
        ("align left", |e, i, it| align_one(e, i, it, Edge::Left, EditSource::User)),
        ("align vcenter", |e, i, it| align_one(e, i, it, Edge::VCenter, EditSource::User)),
        ("centre both", |e, i, it| center_one(e, i, it, Axis::Both, EditSource::User)),
        ("centre horizontal", |e, i, it| center_one(e, i, it, Axis::Horizontal, EditSource::User)),
        ("distribute", |e, i, it| distribute_one(e, i, it, Axis::Horizontal, EditSource::User)),
        ("same size", |e, i, it| match_size_one(e, i, it, Dim::Both, EditSource::User)),
        ("same width", |e, i, it| match_size_one(e, i, it, Dim::Width, EditSource::User)),
        ("nudge", |e, i, it| nudge_one(e, i, it, 7.0, -4.0, EditSource::User)),
    ];
    for (n, (what, op)) in ops.iter().enumerate() {
        let (eng, id, path) = rotated_form(&format!("layout-exact-{n}"), &THREE);
        let abc = items_of(&eng, id, &["A", "B", "C"]);
        let start = all_rects(&eng, id);
        let before = steps(&eng, id);
        op(&eng, id, &abc).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(steps(&eng, id), before + 1, "{what} is one step");
        let done = all_rects(&eng, id);
        assert!(!same_rects(&done, &start), "{what} changed something");
        eng.save(id, None).unwrap();
        let (eng2, id2) = reopen(&path);
        assert!(same_rects(&all_rects(&eng2, id2), &done), "{what}: the saved file shows the same rectangles");
        eng.undo(id).unwrap();
        assert!(same_rects(&all_rects(&eng, id), &start), "{what}: undo puts every field back exactly");
        eng.redo(id).unwrap();
        assert!(same_rects(&all_rects(&eng, id), &done), "{what}: redo");
    }
}

#[test]
fn distribute_with_overlapping_fields_spans_to_the_furthest_edge_and_keeps_the_order() {
    // A long field with two short ones inside its span: by leading edge the last is C, but A ends furthest.
    let fields = [("A", [100.0, 100.0, 300.0, 120.0]), ("B", [110.0, 140.0, 150.0, 160.0]), ("C", [200.0, 180.0, 240.0, 200.0])];
    let (eng, id, _) = rotated_form("layout-distribute-overlap", &fields);
    let abc = items_of(&eng, id, &["A", "B", "C"]);
    distribute_one(&eng, id, &abc, Axis::Horizontal, EditSource::User).unwrap();
    let r = |n: &str| rect_of_name(&eng, id, n);
    let (a, b, c) = (r("A"), r("B"), r("C"));
    assert!(near(a[0], 100.0), "the first stays: {a:?}");
    assert!(a[0] <= b[0] && b[0] <= c[0], "the order is kept: {a:?} {b:?} {c:?}");
    assert!(near(b[0] - a[2], c[0] - b[2]), "the gaps are equal: {a:?} {b:?} {c:?}");
    assert!(near(c[2], 300.0), "the span ends at the furthest edge, not at C's own: {c:?}");
    assert!(near(b[2] - b[0], 40.0) && near(c[2] - c[0], 40.0), "no field is resized");
}

#[test]
fn distribute_needs_three_fields_and_makes_the_gaps_equal() {
    let (eng, id, _) = rotated_form(
        "layout-distribute",
        &[("A", [100.0, 100.0, 140.0, 120.0]), ("B", [160.0, 100.0, 260.0, 120.0]), ("C", [300.0, 100.0, 330.0, 120.0]), ("D", [500.0, 100.0, 560.0, 120.0])],
    );
    let two = items_of(&eng, id, &["A", "D"]);
    let n = steps(&eng, id);
    assert!(distribute_one(&eng, id, &two, Axis::Horizontal, EditSource::User).unwrap_err().to_string().contains("at least 3"));
    assert_eq!(steps(&eng, id), n);
    let all = items_of(&eng, id, &["D", "B", "A", "C"]);
    distribute_one(&eng, id, &all, Axis::Horizontal, EditSource::User).unwrap();
    assert_eq!(steps(&eng, id), n + 1);
    let r = |n: &str| rect_of_name(&eng, id, n);
    assert!(near(r("A")[0], 100.0) && near(r("D")[2], 560.0), "the outer two stay");
    let gaps = [r("B")[0] - r("A")[2], r("C")[0] - r("B")[2], r("D")[0] - r("C")[2]];
    assert!(gaps.iter().all(|g| near(*g, gaps[0])), "{gaps:?}");
    assert!(distribute_one(&eng, id, &all, Axis::Both, EditSource::User).is_err());
}

#[test]
fn layout_tools_refuse_a_certified_document_a_covered_field_and_a_signed_signature() {
    for (name, catalog_p, field_p) in [
        ("catalog-p1", Some(1), None),
        ("catalog-p2", Some(2), None),
        ("catalog-p3", Some(3), None),
        ("signature-p2", None, Some(2)),
        ("signature-p3", None, Some(3)),
    ] {
        let (eng, id, _) = script_form(&format!("layout-certified-{name}"), &["A", "B", "C"]);
        add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 400.0, 240.0, 440.0]));
        let abc = items_of(&eng, id, &["A", "B", "C"]);
        let a = items_of(&eng, id, &["A"]);
        if let Some(p) = field_p {
            sign(&eng, id, None, Some(p));
        }
        if let Some(p) = catalog_p {
            certify_catalog(&eng, id, p);
        }
        let before = (steps(&eng, id), all_rects(&eng, id));
        let src = || EditSource::User;
        // The first is a no-op op on a fresh document (everything is left aligned already): still refused.
        let refused = [
            ("align", align_one(&eng, id, &abc, Edge::Left, src()).map(|_| ())),
            ("align noop", align_one(&eng, id, &abc, Edge::Left, src()).map(|_| ())),
            ("centre", center_one(&eng, id, &abc, Axis::Both, src()).map(|_| ())),
            ("distribute", distribute_one(&eng, id, &abc, Axis::Vertical, src()).map(|_| ())),
            ("match size", match_size_one(&eng, id, &abc, Dim::Both, src()).map(|_| ())),
            ("nudge", nudge_one(&eng, id, &abc, 1.0, 1.0, src()).map(|_| ())),
            ("nudge zero", nudge_one(&eng, id, &abc, 0.0, 0.0, src()).map(|_| ())),
            ("duplicate", duplicate_one(&eng, id, a[0], &[1], src()).map(|_| ())),
            ("copies", copies_one(&eng, id, a[0], 2, 2, (4.0, 4.0), src()).map(|_| ())),
            ("rename", rename_one(&eng, id, a[0], "Renamed", false, src()).map(|_| ())),
            ("rename same", rename_one(&eng, id, a[0], "A", false, src()).map(|_| ())),
        ];
        for (what, r) in refused {
            let e = r.expect_err(&format!("{name}: {what} must be refused")).to_string();
            assert!(e.contains("certified"), "{name}: {what}: {e}");
        }
        assert_eq!(before, (steps(&eng, id), all_rects(&eng, id)), "{name}: nothing changed and no step was made");
    }
}

#[test]
fn a_field_a_signature_covers_and_a_signed_signature_field_are_not_laid_out() {
    let (eng, id, _) = script_form("layout-locked", &["Locked", "Free", "Other"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 400.0, 240.0, 440.0]));
    let all = items_of(&eng, id, &["Locked", "Free", "Other"]);
    // Unsigned: a signature field is laid out like any other.
    let sig = items_of(&eng, id, &["Sig"]);
    nudge_one(&eng, id, &sig, 4.0, 0.0, EditSource::User).unwrap();
    sign(&eng, id, Some(("Include", &["Locked"])), None);
    let before = (steps(&eng, id), all_rects(&eng, id));
    let e = align_one(&eng, id, &all, Edge::Left, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("locked by a signature") && e.contains("Locked"), "{e}");
    assert!(rename_one(&eng, id, all[0], "L2", false, EditSource::User).unwrap_err().to_string().contains("locked by a signature"));
    let e = nudge_one(&eng, id, &sig, 4.0, 0.0, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("signed"), "{e}");
    assert_eq!(before, (steps(&eng, id), all_rects(&eng, id)));
    // The fields it does not cover still move.
    align_one(&eng, id, &all[1..], Edge::Right, EditSource::User).unwrap();
}

#[test]
fn a_rename_may_not_take_a_name_a_signature_covers_or_rewrite_a_calculation_it_locks() {
    let (eng, id, _) = script_form("layout-lock-names", &["Free", "Sum", "Other"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 400.0, 240.0, 440.0]));
    patch_field(&eng, id, "Sum", FieldPatch { calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Free".into()] }), ..Default::default() });
    sign(&eng, id, Some(("Include", &["Reserved", "Sum"])), None);
    let it = |n: &str| items_of(&eng, id, &[n])[0];
    let n = steps(&eng, id);
    let e = rename_one(&eng, id, it("Other"), "Reserved", false, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("covered by a signature"), "{e}");
    let e = rename_one(&eng, id, it("Other"), "Reserved.Sub", false, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("covered by a signature"), "{e}");
    // Sum is locked and lists Free: Free may not take another name (the list would have to follow), nor be copied.
    let e = rename_one(&eng, id, it("Free"), "Free2", false, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("calculation of [ Sum ] is locked"), "{e}");
    let e = copies_one(&eng, id, it("Free"), 1, 2, (6.0, 6.0), EditSource::User).unwrap_err().to_string();
    assert!(e.contains("calculation of [ Sum ] is locked"), "{e}");
    assert_eq!(steps(&eng, id), n, "no step, nothing written");
    assert_eq!(field(&eng, id, "Free").name, "Free");
    match field(&eng, id, "Sum").calculate {
        Calculate::Simple { fields, .. } => assert_eq!(fields, vec!["Free".to_string()]),
        other => panic!("{other:?}"),
    }
    // A name nothing covers is fine.
    rename_one(&eng, id, it("Other"), "Elsewhere", false, EditSource::User).unwrap();
}

#[test]
fn the_name_box_follows_the_same_rename_locks_as_the_layout_rename() {
    let (eng, id, _) = script_form("name-box-locks", &["Free", "Sum", "Other"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 400.0, 240.0, 440.0]));
    patch_field(&eng, id, "Sum", FieldPatch { calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Free".into()] }), ..Default::default() });
    sign(&eng, id, Some(("Include", &["Reserved", "Sum"])), None);
    let n = steps(&eng, id);
    let name = |from: &str, to: &str| {
        let w = field(&eng, id, from);
        edit_one(&eng, id, w.page, w.xref, &FieldPatch { name: Some(to.into()), ..Default::default() }, EditSource::User).map_err(|e| e.to_string())
    };
    assert!(name("Other", "Reserved").unwrap_err().contains("covered by a signature"));
    assert!(name("Free", "Free2").unwrap_err().contains("calculation of [ Sum ] is locked"));
    // The field being renamed is guarded too: Sum is covered by the signature, and a signed signature field stays put.
    assert!(name("Sum", "Total").unwrap_err().contains("locked by a signature"));
    assert!(name("Sig", "Sig2").unwrap_err().contains("is signed"));
    assert_eq!(steps(&eng, id), n, "no step, nothing written");
    assert_eq!(field(&eng, id, "Free").name, "Free");
    assert_eq!(field(&eng, id, "Sum").name, "Sum");
    assert_eq!(field(&eng, id, "Sig").name, "Sig");
    name("Other", "Elsewhere").unwrap();
}

#[test]
fn the_name_box_takes_the_same_name_limits_as_the_layout_rename() {
    let (eng, id, _) = script_form("name-box-limits", &["A", "B"]);
    let n = steps(&eng, id);
    let w = field(&eng, id, "A");
    let name = |to: &str| edit_one(&eng, id, w.page, w.xref, &FieldPatch { name: Some(to.into()), ..Default::default() }, EditSource::User).map_err(|e| e.to_string());
    assert!(name("Bad\u{1}Name").unwrap_err().contains("not a usable field name"));
    assert!(name(&"x".repeat(513)).unwrap_err().contains("not a usable field name"));
    assert_eq!(steps(&eng, id), n, "no step, nothing written");
    assert_eq!(field(&eng, id, "A").name, "A");
    name(&"x".repeat(512)).unwrap();
}

#[test]
fn copies_may_not_take_a_name_a_signature_covers() {
    let (eng, id, _) = script_form("copies-lock-names", &["Free", "Other"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 400.0, 240.0, 440.0]));
    sign(&eng, id, Some(("Include", &["Free.1"])), None);
    let n = steps(&eng, id);
    let a = items_of(&eng, id, &["Free"])[0];
    let e = copies_one(&eng, id, a, 1, 3, (6.0, 6.0), EditSource::User).unwrap_err().to_string();
    assert!(e.contains("[ Free.1 ] is covered by a signature"), "{e}");
    assert_eq!(steps(&eng, id), n, "no step, nothing written");
    assert_eq!(field(&eng, id, "Free").name, "Free");
    // Another field's copies are not affected.
    let o = items_of(&eng, id, &["Other"])[0];
    copies_one(&eng, id, o, 1, 2, (6.0, 6.0), EditSource::User).unwrap();
}

#[test]
fn layout_in_a_signed_document_is_saved_after_the_signed_bytes() {
    let (eng, id, path) = script_form("layout-signed", &["A", "B"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 400.0, 240.0, 440.0]));
    sign(&eng, id, None, None);
    eng.save(id, None).unwrap();
    let original = std::fs::read(&path).unwrap();
    let (eng2, id2) = reopen(&path);
    assert!(eng2.read_custom(id2, |d| Ok(crate::outline::signed(d))).unwrap());
    let ab = items_of(&eng2, id2, &["B", "A"]);
    align_one(&eng2, id2, &ab, Edge::Right, EditSource::User).unwrap();
    center_one(&eng2, id2, &ab, Axis::Horizontal, EditSource::User).unwrap();
    eng2.save(id2, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the file was rewritten: the signature would no longer verify");
}

#[test]
fn duplicate_copies_and_rename_in_a_signed_document_are_saved_after_the_signed_bytes() {
    let (eng, id, path) = script_form("layout-signed-more", &["A", "B"]);
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [72.0, 400.0, 240.0, 440.0]));
    sign(&eng, id, None, None);
    eng.save(id, None).unwrap();
    let original = std::fs::read(&path).unwrap();
    let (eng2, id2) = reopen(&path);
    assert!(eng2.read_custom(id2, |d| Ok(crate::outline::signed(d))).unwrap());
    let n = steps(&eng2, id2);
    let a = items_of(&eng2, id2, &["A"])[0];
    let b = items_of(&eng2, id2, &["B"])[0];
    duplicate_one(&eng2, id2, a, &[1], EditSource::User).unwrap();
    copies_one(&eng2, id2, b, 2, 2, (4.0, 4.0), EditSource::User).unwrap();
    rename_one(&eng2, id2, a, "Renamed", false, EditSource::User).unwrap();
    assert_eq!(steps(&eng2, id2), n + 3);
    eng2.save(id2, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the file was rewritten: the signature would no longer verify");
    let (eng3, id3) = reopen(&path);
    assert_eq!(items_of(&eng3, id3, &["Renamed"]).len(), 1);
}

#[test]
fn layout_tools_hold_their_caps() {
    let (eng, id, _) = script_form("layout-caps", &["A", "B"]);
    let a = items_of(&eng, id, &["A"])[0];
    let n = steps(&eng, id);
    let many: Vec<Item> = std::iter::repeat(a).take(1001).collect();
    let e = align_one(&eng, id, &many, Edge::Left, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("at most 1000"), "{e}");
    let e = copies_one(&eng, id, a, 20, 26, (1.0, 1.0), EditSource::User).unwrap_err().to_string();
    assert!(e.contains("at most 500"), "{e}");
    // 501 places is one over; 500 passes the cap (here nearly all of them run off the page and are skipped).
    let e = copies_one(&eng, id, a, 3, 167, (1.0, 1.0), EditSource::User).unwrap_err().to_string();
    assert!(e.contains("at most 500"), "{e}");
    assert_eq!(steps(&eng, id), n, "no step");
    let (_, done) = copies_one(&eng, id, a, 2, 250, (900.0, 0.0), EditSource::User).unwrap();
    assert_eq!(done, Copied { made: 1, skipped: 498 });
    let n = steps(&eng, id);
    let pages: Vec<u32> = (1..=2001).collect();
    let e = duplicate_one(&eng, id, a, &pages, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("at most 2000"), "{e}");
    assert_eq!(steps(&eng, id), n, "no step");
}

fn form_fields_len(eng: &Engine, id: DocId) -> usize {
    eng.read_custom(id, |d| Ok(get(&d.catalog()?.get_dict("AcroForm")?.unwrap(), "Fields").map_or(0, |f| f.len().unwrap_or(0)))).unwrap()
}

fn layout_form(test: &str) -> (Arc<Engine>, DocId, String) {
    let (eng, id, path) = open(test);
    let opts = vec![Opt { export: "se".into(), label: "Sweden".into() }, Opt { export: "no".into(), label: "Norway".into() }];
    add1(&eng, id, 0, nf(FieldKind::Text, "Name", [72.0, 100.0, 272.0, 120.0]));
    add1(&eng, id, 0, NewField { options: Some(opts), ..nf(FieldKind::Combo, "Country", [72.0, 140.0, 220.0, 160.0]) });
    add1(&eng, id, 0, nf(FieldKind::Text, "Notes", [72.0, 180.0, 272.0, 240.0]));
    add1(&eng, id, 0, nf(FieldKind::Checkbox, "Agree", [72.0, 260.0, 86.0, 274.0]));
    patch_field(&eng, id, "Notes", FieldPatch { multiline: Some(true), ..Default::default() });
    (eng, id, path)
}

#[test]
fn a_field_duplicated_to_other_pages_is_one_field_with_more_widgets() {
    let (eng, id, path) = layout_form("layout-dup");
    fill(&eng, id, "Name", text("Alice")).unwrap();
    let before = all_rects(&eng, id);
    let name = items_of(&eng, id, &["Name"])[0];
    let n = steps(&eng, id);
    duplicate_one(&eng, id, name, &[1], EditSource::User).unwrap();
    assert_eq!(steps(&eng, id), n + 1);
    let ws = widgets_named(&eng, id, "Name");
    assert_eq!(ws.len(), 2, "{ws:?}");
    assert_eq!(ws[0].node, ws[1].node, "one field");
    assert_eq!((ws[0].page, ws[1].page), (0, 1));
    assert!((0..4).all(|i| near(ws[0].rect[i], ws[1].rect[i])), "the same place");
    assert!(ws.iter().all(|w| w.value == "Alice"), "one value: {ws:?}");
    // Typing in one is typing in both.
    fill(&eng, id, "Name", text("Bob")).unwrap();
    assert!(widgets_named(&eng, id, "Name").iter().all(|w| w.value == "Bob"));
    // Editing a property afterwards keeps the field's kind, flags and options (they moved to the parent).
    for (nm, kind) in [("Country", FieldKind::Combo), ("Notes", FieldKind::Text)] {
        let it = items_of(&eng, id, &[nm])[0];
        duplicate_one(&eng, id, it, &[1], EditSource::User).unwrap();
        assert_eq!(patch_field(&eng, id, nm, FieldPatch { required: Some(true), ..Default::default() }), 1);
        let w = widgets_named(&eng, id, nm);
        assert_eq!(w.len(), 2, "{nm}");
        assert!(w.iter().all(|w| w.kind == kind && w.required), "{nm}: {w:?}");
    }
    assert!(widgets_named(&eng, id, "Notes").iter().all(|w| w.multiline));
    assert_eq!(widgets_named(&eng, id, "Country")[0].options.len(), 2);
    fill(&eng, id, "Country", text("no")).unwrap();
    assert!(widgets_named(&eng, id, "Country").iter().all(|w| w.value == "no"));
    // A check box: turning it on turns both widgets on.
    let agree = items_of(&eng, id, &["Agree"])[0];
    duplicate_one(&eng, id, agree, &[1], EditSource::User).unwrap();
    fill(&eng, id, "Agree", FieldValue::Checked(true)).unwrap();
    let a = widgets_named(&eng, id, "Agree");
    assert_eq!(a.len(), 2);
    assert!(a.iter().all(|w| w.checked), "{a:?}");
    assert!(duplicate_one(&eng, id, name, &[0], EditSource::User).unwrap_err().to_string().contains("other page"));
    assert!(duplicate_one(&eng, id, name, &[7], EditSource::User).unwrap_err().to_string().contains("no page 8"));

    // Undo takes the widgets off again, exactly.
    for _ in 0..9 {
        eng.undo(id).unwrap();
    }
    assert!(same_rects(&all_rects(&eng, id), &before), "undo takes every widget off again");
    for _ in 0..9 {
        eng.redo(id).unwrap();
    }
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let w = list_all(&eng2, id2).unwrap().fields;
    assert_eq!(w.iter().filter(|f| f.name == "Name").count(), 2);
    assert_eq!(w.iter().filter(|f| f.name == "Name" && f.page == 1).count(), 1);
    assert!(w.iter().filter(|f| f.name == "Name").all(|f| f.value == "Bob"));
    assert!(!ap_text(&path, "Name", None).is_empty());
}

/// The raw `/MK /R` and the `/MediaBox` width of the `/P` page of the widget named `name` on `page`.
fn widget_turn_and_page(path: &str, page: u32, name: &str) -> (Option<i32>, Option<f32>) {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    for w in doc.load_pdf_page(page as i32).unwrap().widgets() {
        if w.name().unwrap().as_deref() != Some(name) {
            continue;
        }
        let o = w.annotation().object();
        let r = o.get_dict("MK").unwrap().and_then(|m| m.get_dict("R").unwrap()).and_then(|r| r.as_int().ok());
        let width = o.get_dict("P").unwrap().and_then(|p| p.get_dict("MediaBox").unwrap()).map(|b| nums(&b)[2]);
        return (r, width);
    }
    panic!("no widget {name} on page {page}")
}

#[test]
fn a_duplicate_names_its_page_and_turns_the_widget_with_the_difference_in_page_rotation() {
    // Page 1 is rotated 90 (612 wide); page 2 is not (200 wide).
    let (eng, id, path) = rotated_form("layout-dup-rot", &[("Down", [100.0, 100.0, 180.0, 120.0])]);
    let set_turn = |name: &str, page: u32, r: i32| {
        eng.apply_custom(id, "mk".into(), EditSource::User, false, |d| {
            let entries = scan(d)?;
            let e = entries.iter().find(|e| e.name == name && e.page == page).unwrap();
            let mut w = d.new_indirect(e.xref, 0)?;
            let mut mk = d.new_dict()?;
            mk.dict_put("R", PdfObject::new_int(r)?)?;
            w.dict_put("MK", mk)?;
            Ok(vec![page])
        })
        .unwrap();
    };
    set_turn("Down", 0, 90);
    let it = items_of(&eng, id, &["Down"])[0];
    duplicate_one(&eng, id, it, &[1], EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(widget_turn_and_page(&path, 0, "Down").0, Some(90), "the original is as it was");
    let (r, width) = widget_turn_and_page(&path, 1, "Down");
    assert_eq!(r, Some(0), "90 on a page turned 90 is upright; on an unturned page it is 0");
    assert_eq!(width, Some(200.0), "/P is the page it is on");

    // The other way: from the plain page to the turned one.
    add1(&eng, id, 1, nf(FieldKind::Text, "Up", [10.0, 10.0, 60.0, 30.0]));
    set_turn("Up", 1, 0);
    let up = items_of(&eng, id, &["Up"])[0];
    duplicate_one(&eng, id, up, &[0], EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(widget_turn_and_page(&path, 0, "Up"), (Some(90), Some(612.0)));
}

#[test]
fn layout_in_page_space_holds_on_every_page_rotation() {
    // Inside the smaller side of the page as shown (572) whichever way it is turned.
    const FIT: [(&str, [f32; 4]); 3] =
        [("A", [50.0, 60.0, 130.0, 80.0]), ("B", [200.0, 200.0, 240.0, 240.0]), ("C", [400.0, 300.0, 480.0, 330.0])];
    for rot in [0, 90, 180, 270] {
        let bytes = pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [20 30 592 762] /Rotate {rot} /Resources << /Font << /Helv 3 0 R >> >> >>"),
        ]);
        let (eng, id, path) = open_bytes(&format!("layout-rot-{rot}"), bytes);
        for (n, r) in FIT {
            add1(&eng, id, 0, nf(FieldKind::Text, n, r));
        }
        let (abc, ab) = (items_of(&eng, id, &["A", "B", "C"]), items_of(&eng, id, &["A", "B"]));
        let start = all_rects(&eng, id);
        // What the page shows: the rectangles come back as they were given, whatever the rotation.
        for (n, r) in FIT {
            assert!((0..4).all(|i| near(rect_of_name(&eng, id, n)[i], r[i])), "{rot}: {n}");
        }
        align_one(&eng, id, &abc, Edge::Left, EditSource::User).unwrap();
        for n in ["A", "B"] {
            assert!(near(rect_of_name(&eng, id, n)[0], 400.0), "{rot}: {n} on the anchor's left edge");
        }
        distribute_one(&eng, id, &abc, Axis::Vertical, EditSource::User).unwrap();
        let (a, b, c) = (rect_of_name(&eng, id, "A"), rect_of_name(&eng, id, "B"), rect_of_name(&eng, id, "C"));
        assert!(near(b[1] - a[3], c[1] - b[3]), "{rot}: equal gaps {a:?} {b:?} {c:?}");
        let before = rect_of_name(&eng, id, "A");
        nudge_one(&eng, id, &ab, 5.0, -3.0, EditSource::User).unwrap();
        let after = rect_of_name(&eng, id, "A");
        assert!(near(after[0], before[0] + 5.0) && near(after[1], before[1] - 3.0), "{rot}: nudge right and up as shown {before:?} {after:?}");
        center_one(&eng, id, &abc, Axis::Both, EditSource::User).unwrap();
        let bounds = eng.read_custom(id, |d| { let b = d.load_pdf_page(0)?.bounds()?; Ok([b.x0, b.y0, b.x1, b.y1]) }).unwrap();
        let rs: Vec<[f32; 4]> = ["A", "B", "C"].iter().map(|n| rect_of_name(&eng, id, n)).collect();
        let x = (rs.iter().map(|r| r[0]).fold(f32::MAX, f32::min) + rs.iter().map(|r| r[2]).fold(f32::MIN, f32::max)) / 2.0;
        let y = (rs.iter().map(|r| r[1]).fold(f32::MAX, f32::min) + rs.iter().map(|r| r[3]).fold(f32::MIN, f32::max)) / 2.0;
        assert!(near(x, (bounds[0] + bounds[2]) / 2.0) && near(y, (bounds[1] + bounds[3]) / 2.0), "{rot}: centred in {bounds:?}");
        let swapped = rot == 90 || rot == 270;
        assert!(near(bounds[2] - bounds[0], if swapped { 732.0 } else { 572.0 }), "{rot}: the page as shown {bounds:?}");
        // Saved and reopened with a fresh MuPDF: the same rectangles; undone: the start.
        let now = all_rects(&eng, id);
        eng.save(id, None).unwrap();
        let (eng2, id2) = reopen(&path);
        assert!(same_rects(&all_rects(&eng2, id2), &now), "{rot}: reopened");
        for _ in 0..4 {
            eng.undo(id).unwrap();
        }
        assert!(same_rects(&all_rects(&eng, id), &start), "{rot}: undone exactly");
    }
}

#[test]
fn a_duplicated_widget_goes_on_its_own_page_when_pages_share_an_annots_array() {
    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Shared) /Rect [72 600 200 620] /F 4 /DA (/Helv 0 Tf 0 g) >>";
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots 7 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots 7 0 R >>".to_string(),
        widget.to_string(),
        "[6 0 R]".to_string(),
    ]);
    let (eng, id, _) = open_bytes("layout-dup-shared", bytes);
    let w = items_of(&eng, id, &["Shared"])[0];
    assert_eq!(w.0, 0);
    duplicate_one(&eng, id, w, &[1], EditSource::User).unwrap();
    let ws = widgets_named(&eng, id, "Shared");
    assert_eq!(ws.len(), 2, "{ws:?}");
    assert_eq!(ws.iter().filter(|f| f.page == 0).count(), 1, "the first page did not gain one");
    assert_eq!(ws.iter().filter(|f| f.page == 1).count(), 1);
}

#[test]
fn a_page_too_small_for_the_field_refuses_the_duplicate_and_radios_and_signatures_are_not_copied() {
    let (eng, id, _) = rotated_form("layout-dup-small", &[("Wide", [100.0, 20.0, 400.0, 40.0])]);
    add1(&eng, id, 0, nf(FieldKind::Radio, "Pick", [100.0, 60.0, 114.0, 74.0]));
    add1(&eng, id, 0, nf(FieldKind::Signature, "Sig", [100.0, 100.0, 180.0, 130.0]));
    let n = steps(&eng, id);
    let e = duplicate_one(&eng, id, items_of(&eng, id, &["Wide"])[0], &[1], EditSource::User).unwrap_err().to_string();
    assert!(e.contains("does not fit on page 2"), "{e}");
    assert!(duplicate_one(&eng, id, items_of(&eng, id, &["Pick"])[0], &[1], EditSource::User).unwrap_err().to_string().contains("radio"));
    assert!(duplicate_one(&eng, id, items_of(&eng, id, &["Sig"])[0], &[1], EditSource::User).unwrap_err().to_string().contains("signature"));
    assert!(copies_one(&eng, id, items_of(&eng, id, &["Pick"])[0], 2, 1, (2.0, 2.0), EditSource::User).is_err());
    assert_eq!(steps(&eng, id), n);
}

#[test]
fn copies_make_a_grid_named_after_the_field_clipped_to_the_page() {
    let (eng, id, path) = rotated_form("layout-copies", &[("Cell", [100.0, 100.0, 160.0, 120.0]), ("Total", [100.0, 400.0, 200.0, 420.0])]);
    patch_field(&eng, id, "Total", FieldPatch { calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Cell".into()] }), ..Default::default() });
    let before = all_rects(&eng, id);
    let n = steps(&eng, id);
    let (state, done) = copies_one(&eng, id, items_of(&eng, id, &["Cell"])[0], 2, 3, (10.0, 6.0), EditSource::User).unwrap();
    let _ = state;
    assert_eq!(done, Copied { made: 5, skipped: 0 });
    assert_eq!(steps(&eng, id), n + 1);
    let names: Vec<String> = list_all(&eng, id).unwrap().fields.into_iter().map(|f| f.name).filter(|n| n.starts_with("Cell")).collect();
    for i in 0..6 {
        assert!(names.contains(&format!("Cell.{i}")), "{names:?}");
    }
    assert!(!names.contains(&"Cell".to_string()));
    // Row by row, to the right and down, with the gaps between.
    let r = |n: &str| rect_of_name(&eng, id, n);
    assert!(near(r("Cell.0")[0], 100.0) && near(r("Cell.1")[0], 170.0) && near(r("Cell.2")[0], 240.0), "{:?}", r("Cell.1"));
    assert!(near(r("Cell.3")[0], 100.0) && near(r("Cell.3")[1], 126.0), "{:?}", r("Cell.3"));
    assert!(near(r("Cell.5")[2] - r("Cell.5")[0], 60.0));
    // The calculation follows the original to its new name; the copies are independent fields.
    match field(&eng, id, "Total").calculate {
        Calculate::Simple { fields, .. } => assert_eq!(fields, vec!["Cell.0".to_string()]),
        other => panic!("{other:?}"),
    }
    fill(&eng, id, "Cell.2", text("7")).unwrap();
    assert_eq!(field(&eng, id, "Cell.2").value, "7");
    assert_eq!(field(&eng, id, "Cell.0").value, "");
    assert_eq!(field(&eng, id, "Cell.4").value, "");
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    assert!(same_rects(&all_rects(&eng, id), &before), "undo takes the grid off and the name back");
    assert_eq!(field(&eng, id, "Cell").name, "Cell");
    eng.redo(id).unwrap();
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let back = list_all(&eng2, id2).unwrap().fields;
    assert_eq!(back.iter().filter(|f| f.name.starts_with("Cell.")).count(), 6);
    assert_eq!(back.iter().find(|f| f.name == "Cell.2").unwrap().value, "7");

    // Nothing fits past the right edge: those places are skipped and said so; all of them skipped is an error.
    let (_, done) = copies_one(&eng, id, items_of(&eng, id, &["Cell.5"])[0], 1, 4, (300.0, 0.0), EditSource::User).unwrap();
    assert_eq!(done, Copied { made: 1, skipped: 2 });
    let e = copies_one(&eng, id, items_of(&eng, id, &["Cell.4"])[0], 1, 2, (900.0, 0.0), EditSource::User).unwrap_err().to_string();
    assert!(e.contains("no copy fits"), "{e}");
}

#[test]
fn copies_of_a_field_node_with_one_widget_keep_its_format_on_the_original_and_every_copy() {
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [6 0 R] >>".to_string(),
        "<< /FT /Tx /T (Amt) /V (5) /Kids [6 0 R] /AA << /F << /S /JavaScript /JS (AFNumber_Format(2, 0, 0, 0, \"\", true);) >> >> >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [72 600 140 620] /F 4 /DA (/Helv 0 Tf 0 g) >>".to_string(),
    ]);
    let (eng, id, path) = open_bytes("layout-copies-node", bytes);
    let amt = items_of(&eng, id, &["Amt"])[0];
    let fmt = field(&eng, id, "Amt").format;
    assert!(matches!(fmt, Format::Number { .. }), "{fmt:?}");
    let (_, done) = copies_one(&eng, id, amt, 1, 3, (10.0, 0.0), EditSource::User).unwrap();
    assert_eq!(done.made, 2);
    for n in ["Amt.0", "Amt.1", "Amt.2"] {
        assert_eq!(field(&eng, id, n).format, fmt, "{n} keeps the format");
    }
    assert_eq!(field(&eng, id, "Amt.0").value, "5", "the original keeps its value");
    eng.undo(id).unwrap();
    assert_eq!(field(&eng, id, "Amt").format, fmt);
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(field(&eng2, id2, "Amt.2").format, fmt);
    assert_eq!(field(&eng2, id2, "Amt.0").format, fmt);
}

#[test]
fn copies_leave_the_originals_shared_actions_alone() {
    // /AA is an indirect dictionary; its /C is an action with no script, so the field still counts as copyable.
    // /T is the leaf: the widgets are the kids 0, 1, 2 of the group Amt.
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R] >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Amt) /Rect [72 600 140 620] /F 4 /DA (/Helv 0 Tf 0 g) /AA 6 0 R >>".to_string(),
        "<< /F << /S /JavaScript /JS (AFNumber_Format(2, 0, 0, 0, \"\", true);) >> /C << /S /ResetForm >> >>".to_string(),
    ]);
    let (eng, id, path) = open_bytes("layout-copies-shared-aa", bytes);
    let amt = items_of(&eng, id, &["Amt"])[0];
    let (_, done) = copies_one(&eng, id, amt, 1, 3, (10.0, 0.0), EditSource::User).unwrap();
    assert_eq!(done.made, 2);
    eng.save(id, None).unwrap();
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let mut seen = std::collections::BTreeMap::new();
    for w in doc.load_pdf_page(0).unwrap().widgets() {
        let o = w.annotation().object();
        let aa = o.get_dict("AA").unwrap().expect("every widget keeps its actions");
        let t = super::text(&o.get_dict("T").unwrap().unwrap()).unwrap();
        seen.insert(t, (aa.get_dict("F").unwrap().is_some(), aa.get_dict("C").unwrap().is_some()));
    }
    let want: std::collections::BTreeMap<String, (bool, bool)> =
        [("0", (true, true)), ("1", (true, false)), ("2", (true, false))].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    assert_eq!(seen, want, "the original keeps /F and /C, a copy has /F and no /C");
}

#[test]
fn giving_a_field_a_second_widget_moves_only_the_fields_actions_up_and_keeps_the_widgets_triggers() {
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R] >>".to_string(),
        concat!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Amt) /V (5) /Rect [72 600 140 620] /F 4 /DA (/Helv 0 Tf 0 g) ",
            "/AA << /F << /S /JavaScript /JS (AFNumber_Format(2, 0, 0, 0, \"\", true);) >> ",
            "/E << /S /JavaScript /JS (app.alert\\(\"in\"\\);) >> /Fo << /S /JavaScript /JS (app.beep\\(0\\);) >> >> >>"
        )
        .to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
    ]);
    let (eng, id, path) = open_bytes("layout-aa-split", bytes);
    let amt = items_of(&eng, id, &["Amt"])[0];
    let fmt = field(&eng, id, "Amt").format;
    assert!(matches!(fmt, Format::Number { .. }), "{fmt:?}");
    duplicate_one(&eng, id, amt, &[1], EditSource::User).unwrap();
    assert!(widgets_named(&eng, id, "Amt").iter().all(|w| w.format == fmt), "the format is the field's, on both widgets");
    eng.save(id, None).unwrap();
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let w = doc.load_pdf_page(0).unwrap().widgets().next().unwrap().annotation().object();
    let keys = |o: &PdfObject| -> Vec<String> {
        o.dict_iter().unwrap().filter_map(|kv| kv.ok()).filter_map(|(k, _)| super::text(&k)).collect()
    };
    let own = keys(&w.get_dict("AA").unwrap().expect("the widget keeps its triggers"));
    assert!(own.contains(&"E".to_string()) && own.contains(&"Fo".to_string()) && !own.contains(&"F".to_string()), "{own:?}");
    let up = keys(&w.get_dict("Parent").unwrap().unwrap().get_dict("AA").unwrap().expect("the field's actions"));
    assert_eq!(up, vec!["F".to_string()]);
}

#[test]
fn copies_are_refused_for_a_field_with_several_widgets_or_a_calculation() {
    let (eng, id, _) = open_bytes("layout-copies-refused", shared_widgets_form());
    let n = steps(&eng, id);
    let sum = items_of(&eng, id, &["Sum"])[0];
    let e = copies_one(&eng, id, sum, 2, 1, (4.0, 4.0), EditSource::User).unwrap_err().to_string();
    assert!(e.contains("several widgets"), "{e}");
    let (eng2, id2, _) = script_form("layout-copies-calc", &["Rent", "Total"]);
    patch_field(&eng2, id2, "Total", FieldPatch { calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Rent".into()] }), ..Default::default() });
    let e = copies_one(&eng2, id2, items_of(&eng2, id2, &["Total"])[0], 2, 1, (4.0, 4.0), EditSource::User).unwrap_err().to_string();
    assert!(e.contains("calculated"), "{e}");
    assert_eq!(steps(&eng, id), n);
}

#[test]
fn rename_keeps_dotted_names_valid_and_merges_only_fields_of_one_kind() {
    let (eng, id, path) = layout_form("layout-rename");
    add1(&eng, id, 0, nf(FieldKind::Text, "Other", [72.0, 300.0, 200.0, 320.0]));
    add1(&eng, id, 0, nf(FieldKind::Text, "Sum", [72.0, 340.0, 200.0, 360.0]));
    patch_field(&eng, id, "Sum", FieldPatch { calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Other".into()] }), ..Default::default() });
    let it = |n: &str| items_of(&eng, id, &[n])[0];
    let rename = |n: &str, to: &str, merge: bool| rename_one(&eng, id, it(n), to, merge, EditSource::User);
    let top = form_fields_len(&eng, id);

    // A new group, a second field into it, and the calculation follows.
    let n = steps(&eng, id);
    rename("Other", "Grp.Other", false).unwrap();
    assert_eq!(steps(&eng, id), n + 1);
    assert_eq!(field(&eng, id, "Grp.Other").kind, FieldKind::Text);
    match field(&eng, id, "Sum").calculate {
        Calculate::Simple { fields, .. } => assert_eq!(fields, vec!["Grp.Other".to_string()]),
        other => panic!("{other:?}"),
    }
    assert_eq!(form_fields_len(&eng, id), top, "the field left /Fields for its group");
    rename("Name", "Grp.Name", false).unwrap();
    assert_eq!(form_fields_len(&eng, id), top - 1);
    rename("Grp.Name", "Deep.er.Name", false).unwrap();
    rename("Deep.er.Name", "Name", false).unwrap();
    assert_eq!(form_fields_len(&eng, id), top, "the emptied groups are gone");
    eng.undo(id).unwrap();
    assert_eq!(field(&eng, id, "Deep.er.Name").name, "Deep.er.Name");
    eng.redo(id).unwrap();

    // Refusals: nothing changes and there is no step.
    let n = steps(&eng, id);
    let err = |n: &str, to: &str, merge: bool| rename(n, to, merge).unwrap_err().to_string();
    assert!(err("Notes", "Name", false).contains("already exists"));
    assert!(err("Notes", "Agree", true).contains("cannot take that name"), "different kinds never merge");
    assert!(err("Name", "Grp", false).contains("group of fields"));
    assert!(err("Name", "Notes.x", false).contains("no field can be named below"));
    assert!(err("Name", "", false).contains("needs a name"));
    assert!(err("Name", "a..b", false).contains("needs a name"));
    assert!(err("Name", ".x", false).contains("needs a name"));
    assert_eq!(steps(&eng, id), n);
    // The same name is not a step.
    rename("Name", "Name", false).unwrap();
    assert_eq!(steps(&eng, id), n);

    // Merge: same kind, with the target's value kept; one field of two widgets.
    fill(&eng, id, "Name", text("Alice")).unwrap();
    fill(&eng, id, "Notes", text("a note")).unwrap();
    rename("Notes", "Name", true).unwrap();
    let ws = widgets_named(&eng, id, "Name");
    assert_eq!(ws.len(), 2, "{ws:?}");
    assert_eq!(ws[0].node, ws[1].node);
    assert!(ws.iter().all(|w| w.value == "Alice"), "{ws:?}");
    assert!(widgets_named(&eng, id, "Notes").is_empty());
    eng.undo(id).unwrap();
    assert_eq!(widgets_named(&eng, id, "Notes").len(), 1);
    assert_eq!(widgets_named(&eng, id, "Name").len(), 1);
    eng.redo(id).unwrap();
    // Editing a property of the merged field keeps it a text field.
    assert_eq!(patch_field(&eng, id, "Name", FieldPatch { required: Some(true), ..Default::default() }), 1);
    assert!(widgets_named(&eng, id, "Name").iter().all(|w| w.required && w.kind == FieldKind::Text));

    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let back = list_all(&eng2, id2).unwrap().fields;
    assert_eq!(back.iter().filter(|f| f.name == "Name").count(), 2);
    assert!(back.iter().any(|f| f.name == "Grp.Other"));
}

/// The appearance text and the `/AS` of every widget named `name`, on every page, as written to the file.
fn widget_looks(path: &str, name: &str) -> Vec<(String, Option<String>)> {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    let mut out = Vec::new();
    for p in 0..doc.page_count().unwrap() {
        for w in doc.load_pdf_page(p).unwrap().widgets() {
            if w.name().unwrap().as_deref() != Some(name) {
                continue;
            }
            let o = w.annotation().object();
            let n = o.get_dict("AP").unwrap().and_then(|ap| ap.get_dict("N").unwrap());
            let shown = match &n {
                Some(n) if n.is_stream().unwrap_or(false) => String::from_utf8_lossy(&n.read_stream().unwrap()).into_owned(),
                _ => String::new(),
            };
            let state = o.get_dict("AS").unwrap().and_then(|s| super::text(&s));
            out.push((shown, state));
        }
    }
    out
}

#[test]
fn a_merged_widget_is_redrawn_with_the_targets_value_and_a_check_boxes_state_follows_it() {
    let (eng, id, path) = layout_form("layout-merge-look");
    add1(&eng, id, 0, nf(FieldKind::Text, "Title", [72.0, 300.0, 272.0, 320.0]));
    add1(&eng, id, 0, nf(FieldKind::Checkbox, "Agree2", [100.0, 260.0, 114.0, 274.0]));
    fill(&eng, id, "Title", text("Mr")).unwrap();
    fill(&eng, id, "Name", text("Alice")).unwrap();
    let it = |n: &str| items_of(&eng, id, &[n])[0];
    rename_one(&eng, id, it("Title"), "Name", true, EditSource::User).unwrap();
    // A checked box merged into an unchecked one is unchecked; the other way round it is checked.
    fill(&eng, id, "Agree2", FieldValue::Checked(true)).unwrap();
    rename_one(&eng, id, it("Agree2"), "Agree", true, EditSource::User).unwrap();
    // Checked target, unchecked box merged into it: both show the on state, not Off.
    add1(&eng, id, 0, nf(FieldKind::Checkbox, "Tick", [140.0, 260.0, 154.0, 274.0]));
    add1(&eng, id, 0, nf(FieldKind::Checkbox, "Tick2", [180.0, 260.0, 194.0, 274.0]));
    fill(&eng, id, "Tick", FieldValue::Checked(true)).unwrap();
    rename_one(&eng, id, it("Tick2"), "Tick", true, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    let ticks = widget_looks(&path, "Tick");
    assert_eq!(ticks.len(), 2);
    assert!(ticks.iter().all(|(_, s)| s.as_deref().is_some_and(|s| s != "Off")), "the merged box shows the on state: {ticks:?}");
    assert!(widgets_named(&eng, id, "Tick").iter().all(|w| w.checked));
    let names = widget_looks(&path, "Name");
    assert_eq!(names.len(), 2);
    assert!(names.iter().all(|(shown, _)| shown.contains("Alice")), "both widgets show the target's value: {names:?}");
    let boxes = widget_looks(&path, "Agree");
    assert_eq!(boxes.len(), 2);
    assert!(boxes.iter().all(|(_, s)| s.as_deref() == Some("Off")), "the merged check box follows the target's /V: {boxes:?}");
    assert!(widgets_named(&eng, id, "Agree").iter().all(|w| !w.checked));
}

#[test]
fn renaming_a_calculated_field_into_a_group_keeps_its_place_in_the_calculation_order() {
    let (eng, id, path) = script_form("layout-rename-co", &["X", "Y", "Z"]);
    for (i, n) in ["X", "Y", "Z"].iter().enumerate() {
        let calc = Calculate::Custom(format!("event.value = {i};"));
        patch_field(&eng, id, n, FieldPatch { calculate: Some(calc), ..Default::default() });
    }
    let nodes = |names: &[&str]| names.iter().map(|n| field(&eng, id, n).node).collect::<Vec<_>>();
    let order = nodes(&["X", "Y", "Z"]);
    assert_eq!(list_all(&eng, id).unwrap().calc_order, order);
    let x = items_of(&eng, id, &["X"])[0];
    rename_one(&eng, id, x, "Grp.X", false, EditSource::User).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().calc_order, order, "X is first still");
    // Out of the group again, and the middle one into another.
    let gx = items_of(&eng, id, &["Grp.X"])[0];
    rename_one(&eng, id, gx, "X", false, EditSource::User).unwrap();
    let y = items_of(&eng, id, &["Y"])[0];
    rename_one(&eng, id, y, "Other.Y", false, EditSource::User).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().calc_order, order);
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(list_all(&eng2, id2).unwrap().calc_order.len(), 3);
}

#[test]
fn renaming_one_widget_of_a_field_renames_the_field() {
    let (eng, id, _) = open_bytes("layout-rename-kids", shared_widgets_form());
    let w = widgets_named(&eng, id, "agree");
    assert_eq!(w.len(), 3);
    rename_one(&eng, id, (w[1].page, w[1].xref), "consent", false, EditSource::User).unwrap();
    assert_eq!(widgets_named(&eng, id, "consent").len(), 3);
    assert!(widgets_named(&eng, id, "agree").is_empty());
}

#[test]
fn a_form_too_large_to_check_for_a_certification_refuses_typing_and_says_so() {
    // The text field is object 3; the form lists it once and then past the walk's cap (20 000 nodes).
    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (A) /Rect [72 100 200 120] /F 4 /P 4 0 R >>";
    let fields: String = "3 0 R ".repeat(20_001);
    let (eng, id, _) = open_bytes(
        "too-large-for-locks",
        pdf_of(&[
            format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{fields}] >> >>"),
            "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
            widget.to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [3 0 R] >>".to_string(),
        ]),
    );
    let err = fill(&eng, id, "A", text("x")).unwrap_err().to_string();
    assert!(err.contains("too many form fields to check the signature locks"), "{err}");
    assert!(!err.contains("locked by a signature"), "{err}");
    assert_eq!(field(&eng, id, "A").value, "");
}
