//! Every module that writes the document refuses a certified one and leaves no history step; a signed one
//! takes the edits that stay incremental and the file written still starts with the signed bytes.

use super::*;
use crate::engine::Engine;
use crate::outline::tests::{dir, doc_mdp, raw_pdf, sig_value};
use crate::marks::{AddRequest, Bates, Date, Mark, MarkKind};
use crate::organize::attach::NewFile;
use crate::organize::labels::LabelStyle;
use crate::organize::trans::{TransChange, TransStyle, Transition};
use crate::redact::Sanitize;
use std::sync::Arc;

const DATE: Date = Date { year: 2026, month: 10, day: 12 };

/// Two pages (text on the first, a note and a redaction mark, an attached file), the catalog and the
/// signature field (object 8) as given.
fn pdf(catalog: &str, field: &str) -> Vec<u8> {
    let content = "BT /F1 24 Tf 72 700 Td (Chapter One) Tj ET BT /F1 12 Tf 72 600 Td (Plain body text of the document, long enough.) Tj ET";
    raw_pdf(&[
        format!("<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(a.txt) 10 0 R] >> >> {catalog} >>"),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 6 0 R /Annots [7 0 R 9 0 R] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 6 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Annot /Subtype /Text /Rect [100 100 120 120] /Contents (a note) /T (A) >>".to_string(),
        field.to_string(),
        "<< /Type /Annot /Subtype /Redact /Rect [72 690 200 720] /QuadPoints [72 720 200 720 72 690 200 690] /C [1 0 0] >>".to_string(),
        "<< /Type /Filespec /F (a.txt) /UF (a.txt) /EF << /F 11 0 R >> >>".to_string(),
        "<< /Type /EmbeddedFile /Length 5 >>\nstream\nhello\nendstream".to_string(),
    ])
}

fn field(refs: &str) -> String {
    format!("<< /FT /Sig /T (S) /Type /Annot /Subtype /Widget /Rect [0 0 0 0] {} >>", sig_value(refs))
}

fn perms(p: u32) -> String {
    format!("/Perms << /DocMDP << /Type /Sig /Reference [ {} ] >> >>", doc_mdp(&format!("/TransformParams << /P {p} >>")))
}

fn p1_in_the_field() -> Vec<u8> {
    pdf("/AcroForm << /SigFlags 3 /Fields [8 0 R] >>", &field(&doc_mdp("/TransformParams << /P 1 >>")))
}

fn p2_in_the_catalog() -> Vec<u8> {
    pdf(&perms(2), "<< >>")
}

fn p3_in_the_catalog() -> Vec<u8> {
    pdf(&perms(3), "<< >>")
}

fn signed_only() -> Vec<u8> {
    pdf("/AcroForm << /SigFlags 3 /Fields [8 0 R] >>", &field(""))
}

type Op = Box<dyn Fn(&Arc<Engine>, &str, DocId) -> Result<()>>;

fn custom(eng: &Arc<Engine>, id: DocId, f: impl FnOnce(&mut PdfDocument) -> Result<Vec<u32>>) -> Result<()> {
    eng.apply_custom(id, "A change".into(), EditSource::User, false, f).map(|_| ())
}

fn removing(eng: &Arc<Engine>, id: DocId, f: impl FnOnce(&mut PdfDocument) -> Result<Vec<u32>>) -> Result<()> {
    eng.apply_removing(id, "A change".into(), EditSource::User, f).map(|_| ())
}

fn user() -> EditSource {
    EditSource::User
}

fn bates() -> AddRequest {
    AddRequest { mark: Mark::Bates(Bates::default()), pages: vec![0], replace: false, date: DATE }
}

fn attachment_id(eng: &Engine, id: DocId) -> String {
    eng.read_custom(id, |d| crate::organize::attach::list(d)).unwrap().remove(0).id
}

/// (name, is a comment or markup: a /P 3 certification allows these, the operation).
fn ops() -> Vec<(&'static str, bool, Op)> {
    let note = crate::annot::NewAnnot::Note { at: [50.0, 50.0], text: "n".into() };
    let patch = crate::annot::Patch { contents: Some("changed".into()), ..Default::default() };
    let note2 = note.clone();
    let sheet = crate::organize::labels::LabelSpec { style: LabelStyle::LowerRoman, prefix: String::new(), start: 3, extend: false };
    let fade = TransChange {
        transition: Some(Transition { style: TransStyle::Dissolve, duration: 1.0, dimension: None, motion: None, direction: None }),
        auto_advance: None,
    };
    let mut v: Vec<(&'static str, bool, Op)> = Vec::new();
    macro_rules! op {
        ($name:expr, $annot:expr, |$eng:ident, $path:ident, $id:ident| $body:expr) => {
            v.push(($name, $annot, Box::new(move |$eng: &Arc<Engine>, $path: &str, $id: DocId| -> Result<()> {
                let _ = $path;
                $body
            })));
        };
    }
    op!("rotate", false, |eng, _p, id| eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).map(|_| ()));
    op!("delete page", false, |eng, _p, id| eng.apply(id, EditOp::Delete { pages: vec![1] }).map(|_| ()));
    op!("insert blank", false, |eng, _p, id| eng.apply(id, EditOp::InsertBlank { at: 1, like: 0 }).map(|_| ()));
    op!("extract and delete", false, |eng, p, id| {
        let out = format!("{p}.out.pdf");
        let r = eng.extract(id, &[0], &out, true).map(|_| ());
        assert!(!std::path::Path::new(&out).exists(), "the extract was written before the refusal");
        r
    });
    op!("add a comment", true, |eng, _p, id| custom(eng, id, |d| {
        crate::annot::add(d, 0, &note, &Default::default(), "me")?;
        Ok(vec![0])
    }));
    op!("change a comment", true, |eng, _p, id| custom(eng, id, |d| {
        crate::annot::edit(d, 0, 7, &patch, "me")?;
        Ok(vec![0])
    }));
    op!("delete a comment", true, |eng, _p, id| custom(eng, id, |d| {
        crate::annot::delete(d, 0, &[7])?;
        Ok(vec![0])
    }));
    op!("reply to a comment", true, |eng, _p, id| custom(eng, id, |d| {
        crate::annot::reply(d, 0, 7, "re", "me")?;
        Ok(vec![0])
    }));
    op!("review state", true, |eng, _p, id| custom(eng, id, |d| {
        crate::annot::set_state(d, 0, 7, "Accepted", "me")?;
        Ok(vec![0])
    }));
    op!("measurement", true, |eng, _p, id| crate::measure::add_one(
        eng, id, 0, crate::measure::Kind::Distance, &[[10.0, 10.0], [100.0, 10.0]], None, "me", user()
    )
    .map(|_| ()));
    op!("measurement scale", false, |eng, _p, id| crate::measure::scale_set(eng, id, 0, &crate::measure::Scale::default(), user()).map(|_| ()));
    op!("mark an area for redaction", true, |eng, _p, id| crate::redact::app::mark_area(eng, id, 0, [10.0, 10.0, 80.0, 40.0], user()).map(|_| ()));
    op!("mark pages for redaction", true, |eng, _p, id| crate::redact::app::mark_pages(eng, id, &[1], user()).map(|_| ()));
    op!("remove a redaction mark", true, |eng, _p, id| crate::redact::app::unmark(eng, id, &[(0, 9)], user()).map(|_| ()));
    op!("apply redactions", false, |eng, _p, id| crate::redact::app::apply(eng, id, Sanitize::default(), None, user()).map(|_| ()));
    op!("add a mark", false, |eng, _p, id| custom(eng, id, |d| crate::marks::add(d, &bates())));
    op!("remove a mark", false, |eng, _p, id| custom(eng, id, |d| {
        crate::marks::remove(d, MarkKind::Bates, &[0])?;
        Ok(vec![0])
    }));
    op!("page labels", false, |eng, _p, id| crate::organize::labels_apply(eng, id, 0, 0, &sheet, user()).map(|_| ()));
    op!("transition", false, |eng, _p, id| crate::organize::trans_set(eng, id, &[0], &fade, user()).map(|_| ()));
    op!("attach a file", false, |eng, _p, id| crate::organize::attach_add(
        eng, id, vec![NewFile { name: "b.txt".into(), data: b"b".to_vec(), description: String::new(), modified: None }], user()
    )
    .map(|_| ()));
    op!("describe an attachment", false, |eng, _p, id| {
        let a = attachment_id(eng, id);
        crate::organize::attach_describe(eng, id, &a, "described", user()).map(|_| ())
    });
    op!("delete an attachment", false, |eng, _p, id| {
        let a = attachment_id(eng, id);
        crate::organize::attach_remove(eng, id, &[a], user()).map(|_| ())
    });
    op!("edit text", false, |eng, _p, id| {
        let rev = eng.edit_state(id)?.rev;
        let change = crate::textedit::BlockChange { text: Some("Changed".into()), ..Default::default() };
        crate::textedit::edit(eng, id, rev, 0, 0, &change, user()).map(|_| ())
    });
    op!("add text", false, |eng, _p, id| {
        let rev = eng.edit_state(id)?.rev;
        let new = crate::textedit::NewTextBox { text: "New".into(), size: 12.0, color: [0.0; 3], at: [72.0, 72.0], width: None };
        crate::textedit::add(eng, id, rev, 0, &new, user()).map(|_| ())
    });
    op!("change an image", false, |eng, _p, id| {
        let rev = eng.edit_state(id)?.rev;
        crate::imageedit::edit(eng, id, rev, 0, Some(0), &crate::imageedit::ImageChange::Move { dx: 1.0, dy: 1.0 }, user()).map(|_| ())
    });
    let _ = note2;
    v
}

fn certified_message(r: &Result<()>) -> String {
    match r {
        Ok(()) => "(succeeded)".into(),
        Err(e) => e.to_string(),
    }
}

#[test]
fn a_certified_document_is_refused_by_every_write_and_gets_no_history_step() {
    for (variant, bytes, allows_comments) in [
        ("p2-catalog", p2_in_the_catalog(), false),
        ("p1-field", p1_in_the_field(), false),
        ("p3-catalog", p3_in_the_catalog(), true),
    ] {
        for (name, comment, op) in ops() {
            let d = dir("certified", &format!("{variant}-{}", name.replace(' ', "-")));
            let (eng, id, path) = crate::outline::tests::open_bytes(&d, "a.pdf", &bytes);
            let before = eng.edit_state(id).unwrap();
            let r = op(&eng, &path, id);
            let after = eng.edit_state(id).unwrap();
            if allows_comments && comment {
                assert!(r.is_ok(), "{variant}: {name} is a comment and /P 3 allows it: {}", certified_message(&r));
                assert!(after.rev > before.rev, "{variant}: {name} made no step");
            } else {
                let m = certified_message(&r);
                assert!(m.contains("certified"), "{variant}: {name} was not refused for the certification: {m}");
                assert_eq!((after.rev, &after.undo, after.dirty), (before.rev, &before.undo, before.dirty), "{variant}: {name} left a step");
            }
        }
    }
}

#[test]
fn a_certified_document_cannot_be_given_password_security() {
    use crate::protect::ProtectSpec;
    let d = dir("certified", "protect");
    let (eng, id, _) = crate::outline::tests::open_bytes(&d, "a.pdf", &p2_in_the_catalog());
    let spec = ProtectSpec {
        open_password: Some("pw".into()),
        permissions_password: None,
        printing: Default::default(),
        changes: Default::default(),
        copy: true,
        accessibility: true,
        cipher: Default::default(),
    };
    let e = eng.protect(id, spec, None, user()).unwrap_err().to_string();
    assert!(e.contains("certified"), "{e}");
    assert!(eng.edit_state(id).unwrap().undo.is_none());
}

#[test]
fn a_signed_document_takes_the_incremental_edits_and_is_saved_after_the_signed_bytes() {
    let bytes = signed_only();
    let d = dir("certified", "signed");
    let (eng, id, path) = crate::outline::tests::open_bytes(&d, "a.pdf", &bytes);
    // Edits that add or change; then the ones that take something out (the saved file keeps the old objects).
    for (name, comment, op) in ops() {
        if ["rotate", "add a comment", "page labels", "transition", "attach a file", "add a mark", "remove a mark", "describe an attachment",
            "delete an attachment", "edit text", "add text", "delete page", "mark an area for redaction", "measurement scale"]
            .contains(&name)
        {
            let before = eng.edit_state(id).unwrap().rev;
            let r = op(&eng, &path, id);
            assert!(r.is_ok(), "{name}: {}", certified_message(&r));
            assert!(eng.edit_state(id).unwrap().rev > before, "{name}: no step");
        }
        let _ = comment;
    }
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.starts_with(&bytes), "the signed bytes are not the start of the saved file");
}

#[test]
fn a_signed_document_is_not_redacted_in_place_nor_given_password_security() {
    let d = dir("certified", "signed-redact");
    let (eng, id, _) = crate::outline::tests::open_bytes(&d, "a.pdf", &signed_only());
    let e = crate::redact::app::apply(&eng, id, Sanitize::default(), None, user()).unwrap_err().to_string();
    assert!(e.contains("signed"), "{e}");
    use crate::protect::ProtectSpec;
    let spec = ProtectSpec {
        open_password: Some("pw".into()),
        permissions_password: None,
        printing: Default::default(),
        changes: Default::default(),
        copy: true,
        accessibility: true,
        cipher: Default::default(),
    };
    let e = eng.protect(id, spec, None, user()).unwrap_err().to_string();
    assert!(e.contains("signed"), "{e}");
    assert!(eng.edit_state(id).unwrap().undo.is_none());
}
