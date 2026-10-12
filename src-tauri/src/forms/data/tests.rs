//! Form data export and import: every field kind through FDF, XFDF and JSON, what import reports and
//! skips, locked fields, certified and signed documents, hostile files.

use super::*;
use crate::engine::fixture::{self, Fixture};
use crate::forms::{FieldPatch, NewField, Opt, add_one, edit_one, list_all, put_name, set_value_one};
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
        assert_eq!(r.summary.set, 5, "{ext}: {:?}", r.summary);
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
fn a_certified_document_refuses_import_with_no_history_step_but_exports() {
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
