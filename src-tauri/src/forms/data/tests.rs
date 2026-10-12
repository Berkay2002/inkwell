//! Form data export and import: every field kind through FDF, XFDF and JSON, what import reports and
//! skips, locked fields, certified and signed documents, hostile files.

use super::*;
use crate::engine::fixture::{self, Fixture};
use crate::forms::{FieldDto, FieldPatch, NewField, Opt, add_one, edit_one, list_all, put_name, set_value_one};
use std::path::PathBuf;
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("formdata-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn nf(kind: FieldKind, name: &str, rect: [f32; 4]) -> NewField {
    NewField { kind, rect, name: Some(name.into()), group: None, export: None, caption: None, options: None }
}

/// A form with one field of every kind (and a read-only one), on a two-page fixture.
fn form(test: &str) -> (Arc<Engine>, DocId, String) {
    let p = dir(test).join("a.pdf");
    std::fs::write(&p, fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() })).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    let opts = vec![Opt { export: "se".into(), label: "Sweden".into() }, Opt { export: "no".into(), label: "Norway".into() }];
    let all = vec![
        nf(FieldKind::Text, "Full name", [72.0, 100.0, 272.0, 120.0]),
        nf(FieldKind::Text, "Address.Street", [72.0, 130.0, 272.0, 150.0]),
        nf(FieldKind::Checkbox, "Agree", [72.0, 160.0, 86.0, 174.0]),
        nf(FieldKind::Radio, "Contact", [72.0, 190.0, 86.0, 204.0]),
        NewField { group: Some("Contact".into()), name: None, ..nf(FieldKind::Radio, "", [120.0, 190.0, 134.0, 204.0]) },
        NewField { options: Some(opts.clone()), ..nf(FieldKind::Combo, "Country", [72.0, 220.0, 220.0, 240.0]) },
        NewField { options: Some(opts), ..nf(FieldKind::List, "Pick", [72.0, 250.0, 220.0, 310.0]) },
        NewField { caption: Some("Reset".into()), ..nf(FieldKind::Button, "Reset", [72.0, 320.0, 150.0, 344.0]) },
        nf(FieldKind::Text, "Frozen", [72.0, 360.0, 272.0, 380.0]),
        nf(FieldKind::Signature, "Sig", [72.0, 400.0, 272.0, 440.0]),
    ];
    for w in &all {
        add_one(&eng, id, 0, w, EditSource::User).unwrap();
    }
    let f = |n: &str| list_all(&eng, id).unwrap().fields.into_iter().find(|w| w.name == n).unwrap();
    let (pick, frozen) = (f("Pick"), f("Frozen"));
    edit_one(&eng, id, 0, pick.xref, &FieldPatch { multi_select: Some(true), ..Default::default() }, EditSource::User).unwrap();
    edit_one(&eng, id, 0, frozen.xref, &FieldPatch { read_only: Some(true), ..Default::default() }, EditSource::User).unwrap();
    (eng, id, path)
}

fn dto(eng: &Engine, id: DocId, name: &str) -> Vec<FieldDto> {
    list_all(eng, id).unwrap().fields.into_iter().filter(|w| w.name == name).collect()
}

fn set(eng: &Engine, id: DocId, name: &str, v: FieldValue) {
    let w = &dto(eng, id, name)[0];
    set_value_one(eng, id, w.page, w.xref, &v, EditSource::User).unwrap();
}

fn text(s: &str) -> FieldValue {
    FieldValue::Text(s.into())
}

fn fill_everything(eng: &Engine, id: DocId) {
    set(eng, id, "Full name", text("Asa <Berg> & \"Co\" \u{e5}\u{e4}\u{f6} \u{4e2d}"));
    set(eng, id, "Address.Street", text("Main St 4"));
    set(eng, id, "Agree", FieldValue::Checked(true));
    let second = dto(eng, id, "Contact")[1].clone();
    set_value_one(eng, id, second.page, second.xref, &FieldValue::Checked(true), EditSource::User).unwrap();
    set(eng, id, "Country", text("no"));
    set(eng, id, "Pick", FieldValue::Many(vec!["se".into(), "no".into()]));
}

fn snapshot(eng: &Engine, id: DocId) -> Vec<(String, String, Vec<String>, bool)> {
    list_all(eng, id).unwrap().fields.iter().map(|w| (w.name.clone(), w.value.clone(), w.values.clone(), w.checked)).collect()
}

fn skipped<'a>(s: &'a Summary, name: &str) -> &'a str {
    &s.skipped.iter().find(|k| k.name == name).unwrap_or_else(|| panic!("{name} not reported: {:?}", s.skipped)).reason
}

#[test]
fn every_field_kind_round_trips_through_fdf_xfdf_and_json_as_one_undo_step() {
    for (fmt, ext) in [(Format::Json, "json"), (Format::Xfdf, "xfdf"), (Format::Fdf, "fdf")] {
        let (a, ida, _) = form(&format!("rt-{ext}-a"));
        fill_everything(&a, ida);
        let (bytes, done) = export(&a, ida, fmt, "a.pdf").unwrap();
        // The signature and the push button carry no value.
        assert_eq!(done.count, 7, "{ext}");
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.fields.len(), 7, "{ext}: {:?}", parsed.fields);
        assert!(parsed.fields.iter().all(|(n, _)| n != "Sig" && n != "Reset"));

        let (b, idb, _) = form(&format!("rt-{ext}-b"));
        let steps = b.history(idb).unwrap().len();
        let r = import_one(&b, idb, &parsed, EditSource::User).unwrap();
        // "Frozen" is read-only and empty: unchanged is a no-op, never an error. Empty fields are unchanged.
        assert!(r.summary.skipped.iter().all(|k| k.name == "Frozen") && r.summary.skipped_total <= 1, "{ext}: {:?}", r.summary);
        assert_eq!(r.summary.set, 6, "{ext}: {:?}", r.summary);
        assert!(!r.signed);
        assert_eq!(b.history(idb).unwrap().len(), steps + 1, "{ext}: one undo step");
        assert_eq!(snapshot(&b, idb), snapshot(&a, ida), "{ext}");

        // A second import changes nothing: no summary count, no new undo step.
        let again = import_one(&b, idb, &parsed, EditSource::User).unwrap();
        assert_eq!(again.summary.set, 0, "{ext}");
        assert_eq!(b.history(idb).unwrap().len(), steps + 1, "{ext}: nothing to undo twice");
        b.undo(idb).unwrap();
        assert_eq!(dto(&b, idb, "Full name")[0].value, "", "{ext}: undo empties the form again");
    }
}

#[test]
fn json_has_full_names_export_values_and_lists() {
    let (eng, id, _) = form("json");
    fill_everything(&eng, id);
    let (bytes, _) = export(&eng, id, Format::Json, "a.pdf").unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let want = dto(&eng, id, "Contact").iter().find(|w| w.checked).unwrap().export.clone();
    assert_eq!(v["Address.Street"], "Main St 4");
    assert_eq!(v["Contact"], want.as_str());
    assert_eq!(v["Country"], "no");
    assert_eq!(v["Pick"], serde_json::json!(["se", "no"]));
    assert_ne!(v["Agree"], "Off");
    assert_eq!(v["Frozen"], "");
    assert!(v.get("Sig").is_none() && v.get("Reset").is_none());
}

#[test]
fn import_reports_what_it_could_not_take_and_still_fills_the_rest() {
    let (eng, id, _) = form("report");
    let file = br#"{"Full name": "Kim", "Nope": "1", "Country": "de", "Agree": "Maybe", "Frozen": "z",
        "Pick": ["se", "zz"], "Sig": "x", "Address.Street": {"a": 1}, "Contact": ["a", "b"]}"#;
    let steps = eng.history(id).unwrap().len();
    let r = import_one(&eng, id, &parse(file).unwrap(), EditSource::User).unwrap();
    assert_eq!((r.summary.set, r.summary.unchanged, r.summary.skipped_total), (1, 0, 8), "{:?}", r.summary);
    assert!(skipped(&r.summary, "Nope").contains("no field"));
    assert!(skipped(&r.summary, "Frozen").contains("read-only"));
    assert!(skipped(&r.summary, "Country").contains("choices"));
    assert!(skipped(&r.summary, "Agree").contains("values"));
    assert_eq!(dto(&eng, id, "Full name")[0].value, "Kim");
    assert_eq!(dto(&eng, id, "Frozen")[0].value, "");
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);

    // Nothing takeable: no undo step at all.
    let steps = eng.history(id).unwrap().len();
    let r = import_one(&eng, id, &parse(br#"{"Nope": "1"}"#).unwrap(), EditSource::User).unwrap();
    assert_eq!((r.summary.set, r.summary.skipped_total), (0, 1));
    assert_eq!(eng.history(id).unwrap().len(), steps);
}

/// Marks the signature field `Sig` as signed, with an optional `/Lock` and an optional DocMDP `/P`.
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
fn a_field_locked_by_a_signature_is_skipped_and_reported() {
    let (eng, id, _) = form("locked");
    sign(&eng, id, Some(("Include", &["Full name"])), None);
    let r = import_one(&eng, id, &parse(br#"{"Full name": "x", "Address.Street": "y"}"#).unwrap(), EditSource::User).unwrap();
    assert!(r.signed);
    assert_eq!(r.summary.set, 1, "{:?}", r.summary);
    assert!(skipped(&r.summary, "Full name").contains("signature"));
    assert_eq!(dto(&eng, id, "Full name")[0].value, "");
    assert_eq!(dto(&eng, id, "Address.Street")[0].value, "y");
}

#[test]
fn a_p1_certification_refuses_the_import_with_no_history_step_but_exports() {
    let (eng, id, _) = form("certified");
    sign(&eng, id, None, Some(1));
    let before = eng.edit_state(id).unwrap();
    let steps = eng.history(id).unwrap().len();
    match import_one(&eng, id, &parse(br#"{"Full name": "x"}"#).unwrap(), EditSource::User) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{:?}", other.map(|r| r.summary)),
    }
    let after = eng.edit_state(id).unwrap();
    assert_eq!((after.rev, after.undo, after.dirty), (before.rev, before.undo, before.dirty));
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert!(export(&eng, id, Format::Json, "a.pdf").is_ok());
    assert!(!exchange::gates(&eng, id).unwrap().forms);
}

#[test]
fn p2_and_p3_certifications_fill_existing_fields_and_make_nothing_else() {
    for p in [2, 3] {
        let (eng, id, _) = form(&format!("certified-{p}"));
        sign(&eng, id, None, Some(p));
        assert!(exchange::gates(&eng, id).unwrap().forms, "P {p}");
        let fields = |eng: &Engine| list_all(eng, id).unwrap().fields.len();
        let (n, pages) = (fields(&eng), eng.edit_state(id).unwrap().page_count);
        let r = import_one(&eng, id, &parse(br#"{"Full name": "Kim", "Brand new": "x", "Sig": "y"}"#).unwrap(), EditSource::User).unwrap();
        assert_eq!(r.summary.set, 1, "P {p}: {:?}", r.summary);
        assert!(skipped(&r.summary, "Brand new").contains("no field"), "P {p}");
        assert_eq!(dto(&eng, id, "Full name")[0].value, "Kim", "P {p}");
        assert_eq!((fields(&eng), eng.edit_state(id).unwrap().page_count), (n, pages), "P {p}: no field or page was made");
        // Through FDF and XFDF too: a field the file names that the form lacks is reported, never created.
        let (bytes, _) = export(&eng, id, Format::Fdf, "a.pdf").unwrap();
        let extra = String::from_utf8_lossy(&bytes).replace("/Fields [", "/Fields [ << /T (Created) /V (x) >> ");
        let r = import_one(&eng, id, &parse(extra.as_bytes()).unwrap(), EditSource::User).unwrap();
        assert!(skipped(&r.summary, "Created").contains("no field"), "P {p}");
        assert_eq!(fields(&eng), n, "P {p}");
    }
}

#[test]
fn a_signed_document_keeps_its_signed_bytes_as_a_prefix_after_an_import() {
    let (eng, id, path) = form("signed");
    sign(&eng, id, None, None);
    eng.save(id, None).unwrap();
    let original = std::fs::read(&path).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let r = import_one(&eng2, id2, &parse(br#"{"Full name": "Kim"}"#).unwrap(), EditSource::User).unwrap();
    assert!(r.signed);
    assert_eq!(r.summary.set, 1);
    eng2.save(id2, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original));
}

#[test]
fn a_signed_document_keeps_its_signed_bytes_as_a_prefix_after_a_csv_import() {
    let (eng, id, path) = form("signed_csv");
    sign(&eng, id, None, None);
    eng.save(id, None).unwrap();
    let original = std::fs::read(&path).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let r = import_one(&eng2, id2, &parse(b"Full name\r\nKim\r\n").unwrap(), EditSource::User).unwrap();
    assert!(r.signed);
    assert_eq!(r.summary.set, 1);
    eng2.save(id2, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original));
}

#[test]
fn an_fdf_is_read_by_name_without_its_scripts_or_file_references() {
    let fdf = b"%FDF-1.2\n\
        1 0 obj\n<< /FDF << /F (evil.exe) /JavaScript << /Before (evil_before) >> /Fields [ \
        << /T (Full name) /V (Kim) /AA << /K << /S /JavaScript /JS (evil_aa) >> >> /A << /S /Launch /F (evil_launch) >> >> \
        << /T (Address) /Kids [ << /T (Street) /V (Main St) >> ] >> \
        << /T (Pick) /V [(se) (no)] >> << /T (Agree) /V /Yes >> ] >> >>\nendobj\n\
        trailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let p = parse(fdf).unwrap();
    assert_eq!(
        p.fields,
        vec![
            ("Full name".to_string(), Val::Text("Kim".into())),
            ("Address.Street".to_string(), Val::Text("Main St".into())),
            ("Pick".to_string(), Val::Many(vec!["se".into(), "no".into()])),
            ("Agree".to_string(), Val::Text("Yes".into())),
        ]
    );
    assert!(!format!("{p:?}").contains("evil"));

    // A kid that lists itself, and one that nests without end, end.
    let cyc = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [2 0 R] >> >>\nendobj\n2 0 obj\n<< /T (a) /V (x) /Kids [2 0 R] >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    assert_eq!(parse(cyc).unwrap().fields.len(), 1);
}

#[test]
fn hostile_data_files_are_refused_or_read_harmlessly() {
    let deep_json = format!("{}1{}", "[".repeat(200), "]".repeat(200));
    assert!(parse(format!("{{\"a\": {deep_json}}}").as_bytes()).is_err(), "nested JSON");
    assert!(parse(b"[1, 2]").is_err(), "a list is not form data");
    assert!(parse(b"{\"a\": ").is_err(), "truncated JSON");
    let laughs = "<?xml version=\"1.0\"?><!DOCTYPE l [<!ENTITY a \"aaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]><xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><fields><field name=\"x\"><value>&b;</value></field></fields></xfdf>";
    assert!(matches!(parse(laughs.as_bytes()), Err(EngineError::Invalid(_))));
    let many = format!("{{{}}}", vec!["\"a\":1"; 100_050].join(","));
    assert!(parse(many.as_bytes()).is_err(), "more than 100 000 values");
    let wide = format!("{{{}}}", (0..10_050).map(|i| format!("\"f{i}\": \"v\"")).collect::<Vec<_>>().join(","));
    let p = parse(wide.as_bytes()).unwrap();
    assert_eq!((p.fields.len(), p.over), (10_000, 50));
    let long = format!("{{\"a\": \"{}\"}}", "x".repeat((1 << 20) + 1));
    assert!(matches!(parse(long.as_bytes()).unwrap().fields[0].1, Val::Invalid(_)));

    let (eng, id, _) = form("hostile");
    fill_everything(&eng, id);
    for fmt in [Format::Xfdf, Format::Fdf, Format::Json] {
        let (bytes, _) = export(&eng, id, fmt, "a.pdf").unwrap();
        for cut in (0..bytes.len()).step_by(29) {
            let _ = parse(&bytes[..cut]);
        }
        for at in (0..bytes.len()).step_by(41) {
            let mut b = bytes.clone();
            b[at] ^= 0xA5;
            let _ = parse(&b);
        }
    }
}

#[test]
fn a_json_name_over_one_megabyte_is_counted_as_skipped() {
    let (eng, id, _) = form("json-long-name");
    let file = format!(r#"{{"Full name": "Kim", "{}": "x"}}"#, "n".repeat(MAX_STR + 1));
    let r = import_one(&eng, id, &parse(file.as_bytes()).unwrap(), EditSource::User).unwrap();
    assert_eq!((r.summary.set, r.summary.skipped_total), (1, 1), "{:?}", r.summary);
    let k = &r.summary.skipped[0];
    assert!(k.name.starts_with("nnn") && k.name.len() <= 60 && k.reason.contains("1 MB"), "{k:?}");
}

#[test]
fn one_value_shared_by_many_fields_is_refused_by_the_byte_budget() {
    let fdf = |n: usize| {
        let fields: String = (0..n).map(|i| format!("<< /T (f{i}) /V 2 0 R >> ")).collect();
        format!("%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [{fields}] >> >>\nendobj\n2 0 obj\n({})\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n", "x".repeat(900_000))
            .into_bytes()
    };
    assert!(matches!(parse(&fdf(200)), Err(EngineError::Invalid(m)) if m.contains("same data")));
    let ok = parse(&fdf(5)).unwrap();
    assert_eq!(ok.fields.len(), 5);
    assert!(ok.fields.iter().all(|(_, v)| matches!(v, Val::Text(t) if t.len() == 900_000)));
}

#[test]
fn an_fdf_with_more_fields_than_the_walk_allows_is_refused_not_cut_short() {
    let refs = "<< /T (a) /V (x) >> ".repeat(MAX_NODES + 1);
    let f = format!("%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [{refs}] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    assert!(matches!(parse(f.as_bytes()), Err(EngineError::Invalid(_))));
}

#[test]
fn one_kids_array_shared_by_many_parents_is_refused_before_the_stack_grows_past_the_limit() {
    // Direct kids with no name that each point back at the one indirect array: nothing dedups them
    // and no name is charged, so only the count of kids pushed can stop the walk.
    let kids = "<< /Kids 2 0 R >> ".repeat(60_000);
    let f = format!("%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [<< /Kids 2 0 R >>] >> >>\nendobj\n2 0 obj\n[{kids}]\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    PEAK_STACK.with(|p| p.set(0));
    assert!(matches!(parse(f.as_bytes()), Err(EngineError::Invalid(m)) if m.contains("100 000 fields")));
    let peak = PEAK_STACK.with(|p| p.get());
    assert!(peak > 0 && peak <= MAX_NODES, "the stack held {peak} entries");
}

#[test]
fn xfdf_field_names_built_from_long_parents_are_charged_and_capped() {
    let xf = |body: String| format!("<xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><fields>{body}</fields></xfdf>").into_bytes();
    // One 900 kB parent over many leaves: every leaf's full name is 900 kB, 90 MB in all.
    let long = "a".repeat(900_000);
    let leaves: String = (0..100).map(|i| format!("<field name=\"l{i}\"><value>x</value></field>")).collect();
    let f = xf(format!("<field name=\"{long}\">{leaves}</field>"));
    assert!(matches!(parse_xfdf(&f), Err(EngineError::Invalid(m)) if m.contains("same data")));
    // A few are fine.
    let ok = parse_xfdf(&xf(format!("<field name=\"{long}\"><field name=\"l\"><value>x</value></field></field>"))).unwrap();
    assert_eq!(ok.fields.len(), 1);
    // A name past 1 MB is left out, and named in the summary by its start.
    let half = "b".repeat(600_000);
    let f = xf(format!("<field name=\"{half}\"><field name=\"{half}\"><value>x</value></field></field>"));
    let p = parse_xfdf(&f).unwrap();
    assert!(p.fields.is_empty());
    assert_eq!((p.over, p.left_out.len()), (1, 1));
    assert_eq!(p.left_out[0].0.len(), 60);
}

#[test]
fn a_password_field_is_never_written_to_a_file_or_filled_from_one() {
    let (eng, id, _) = form("password");
    add_one(&eng, id, 1, &nf(FieldKind::Text, "Secret", [72.0, 100.0, 272.0, 120.0]), EditSource::User).unwrap();
    let w = dto(&eng, id, "Secret").remove(0);
    edit_one(&eng, id, w.page, w.xref, &FieldPatch { password: Some(true), ..Default::default() }, EditSource::User).unwrap();
    set(&eng, id, "Full name", text("Asa"));
    set(&eng, id, "Secret", text("hunter2"));
    for fmt in [Format::Json, Format::Xfdf, Format::Fdf] {
        let (bytes, done) = export(&eng, id, fmt, "a.pdf").unwrap();
        assert_eq!(done.left_out, 1, "{fmt:?}");
        assert!(!String::from_utf8_lossy(&bytes).contains("hunter2"), "{fmt:?}: the secret is in the file");
        assert!(!parse(&bytes).unwrap().fields.iter().any(|(n, _)| n == "Secret"), "{fmt:?}");
    }
    let r = import_one(&eng, id, &parse(br#"{"Secret": "from the file", "Full name": "Bo"}"#).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.set, 1, "{:?}", r.summary);
    assert_eq!(skipped(&r.summary, "Secret"), "password field");
    assert_eq!(dto(&eng, id, "Secret")[0].value, "hunter2");
}

#[test]
fn a_value_over_1_mb_is_left_out_and_counted_on_export_and_named_on_import() {
    let (eng, id, _) = form("long-value");
    set(&eng, id, "Full name", text(&"x".repeat(MAX_STR + 1)));
    set(&eng, id, "Address.Street", text("Main St"));
    for fmt in [Format::Json, Format::Xfdf, Format::Fdf] {
        let (bytes, done) = export(&eng, id, fmt, "a.pdf").unwrap();
        assert_eq!(done.long_text, 1, "{fmt:?}");
        // What was written is a file Inkwell reads back, with the short value in it.
        let p = parse(&bytes).unwrap();
        assert!(p.fields.iter().any(|(n, v)| n == "Address.Street" && *v == Val::Text("Main St".into())), "{fmt:?}");
    }
    // An FDF that carries one says why it was not taken, not that the field takes text only.
    let long = "y".repeat(MAX_STR + 1);
    let fdf = format!("%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [ << /T (Full name) /V ({long}) >> ] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    let r = import_one(&eng, id, &parse(fdf.as_bytes()).unwrap(), EditSource::User).unwrap();
    assert_eq!(skipped(&r.summary, "Full name"), "the value is longer than 1 MB");
}

#[test]
fn one_indirect_value_over_1_mb_shared_by_many_fields_is_dropped_by_each_and_not_copied() {
    let long = "z".repeat(2 << 20);
    let n = 200;
    let fields: String = (0..n).map(|i| format!("<< /T (F{i}) /V 2 0 R >> ")).collect();
    let fdf = format!("%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [ {fields}] >> >>\nendobj\n2 0 obj\n({long})\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    // 400 MB if each field took its own copy: the file is read, not refused, and no field holds the text.
    let p = parse(fdf.as_bytes()).unwrap();
    assert_eq!(p.fields.len(), n);
    assert!(p.fields.iter().all(|(_, v)| *v == Val::Invalid("the value is longer than 1 MB")));
}

#[test]
fn a_json_list_with_one_entry_over_1_mb_says_so_and_not_that_a_list_takes_text_only() {
    let long = "y".repeat(MAX_STR + 1);
    let file = format!(r#"{{"Long": ["a", "{long}"], "Mixed": ["a", {{"b": 1}}], "Fine": ["a", "b"]}}"#);
    let p = parse(file.as_bytes()).unwrap();
    let of = |k: &str| p.fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap();
    assert_eq!(of("Long"), Val::Invalid("the value is longer than 1 MB"));
    assert_eq!(of("Mixed"), Val::Invalid("a list takes text values only"));
    assert_eq!(of("Fine"), Val::Many(vec!["a".into(), "b".into()]));
}

#[test]
fn a_value_is_measured_by_length_before_it_is_read() {
    let (eng, id, _) = form("value-length");
    let got = eng
        .read_custom(id, |d| {
            let s = |n: usize| PdfObject::new_string(&"v".repeat(n));
            let mut list = d.new_array()?;
            list.array_push(s(3)?)?;
            let mut c = Decoded::new();
            let short_list = string_over_limit(&list, &mut c);
            list.array_push(s(MAX_STR + 1)?)?;
            Ok([
                string_over_limit(&s(2 * MAX_STR + 3)?, &mut c),
                string_over_limit(&s(MAX_STR + 1)?, &mut c),
                string_over_limit(&PdfObject::new_name("On")?, &mut c),
                short_list,
                string_over_limit(&list, &mut c),
            ])
        })
        .unwrap();
    // over twice the limit, in between (measured once decoded, here 1 MB + 1 of plain text), a name, a list of
    // short values, the same list with one long entry
    assert_eq!(got, [true, true, false, false, true]);
}

#[test]
fn a_utf16_value_is_measured_decoded_not_by_its_bytes() {
    // One kanji makes the whole string UTF-16BE: 2 bytes a character, 1.5 MB raw, 750 002 bytes of text.
    let (eng, id, _) = form("utf16-under");
    let under = format!("あ{}", "x".repeat(749_999));
    set(&eng, id, "Full name", text(&under));
    set(&eng, id, "Address.Street", text("Main St"));
    for fmt in [Format::Json, Format::Xfdf, Format::Fdf] {
        let (bytes, done) = export(&eng, id, fmt, "a.pdf").unwrap();
        assert_eq!(done.long_text, 0, "{fmt:?}");
        let p = parse(&bytes).unwrap();
        assert!(p.fields.iter().any(|(n, v)| n == "Full name" && *v == Val::Text(under.clone())), "{fmt:?}");
    }
    // 600 000 kanji: 1.2 MB raw, 1.8 MB of text: left out and counted
    let (eng, id, _) = form("utf16-over");
    set(&eng, id, "Full name", text(&"あ".repeat(600_000)));
    set(&eng, id, "Address.Street", text("Main St"));
    for fmt in [Format::Json, Format::Xfdf, Format::Fdf] {
        let (bytes, done) = export(&eng, id, fmt, "a.pdf").unwrap();
        assert_eq!(done.long_text, 1, "{fmt:?}");
        let p = parse(&bytes).unwrap();
        assert!(p.fields.iter().all(|(n, _)| n != "Full name"), "{fmt:?}");
        assert!(p.fields.iter().any(|(n, v)| n == "Address.Street" && *v == Val::Text("Main St".into())), "{fmt:?}");
    }
}

#[test]
fn a_list_of_one_shared_utf16_string_is_not_decoded_once_per_entry() {
    use crate::outline::tests::{open_bytes, raw_pdf};
    // 1.99 MB of UTF-16BE (under the 2 MB that is surely over, 996 001 bytes of text), listed 4096 times
    let units = 996_000;
    let hex = format!("<FEFF3042{}>", "0078".repeat(units));
    let refs = "7 0 R ".repeat(4096);
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R] >>".into(),
        "<< >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (List) /Rect [10 10 100 50] /P 3 0 R /V [{refs}] >>"),
        "<< >>".into(),
        hex,
    ];
    let d = dir("shared-utf16");
    let (eng, id, _) = open_bytes(&d, "s.pdf", &raw_pdf(&objs));
    DECODES.with(|d| d.set(0));
    let (_, done) = export(&eng, id, Format::Json, "s.pdf").unwrap();
    assert_eq!(done.long_text, 1);
    // Decoded once: the 8 MiB budget charges every entry, so the list is over, but the string is read one time.
    assert_eq!(DECODES.with(|d| d.get()), 1);
}

/// `n` text fields (F0, F1 ...) whose /V all reference one string of `hex`, opened as a document.
fn shared_string_form(test: &str, n: usize, hex: &str) -> (Arc<Engine>, DocId) {
    use crate::outline::tests::{open_bytes, raw_pdf};
    let refs: String = (0..n).map(|i| format!("{} 0 R ", 5 + i)).collect();
    let mut objs: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{refs}] >> >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{refs}] >>"),
        "<< >>".into(),
    ];
    objs.extend((0..n).map(|i| {
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (F{i}) /Rect [10 10 100 50] /P 3 0 R /V {} 0 R >>", 5 + n)
    }));
    objs.push(format!("<{hex}>"));
    let (eng, id, _) = open_bytes(&dir(test), "s.pdf", &raw_pdf(&objs));
    (eng, id)
}

#[test]
fn many_fields_sharing_one_gray_zone_string_decode_it_once() {
    // 10 fields on 1.9 MB of UTF-16BE kanji (2.85 MB of text): every one is over, and the string is read once.
    // The counter covers forms/data.rs decodes only, not MuPDF's own per-widget load work (scan).
    let (eng, id) = shared_string_form("shared-over", 10, &format!("FEFF{}", "3042".repeat(950_000)));
    DECODES.with(|d| d.set(0));
    let (_, done) = export(&eng, id, Format::Json, "s.pdf").unwrap();
    assert_eq!((done.count, done.long_text), (0, 10));
    assert_eq!(DECODES.with(|d| d.get()), 1);

    // Under the limit (996 000 characters), the value each field exports comes from that one decode too.
    let (eng, id) = shared_string_form("shared-under", 3, &format!("FEFF{}", "0078".repeat(996_000)));
    DECODES.with(|d| d.set(0));
    let (bytes, done) = export(&eng, id, Format::Json, "s.pdf").unwrap();
    assert_eq!((done.count, done.long_text), (3, 0));
    let p = parse(&bytes).unwrap();
    assert!(p.fields.iter().all(|(_, v)| *v == Val::Text("x".repeat(996_000))));
    assert_eq!(DECODES.with(|d| d.get()), 1);
}

// ---- CSV, tab text and XML ------------------------------------------------------------------------

/// `form()` with a multi-line "Notes" field and a single-select list box "Single".
fn table_form(test: &str) -> (Arc<Engine>, DocId) {
    let (eng, id, _) = form(test);
    let opts = vec![Opt { export: "se".into(), label: "Sweden".into() }, Opt { export: "no".into(), label: "Norway".into() }];
    add_one(&eng, id, 0, &nf(FieldKind::Text, "Notes", [72.0, 460.0, 272.0, 500.0]), EditSource::User).unwrap();
    add_one(&eng, id, 0, &NewField { options: Some(opts), ..nf(FieldKind::List, "Single", [72.0, 510.0, 220.0, 560.0]) }, EditSource::User).unwrap();
    let w = &dto(&eng, id, "Notes")[0];
    edit_one(&eng, id, 0, w.xref, &FieldPatch { multiline: Some(true), ..Default::default() }, EditSource::User).unwrap();
    (eng, id)
}

const NOTES: &str = "line one\nline, two \"quoted\" \u{e5}\u{e4}\u{f6} \u{4e2d}";

fn fill_table_form(eng: &Engine, id: DocId) {
    fill_everything(eng, id);
    set(eng, id, "Address.Street", text("Main St 4, apt 2"));
    set(eng, id, "Notes", text(NOTES));
}

fn try_set(eng: &Engine, id: DocId, name: &str, v: FieldValue) -> std::result::Result<EditState, EngineError> {
    let w = &dto(eng, id, name)[0];
    set_value_one(eng, id, w.page, w.xref, &v, EditSource::User)
}

fn put_script(eng: &Engine, id: DocId, name: &str, key: &str, js: &str) {
    eng.apply_custom(id, "Add script".into(), EditSource::User, false, |d| {
        let entries = scan(d)?;
        let e = entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("no field {name}"));
        let mut f = e.field.clone();
        crate::forms::author::put_js(d, &mut f, key, Some(js.to_string()))?;
        Ok(vec![e.page])
    })
    .unwrap();
}

#[test]
fn csv_tab_text_and_xml_round_trip_with_a_comma_a_quote_a_newline_and_non_ascii() {
    for (fmt, ext) in [(Format::Csv, "csv"), (Format::Tab, "tab"), (Format::Xml, "xml")] {
        let (a, ida) = table_form(&format!("tbl-{ext}-a"));
        fill_table_form(&a, ida);
        let (bytes, done) = export(&a, ida, fmt, "a.pdf").unwrap();
        // The signature and the push button carry no value.
        assert_eq!(done.count, 9, "{ext}");
        let text_of = String::from_utf8(bytes.clone()).unwrap();
        match fmt {
            Format::Csv => {
                assert!(text_of.starts_with('\u{FEFF}'), "a spreadsheet needs the BOM to read UTF-8");
                assert!(text_of.contains("\"Asa <Berg> & \"\"Co\"\""), "RFC 4180 quoting: {text_of}");
                assert!(text_of.contains("\"Main St 4, apt 2\""), "{text_of}");
                assert!(text_of.contains("\"line one\nline, two \"\"quoted\"\""), "{text_of}");
            }
            Format::Tab => {
                // Quoted as Excel does, so the tab and the line break in a value stay what they are.
                assert!(text_of.contains("\"line one\nline, two \"\"quoted\"\" "), "{text_of:?}");
                assert!(text_of.contains("\"Asa <Berg> & \"\"Co\"\" "), "{text_of:?}");
            }
            _ => {
                assert!(text_of.contains("xfdf:original=\"Full name\""), "{text_of}");
                assert!(text_of.contains("<Address><Street>Main St 4, apt 2</Street></Address>"), "{text_of}");
            }
        }
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.fields.len(), 9, "{ext}: {:?}", parsed.fields);

        let (b, idb) = table_form(&format!("tbl-{ext}-b"));
        let steps = b.history(idb).unwrap().len();
        let r = import_one(&b, idb, &parsed, EditSource::User).unwrap();
        assert!(r.summary.skipped.iter().all(|k| k.name == "Frozen") && r.summary.skipped_total <= 1, "{ext}: {:?}", r.summary);
        assert_eq!(r.summary.set, 7, "{ext}: {:?}", r.summary);
        assert_eq!(b.history(idb).unwrap().len(), steps + 1, "{ext}: one undo step");
        assert_eq!(snapshot(&b, idb), snapshot(&a, ida), "{ext}");
        // The same data again changes nothing.
        assert_eq!(import_one(&b, idb, &parsed, EditSource::User).unwrap().summary.set, 0, "{ext}");
    }
}

#[test]
fn excels_unicode_text_is_read_as_utf16_with_a_bom() {
    let (eng, id) = table_form("tbl-utf16");
    for little in [true, false] {
        let text = "Full name\tAddress.Street\tAgree\tCountry\tPick\tNope\r\nBj\u{f6}rn \u{c5}s\tStorgatan 1\tYes\tse\tse, no\tx\r\n";
        let mut bytes: Vec<u8> = if little { vec![0xFF, 0xFE] } else { vec![0xFE, 0xFF] };
        for u in text.encode_utf16() {
            bytes.extend(if little { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        let parsed = parse(&bytes).unwrap();
        let r = import_one(&eng, id, &parsed, EditSource::User).unwrap();
        assert_eq!(dto(&eng, id, "Full name")[0].value, "Bj\u{f6}rn \u{c5}s", "little {little}");
        assert_eq!(dto(&eng, id, "Address.Street")[0].value, "Storgatan 1");
        assert!(dto(&eng, id, "Agree")[0].checked);
        assert_eq!(dto(&eng, id, "Pick")[0].values, vec!["se", "no"], "a multi-select list's picks are one cell");
        // The unknown column is reported, and nothing else is lost.
        assert!(skipped(&r.summary, "Nope").contains("no field"), "{:?}", r.summary);
        assert_eq!(r.summary.skipped_total, 1);
        eng.undo(id).unwrap();
    }
}

#[test]
fn text_that_is_not_utf8_or_utf16_with_a_bom_is_refused() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("Latin-1", b"Full name\r\nBj\xF6rn\r\n".to_vec()),
        ("UTF-16 without a BOM", "Full name\r\nKim\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect()),
        ("UTF-32", vec![0xFF, 0xFE, 0, 0, b'a', 0, 0, 0]),
        ("an odd UTF-16 length", vec![0xFF, 0xFE, b'a', 0, b'b']),
        ("an unpaired surrogate", vec![0xFF, 0xFE, 0x00, 0xD8, b'a', 0]),
    ];
    for (what, bytes) in cases {
        match parse(&bytes) {
            Err(EngineError::Invalid(m)) => assert!(m.contains("UTF-8") && m.contains("UTF-16"), "{what}: {m}"),
            other => panic!("{what}: {:?}", other.map(|p| p.fields)),
        }
    }
    // UTF-8 with a BOM is fine.
    assert_eq!(parse(b"\xEF\xBB\xBFFull name\r\nKim\r\n").unwrap().fields, vec![("Full name".to_string(), Val::Cell("Kim".into()))]);
}

#[test]
fn a_file_with_several_records_lists_them_and_imports_the_picked_one() {
    let (eng, id) = table_form("tbl-rows");
    let csv = "Full name,Address.Street,Country\r\nAsa Berg,Main St 1,se\r\n,Side St 2,no\r\n\r\n,,\r\nOle Dahl,Hill 3,no\r\n";
    let r = records(csv.as_bytes(), None).unwrap();
    assert_eq!(r.count, 3, "a blank line and a row of empty cells are not records");
    // The first column, or the first one with text.
    assert_eq!(r.labels, vec!["Asa Berg", "Side St 2", "Ole Dahl"]);
    // As a spreadsheet numbers them: the header is row 1, the blank line and the row of empty cells count.
    assert_eq!(r.rows, vec![2, 3, 6]);
    // A line break inside quotes is not a row of its own.
    assert_eq!(records(b"A,B\n\"x\ny\",1\nz,2\n", None).unwrap().rows, vec![2, 3]);
    // Bidirectional overrides cannot reorder what the picker shows.
    assert_eq!(records(b"A\n\xE2\x80\xAEgnp.exe\nb\n", None).unwrap().labels[0], " gnp.exe");
    match parse(csv.as_bytes()) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("3 records"), "{m}"),
        other => panic!("{:?}", other.map(|p| p.fields)),
    }
    assert!(parse_pick(csv.as_bytes(), Some(3), None).is_err());
    let p = parse_pick(csv.as_bytes(), Some(2), None).unwrap();
    import_one(&eng, id, &p, EditSource::User).unwrap();
    assert_eq!(dto(&eng, id, "Full name")[0].value, "Ole Dahl");
    assert_eq!(dto(&eng, id, "Country")[0].value, "no");
    // A one-record file needs no pick, and every other format is one record.
    assert_eq!(records(b"A,B\r\n1,2\r\n", None).unwrap(), Records { count: 1, labels: vec![], rows: vec![] });
    assert_eq!(parse(b"A,B\r\n1,2\r\n").unwrap().fields.len(), 2);
    assert_eq!(records(br#"{"A": "1"}"#, None).unwrap().count, 1);
}

#[test]
fn csv_reading_edge_cases() {
    let cells = |s: &str| -> Vec<(String, Val)> { parse(s.as_bytes()).unwrap().fields };
    // Semicolons (a spreadsheet in a Swedish locale), quotes, a line break and a doubled quote inside one.
    assert_eq!(cells("A;B\n\"x;y\";\"say \"\"hi\"\"\nthere\"\n"), vec![("A".into(), Val::Cell("x;y".into())), ("B".into(), Val::Cell("say \"hi\"\nthere".into()))]);
    // Tab text is quoted the same way: the tab inside the quotes is part of the cell.
    assert_eq!(cells("A\tB\n\"x\ty\"\t5\n"), vec![("A".into(), Val::Cell("x\ty".into())), ("B".into(), Val::Cell("5".into()))]);
    // A short row leaves the field alone; an empty cell is a value.
    let p = parse(b"A,B,C\n1,,\n").unwrap();
    assert_eq!(p.fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["A", "B", "C"]);
    assert_eq!(parse(b"A,B,C\n1\n").unwrap().fields.len(), 1);
    // A row longer than the header, a column named twice and one with no name are reported, not fatal.
    let p = parse(b"A,A,,B\n1,2,3,4,5\n").unwrap();
    assert_eq!(p.fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    assert_eq!(p.over, 3, "{:?}", p.left_out);
    assert!(p.left_out.iter().any(|(_, why)| why.contains("twice")) && p.left_out.iter().any(|(_, why)| why.contains("no name")));
    // A header and no values, an empty file, a quote that never closes.
    for (what, file) in [("header only", "A,B\r\n"), ("empty", ""), ("blank", "\r\n\r\n"), ("open quote", "A,B\r\n1,\"2\r\n")] {
        assert!(matches!(parse(file.as_bytes()), Err(EngineError::Invalid(_))), "{what}");
    }
    // Over the caps: too many rows, too many columns, one cell over 1 MB.
    let rows = format!("A\n{}", "x\n".repeat(table::MAX_ROWS + 1));
    assert!(matches!(parse(rows.as_bytes()), Err(EngineError::Invalid(m)) if m.contains("rows")));
    let cols = format!("{}\n{}\n", vec!["c"; MAX_ITEMS + 1].join(","), vec!["v"; MAX_ITEMS + 1].join(","));
    assert!(matches!(parse(cols.as_bytes()), Err(EngineError::Invalid(m)) if m.contains("columns")));
    let long = format!("A,B\n{},x\n", "y".repeat(MAX_STR + 5));
    let p = parse(long.as_bytes()).unwrap();
    assert_eq!(p.fields[0].1, Val::Invalid("the value is longer than 1 MB"));
    assert_eq!(p.fields[1].1, Val::Cell("x".into()));
    // A quote that never closes in a 30 MB file is refused without copying it.
    let open = format!("A\n\"{}", "z".repeat(30 << 20));
    assert!(parse(open.as_bytes()).is_err());
}

#[test]
fn plain_xml_is_read_by_the_case_of_its_tags_and_nested_names() {
    let (eng, id) = table_form("tbl-xml");
    let xml = "<?xml version=\"1.0\"?>\n<fields xmlns:xfdf=\"http://ns.adobe.com/xfdf-transition/\">\n\
        <Full_name xfdf:original=\"Full name\">Kim</Full_name>\n<Address><Street>Main St</Street></Address>\n<Agree>Yes</Agree><Unknown>1</Unknown></fields>";
    let r = import_one(&eng, id, &parse(xml.as_bytes()).unwrap(), EditSource::User).unwrap();
    assert_eq!(dto(&eng, id, "Full name")[0].value, "Kim");
    assert_eq!(dto(&eng, id, "Address.Street")[0].value, "Main St");
    assert!(dto(&eng, id, "Agree")[0].checked);
    assert!(skipped(&r.summary, "Unknown").contains("no field"));
    // A file with a DOCTYPE is still refused.
    assert!(parse(b"<?xml version=\"1.0\"?><!DOCTYPE f [<!ENTITY a \"b\">]><fields><x>&a;</x></fields>").is_err());
}

#[test]
fn a_certified_document_takes_csv_only_where_filling_is_allowed() {
    let csv = b"Full name\r\nKim\r\n";
    let (eng, id) = table_form("tbl-p1");
    sign(&eng, id, None, Some(1));
    let steps = eng.history(id).unwrap().len();
    match import_one(&eng, id, &parse(csv).unwrap(), EditSource::User) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{:?}", other.map(|r| r.summary)),
    }
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert!(export(&eng, id, Format::Csv, "a.pdf").is_ok(), "export is always allowed");
    let (eng, id) = table_form("tbl-p2");
    sign(&eng, id, None, Some(2));
    let r = import_one(&eng, id, &parse(csv).unwrap(), EditSource::User).unwrap();
    assert_eq!((r.summary.set, dto(&eng, id, "Full name")[0].value.as_str()), (1, "Kim"));
}

#[test]
fn a_check_box_validate_script_rejects_a_change_and_an_import_reports_it() {
    let (eng, id) = table_form("tbl-checkbox");
    put_script(&eng, id, "Agree", "V", "if (event.value !== \"Off\") { app.alert(\"Not yet.\"); event.rc = false; }");
    let steps = eng.history(id).unwrap().len();
    let e = try_set(&eng, id, "Agree", FieldValue::Checked(true)).unwrap_err().to_string();
    assert!(e.contains("Not yet."), "{e}");
    assert!(!dto(&eng, id, "Agree")[0].checked);
    assert_eq!(eng.history(id).unwrap().len(), steps, "a rejected change leaves no step");
    // An import goes the same way: the field is skipped with the script's own message, the rest is filled.
    let r = import_one(&eng, id, &parse(b"Full name,Agree\r\nKim,Yes\r\n").unwrap(), EditSource::User).unwrap();
    assert_eq!((r.summary.set, skipped(&r.summary, "Agree")), (1, "Not yet."));
    assert!(!dto(&eng, id, "Agree")[0].checked);
    // Turning it off is allowed (event.value is Off), and a script that is fine with it lets it through.
    put_script(&eng, id, "Agree", "V", "if (event.value === \"Yes\") { event.rc = true; }");
    try_set(&eng, id, "Agree", FieldValue::Checked(true)).unwrap();
    assert!(dto(&eng, id, "Agree")[0].checked);
}

#[test]
fn a_list_box_keystroke_script_runs_on_a_pick_and_on_import() {
    let (eng, id) = table_form("tbl-listbox");
    put_script(&eng, id, "Single", "K", "if (event.value === \"no\") { app.alert(\"Not Norway\"); event.rc = false; } else { this.getField(\"Full name\").value = \"picked \" + event.value; }");
    try_set(&eng, id, "Single", text("se")).unwrap();
    assert_eq!(dto(&eng, id, "Single")[0].value, "se");
    assert_eq!(dto(&eng, id, "Full name")[0].value, "picked se", "the script saw the pick as event.value");
    let steps = eng.history(id).unwrap().len();
    let e = try_set(&eng, id, "Single", text("no")).unwrap_err().to_string();
    assert!(e.contains("Not Norway"), "{e}");
    assert_eq!(dto(&eng, id, "Single")[0].value, "se", "restored");
    assert_eq!(eng.history(id).unwrap().len(), steps);
    let r = import_one(&eng, id, &parse(b"Single,Notes\r\nno,hello\r\n").unwrap(), EditSource::User).unwrap();
    assert_eq!((r.summary.set, skipped(&r.summary, "Single")), (1, "Not Norway"));
    // A list box that takes several shows its first pick to a validate script, and may be rejected by it.
    put_script(&eng, id, "Pick", "V", "if (event.value === \"no\") { event.rc = false; }");
    assert!(try_set(&eng, id, "Pick", FieldValue::Many(vec!["no".into(), "se".into()])).is_err());
    try_set(&eng, id, "Pick", FieldValue::Many(vec!["se".into(), "no".into()])).unwrap();
    assert_eq!(dto(&eng, id, "Pick")[0].values, vec!["se", "no"]);
}

#[test]
fn an_af_call_on_a_check_box_or_list_box_does_not_reject_its_value() {
    // The AF number and mask calls are for text: a stray one on a check box must not make "Off" or "Yes" invalid.
    let (eng, id) = table_form("tbl-af");
    put_script(&eng, id, "Agree", "K", "AFNumber_Keystroke(2, 0, 0, 0, \"\", true);");
    put_script(&eng, id, "Single", "V", "AFRange_Validate(true, 0, true, 10);");
    try_set(&eng, id, "Agree", FieldValue::Checked(true)).unwrap();
    try_set(&eng, id, "Single", text("se")).unwrap();
}

// ---- delimiter, quoting in tab text, XML names, multi-select scripts ----------------------------

#[test]
fn the_delimiter_comes_from_the_file_name_then_the_header() {
    let one = |p: Parsed| p.fields;
    // A one-field tab file has no tab in its header: its own export reads back, comma and all.
    let one_field = table::tab(&["Full name"], &["Berg, Asa".to_string()]);
    let want = vec![("Full name".to_string(), Val::Cell("Berg, Asa".into()))];
    assert_eq!(one(parse_pick(one_field.as_bytes(), None, Some(Format::Tab)).unwrap()), want);
    // With no name to go by, a header with no delimiter is one column: no split.
    assert_eq!(one(parse(one_field.as_bytes()).unwrap()), want);
    assert_eq!(one(parse(b"Full name\r\nBerg, Asa\r\n").unwrap()), want);
    assert_eq!(one(parse_pick(b"Full name\r\nBerg, Asa\r\n", None, Some(Format::Csv)).unwrap()), want);
    // A tab in the header wins over commas inside the names.
    let tabbed = b"a, b, c\tAge\r\nx\t3\r\n";
    for hint in [None, Some(Format::Tab)] {
        let f = one(parse_pick(tabbed, None, hint).unwrap());
        assert_eq!(f, vec![("a, b, c".to_string(), Val::Cell("x".into())), ("Age".to_string(), Val::Cell("3".into()))], "{hint:?}");
    }
    // Spreadsheets in a Swedish locale write semicolons; a tie goes to comma.
    let f = one(parse_pick(b"A;B\r\n1;2\r\n", None, Some(Format::Csv)).unwrap());
    assert_eq!(f.len(), 2);
    let f = one(parse(b"a,b;c\r\n1,2;3\r\n").unwrap());
    assert_eq!(f[1].0, "b;c");
    // Blank lines above the header do not hide it.
    assert_eq!(one(parse(b"\r\n\r\nA\tB\r\n1\t2\r\n").unwrap()).len(), 2);
    // A one-field file round trips through each of the writers.
    for (hint, text) in [(Some(Format::Csv), table::csv(&["N"], &["a, \"b\"".to_string()])), (Some(Format::Tab), table::tab(&["N"], &["a\t\"b\"".to_string()]))] {
        assert_eq!(one(parse_pick(text.as_bytes(), None, hint).unwrap())[0].1, Val::Cell(if hint == Some(Format::Csv) { "a, \"b\"" } else { "a\t\"b\"" }.into()));
    }
}

#[test]
fn excels_tab_text_quotes_a_cell_with_a_quote_a_tab_or_a_line_break() {
    let (eng, id) = table_form("tbl-excel-quotes");
    let text = "Full name\tNotes\tAddress.Street\r\n\"He said \"\"hi\"\"\"\t\"line one\nline\ttwo, \"\"x\"\"\"\tMain St\r\n";
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let r = records(&bytes, Some(Format::Tab)).unwrap();
    assert_eq!(r.count, 1, "a line break in quotes does not split the record");
    let parsed = parse_pick(&bytes, None, Some(Format::Tab)).unwrap();
    import_one(&eng, id, &parsed, EditSource::User).unwrap();
    assert_eq!(dto(&eng, id, "Full name")[0].value, "He said \"hi\"");
    assert_eq!(dto(&eng, id, "Notes")[0].value, "line one\nline\ttwo, \"x\"");
    assert_eq!(dto(&eng, id, "Address.Street")[0].value, "Main St");
    // A quote that starts inside a cell is just a character.
    let f = parse_pick(b"A\tB\r\n5\" pipe\tx\r\n", None, Some(Format::Tab)).unwrap().fields;
    assert_eq!(f[0].1, Val::Cell("5\" pipe".into()));
}

#[test]
fn a_field_name_that_is_not_an_xml_name_exports_as_valid_xml_and_comes_back() {
    let make = |t: &str| {
        let (eng, id) = table_form(t);
        for (i, n) in ["m\u{b2}", "1st", "a<b>&\"c"].iter().enumerate() {
            add_one(&eng, id, 0, &nf(FieldKind::Text, n, [320.0, 100.0 + 40.0 * i as f32, 420.0, 130.0 + 40.0 * i as f32]), EditSource::User).unwrap();
        }
        (eng, id)
    };
    let (a, ida) = make("xml-names-a");
    for (n, v) in [("m\u{b2}", "one"), ("1st", "two"), ("a<b>&\"c", "three")] {
        set(&a, ida, n, text(v));
    }
    let (bytes, _) = export(&a, ida, Format::Xml, "a.pdf").unwrap();
    let xml = String::from_utf8(bytes.clone()).unwrap();
    assert!(xml.contains("<m_ xfdf:original=\"m\u{b2}\">one</m_>"), "{xml}");
    // Valid: our own reader (quick-xml underneath) takes it, and no element name has a character XML forbids.
    let root = exchange::parse_xml(xml.trim_start_matches('\u{FEFF}').as_bytes()).unwrap();
    assert_eq!(root.name, "fields");
    for k in &root.kids {
        assert!(k.tag.chars().next().is_some_and(name_start) && k.tag.chars().all(name_char), "{}", k.tag);
    }
    let (b, idb) = make("xml-names-b");
    let r = import_one(&b, idb, &parse(&bytes).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.skipped_total, 1, "only the frozen field: {:?}", r.summary);
    assert_eq!(dto(&b, idb, "m\u{b2}")[0].value, "one");
    assert_eq!(dto(&b, idb, "1st")[0].value, "two");
    assert_eq!(dto(&b, idb, "a<b>&\"c")[0].value, "three");
}

#[test]
fn a_keystroke_script_on_a_multi_select_list_replaces_the_first_pick() {
    let (eng, id) = table_form("tbl-multi-k");
    put_script(&eng, id, "Pick", "K", "if (event.value === \"no\") { event.value = \"se\"; }");
    try_set(&eng, id, "Pick", FieldValue::Many(vec!["no".into(), "se".into()])).unwrap();
    assert_eq!(dto(&eng, id, "Pick")[0].values, vec!["se", "se"]);
    // What is not an entry of the list is not followed.
    put_script(&eng, id, "Pick", "K", "event.value = \"zz\";");
    try_set(&eng, id, "Pick", FieldValue::Many(vec!["no".into(), "se".into()])).unwrap();
    assert_eq!(dto(&eng, id, "Pick")[0].values, vec!["no", "se"]);
}
