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
            let short_list = string_over_limit(&list);
            list.array_push(s(MAX_STR + 1)?)?;
            Ok([
                string_over_limit(&s(2 * MAX_STR + 3)?),
                string_over_limit(&s(MAX_STR + 1)?),
                string_over_limit(&PdfObject::new_name("On")?),
                short_list,
                string_over_limit(&list),
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
    // The 8 MiB budget pays for four decodes of this string; the other 4092 entries are not decoded.
    assert!(DECODES.with(|d| d.get()) <= 4, "decoded {} times", DECODES.with(|d| d.get()));
}
