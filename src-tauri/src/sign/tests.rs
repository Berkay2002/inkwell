//! Signing through the edit lane and validating the result: incremental updates that keep earlier
//! signatures valid, DocMDP certification, tampering, empty fields, appearance pictures, and the
//! Windows CNG path (an ephemeral PFX key, never the user's certificate store).
//! Every ID here is a self-signed test ID made in the test.

use super::app::{list, sign_step, system_trust};
use super::crypto::cms::{self, SignedData};
use super::crypto::der::{self, tag};
use super::crypto::{Certificate, DigestAlg, DigitalId, Name, PrivateKey, Time};
use super::pdf::{self, Changes, SignDate, SignRequest, SignatureInfo, Status, WarningCode};
use crate::annot::{NewAnnot, Style, add_one};
use crate::engine::edit::{EditOp, EditSource};
use crate::engine::fixture::{self, Fixture};
use crate::engine::{DocId, Engine};
use mupdf::pdf::{PdfDocument, PdfObject, PdfWriteOptions};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("sign-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open_bytes(test: &str, bytes: &[u8]) -> (Arc<Engine>, DocId, String) {
    let p = dir(test).join("a.pdf");
    std::fs::write(&p, bytes).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

fn open(test: &str) -> (Arc<Engine>, DocId, String) {
    open_bytes(test, &fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() }))
}

/// Key generation is slow in debug builds: two IDs for the whole file.
fn ids() -> &'static (DigitalId, DigitalId) {
    static IDS: OnceLock<(DigitalId, DigitalId)> = OnceLock::new();
    IDS.get_or_init(|| {
        (
            DigitalId::create_self_signed("Maple Property Test", "landlord@example.com", "Inkwell Tests", 2).unwrap(),
            DigitalId::create_self_signed("Alex Johnson Test", "tenant@example.com", "Inkwell Tests", 2).unwrap(),
        )
    })
}

fn req(page: u32, rect: [f32; 4]) -> SignRequest {
    SignRequest { page, rect: Some(rect), reason: Some("Lease signed".into()), date: Some(SignDate::new(1_791_800_000, 120)), ..Default::default() }
}

/// Trusts exactly the two test IDs (in place of the Windows root store).
fn trust_tests(c: &Certificate, _: &[Certificate]) -> (bool, String) {
    let (a, b) = ids();
    if c.raw == a.cert.raw || c.raw == b.cert.raw { (true, String::new()) } else { (false, "not a test ID".into()) }
}

fn codes(s: &SignatureInfo) -> Vec<WarningCode> {
    s.warnings.iter().map(|w| w.code).collect()
}

fn infos(bytes: &[u8]) -> Vec<SignatureInfo> {
    let d = mupdf::pdf::PdfDocument::from_bytes(bytes).unwrap();
    pdf::list(&d, bytes, &trust_tests).unwrap()
}

#[test]
fn a_signature_is_one_undo_step_appended_to_the_file_and_valid_after_reopen() {
    let (eng, id, path) = open("one");
    let before = std::fs::read(&path).unwrap();
    let steps = eng.history(id).unwrap().len();
    sign_step(&eng, id, &ids().0, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign as Maple".into()).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    eng.save(id, None).unwrap();
    let after = std::fs::read(&path).unwrap();
    assert!(after.len() > before.len() && after[..before.len()] == before[..], "an incremental update of the saved file");

    let i = infos(&after);
    assert_eq!(i.len(), 1);
    let s = &i[0];
    assert_eq!((s.status, s.intact, s.changes), (Status::Valid, true, Changes::None), "{s:?}");
    assert_eq!(s.signer, "Maple Property Test");
    assert_eq!(s.reason, "Lease signed");
    assert_eq!(s.sub_filter, "ETSI.CAdES.detached");
    assert_eq!(s.time, "D:20261012121320+02'00'");
    assert_eq!(s.page, 0);
    assert!((s.rect[0] - 300.0).abs() < 0.5 && (s.rect[3] - 680.0).abs() < 0.5, "{:?}", s.rect);

    // Through the app's own listing (the Windows trust store): intact, signer unknown.
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let s = &list(&eng2, id2).unwrap()[0];
    assert_eq!((s.status, s.intact, s.trusted), (Status::UnknownSigner, true, false), "{s:?}");

    // Undo takes the signature away again.
    eng.undo(id).unwrap();
    assert!(infos(&eng.current_bytes(id).unwrap()).is_empty());
}

#[test]
fn the_cms_is_pades_b_b() {
    let (eng, id, _) = open("pades");
    sign_step(&eng, id, &ids().0, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign".into()).unwrap();
    let bytes = eng.current_bytes(id).unwrap();
    let d = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let v = d.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap().get_dict("Fields").unwrap().unwrap().get_array(0).unwrap().unwrap().get_dict("V").unwrap().unwrap();
    let cms = v.get_dict("Contents").unwrap().unwrap().as_bytes().unwrap();
    let sd = SignedData::parse(&cms).unwrap();
    let attrs = sd.signer.signed_attrs.clone().unwrap();
    let has = |oid: &[u8]| attrs.windows(oid.len()).any(|w| w == oid);
    assert!(has(&[0x06, 0x0B, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x10, 0x02, 0x2F]), "signingCertificateV2");
    assert!(!has(&[0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x05]), "no signingTime");
    assert_eq!(d.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap().get_dict("SigFlags").unwrap().unwrap().as_int().unwrap(), 3);
}

#[test]
fn a_second_signature_keeps_the_first_valid_and_an_edit_after_is_reported() {
    let (eng, id, path) = open("two");
    let (a, b) = ids();
    sign_step(&eng, id, a, &req(0, [72.0, 600.0, 280.0, 680.0]), "Sign A".into()).unwrap();
    eng.save(id, None).unwrap();
    sign_step(&eng, id, b, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign B".into()).unwrap();
    eng.save(id, None).unwrap();
    let i = infos(&std::fs::read(&path).unwrap());
    assert_eq!(i.len(), 2);
    let first = i.iter().find(|s| s.signer.starts_with("Maple")).unwrap();
    let second = i.iter().find(|s| s.signer.starts_with("Alex")).unwrap();
    // Only a signature came after the first, but its box is drawn on the page the first signed.
    assert_eq!((first.status, first.changes, codes(first)), (Status::Warning, Changes::SignaturesOnly, vec![WarningCode::LaterBox]), "{first:?}");
    assert_eq!((second.status, second.changes), (Status::Valid, Changes::None), "{second:?}");

    // A comment after signing: both stay intact, and both warn that the document changed.
    add_one(&eng, id, 1, &NewAnnot::Note { at: [100.0, 100.0], text: "after".into() }, &Style::default(), "T", EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    for s in infos(&std::fs::read(&path).unwrap()) {
        assert_eq!((s.status, s.intact, s.changes), (Status::Warning, true, Changes::Changed), "{s:?}");
        assert_eq!(s.warnings[0].text, "The document was changed after it was signed.", "{s:?}");
        assert_eq!(codes(&s)[0], WarningCode::Changed, "{s:?}");
    }

    // Deleting a page after signing appends too (no rewrite that would break the signatures).
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    eng.save(id, None).unwrap();
    for s in infos(&std::fs::read(&path).unwrap()) {
        assert!(s.intact, "{s:?}");
    }
}

#[test]
fn a_changed_byte_makes_the_signature_invalid() {
    let (eng, id, _) = open("tamper");
    sign_step(&eng, id, &ids().0, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign".into()).unwrap();
    let mut bytes = eng.current_bytes(id).unwrap();
    // A byte inside the page content stream of the original file.
    let at = bytes.windows(2).position(|w| w == b"Tj").unwrap();
    bytes[at - 2] ^= 0x01;
    let s = &infos(&bytes)[0];
    assert_eq!((s.status, s.intact), (Status::Invalid, false), "{s:?}");
    assert!(s.summary.contains("changed after it was signed"), "{s:?}");
    // The CMS on its own still checks out: its signature over the signed attributes is fine.
    // Only messageDigest, compared with the bytes, catches the change.
    let d = PdfDocument::from_bytes(&bytes).unwrap();
    let v = d.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap().get_dict("Fields").unwrap().unwrap().get_array(0).unwrap().unwrap().get_dict("V").unwrap().unwrap();
    let sd = SignedData::parse(&v.get_dict("Contents").unwrap().unwrap().as_bytes().unwrap()).unwrap();
    assert!(sd.verify_signature(&ids().0.cert, &[]).unwrap());
}

#[test]
fn certification_rules() {
    let (eng, id, _) = open("certify");
    let (a, b) = ids();
    let mut r = req(0, [72.0, 600.0, 280.0, 680.0]);
    r.certify = Some(1);
    sign_step(&eng, id, a, &r, "Certify".into()).unwrap();
    let s = &infos(&eng.current_bytes(id).unwrap())[0];
    assert_eq!(s.certify, Some(1));
    // No changes allowed: a second signature is refused.
    assert!(sign_step(&eng, id, b, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign".into()).is_err());

    // Certifying a document that is already signed is refused.
    let (eng, id, _) = open("certify-late");
    sign_step(&eng, id, b, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign".into()).unwrap();
    let mut r = req(0, [72.0, 600.0, 280.0, 680.0]);
    r.certify = Some(2);
    assert!(sign_step(&eng, id, a, &r, "Certify".into()).is_err());
}

#[test]
fn a_page_deleted_before_signing_is_gone_from_the_signed_bytes() {
    let (eng, id, path) = open("dirty");
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    sign_step(&eng, id, &ids().0, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign".into()).unwrap();
    eng.save(id, None).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let d = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    assert_eq!(d.page_count().unwrap(), 1);
    assert_eq!(infos(&bytes)[0].status, Status::Valid);
}

/// The fixture with an empty signature field "Tenant" on page 1.
fn with_empty_field() -> Vec<u8> {
    let base = fixture::build(&Fixture { pages: 1, lines: 4, ..Default::default() });
    let mut doc = mupdf::pdf::PdfDocument::from_bytes(&base).unwrap();
    let mut page = doc.find_page(0).unwrap();
    let w = doc.new_object_from_str("<</Type/Annot/Subtype/Widget/FT/Sig/T(Tenant)/F 4/Rect[300 100 520 180]>>").unwrap();
    let mut w = doc.add_object(&w).unwrap();
    w.dict_put("P", page.clone()).unwrap();
    let mut annots = doc.new_array().unwrap();
    annots.array_push(w.clone()).unwrap();
    page.dict_put("Annots", annots).unwrap();
    let mut fields = doc.new_array().unwrap();
    fields.array_push(w).unwrap();
    let mut af = doc.new_dict().unwrap();
    af.dict_put("Fields", fields).unwrap();
    doc.catalog().unwrap().dict_put("AcroForm", af).unwrap();
    let mut out = Vec::new();
    doc.write_to(&mut out).unwrap();
    out
}

#[test]
fn an_empty_field_is_listed_and_can_be_signed() {
    let (eng, id, _) = open_bytes("field", &with_empty_field());
    let i = infos(&eng.current_bytes(id).unwrap());
    assert_eq!(i.len(), 1);
    assert_eq!((i[0].field.as_str(), i[0].status), ("Tenant", Status::Unsigned));
    let r = SignRequest { field: Some("Tenant".into()), ..req(0, [0.0; 4]) };
    sign_step(&eng, id, &ids().1, &r, "Sign".into()).unwrap();
    let s = &infos(&eng.current_bytes(id).unwrap())[0];
    assert_eq!((s.field.as_str(), s.status, s.signer.as_str()), ("Tenant", Status::Valid, "Alex Johnson Test"));
    // Signing it twice is refused.
    assert!(sign_step(&eng, id, &ids().0, &r, "Sign".into()).is_err());
}

#[test]
fn the_appearance_can_carry_a_picture_and_renders() {
    let (eng, id, _) = open("picture");
    // A small transparent PNG: a dark diagonal stroke.
    let mut img = image::RgbaImage::new(60, 20);
    for x in 0..60 {
        img.put_pixel(x, (x / 3).min(19), image::Rgba([31, 42, 68, 255]));
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let r = SignRequest { image: Some(png), ..req(0, [300.0, 600.0, 520.0, 680.0]) };
    sign_step(&eng, id, &ids().0, &r, "Sign".into()).unwrap();
    let bytes = eng.current_bytes(id).unwrap();
    let s = &infos(&bytes)[0];
    assert_eq!(s.status, Status::Valid);
    let d = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let w = d.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap().get_dict("Fields").unwrap().unwrap().get_array(0).unwrap().unwrap();
    let ap = w.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
    let xo = ap.get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap();
    assert!(xo.dict_len().unwrap() >= 1, "the picture is an XObject of the appearance");
    // It draws: the page has ink inside the signature box.
    let page = mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap().load_page(0).unwrap();
    let pix = page.to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_gray(), false, true).unwrap();
    let (wd, sam) = (pix.width() as usize, pix.samples());
    let dark = (605..675).flat_map(|y| (305..515).map(move |x| (x, y))).filter(|&(x, y)| sam[y * wd + x] < 128).count();
    assert!(dark > 50, "dark pixels in the box: {dark}");

    // A small file that claims a huge canvas is refused before it is decoded.
    let mut huge = Vec::new();
    image::DynamicImage::ImageLuma8(image::GrayImage::new(5000, 1)).write_to(&mut std::io::Cursor::new(&mut huge), image::ImageFormat::Png).unwrap();
    assert!(huge.len() < 1000);
    let r = SignRequest { image: Some(huge), ..req(0, [300.0, 600.0, 520.0, 680.0]) };
    assert!(sign_step(&eng, id, &ids().1, &r, "Sign".into()).is_err());
}

#[cfg(windows)]
#[test]
fn a_cng_key_signs_a_document() {
    let pfx = super::crypto::pkcs12::write(&ids().1, "pw").unwrap();
    let cng = super::crypto::winstore::import_pfx_ephemeral(&pfx, "pw").unwrap();
    assert!(cng.key.is_external());
    let (eng, id, _) = open("cng");
    sign_step(&eng, id, &cng, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign".into()).unwrap();
    let s = &infos(&eng.current_bytes(id).unwrap())[0];
    assert_eq!((s.status, s.intact), (Status::Valid, true), "{s:?}");
}

#[test]
fn system_trust_does_not_trust_a_test_id() {
    assert!(!system_trust(&ids().0.cert, &[]).0);
}

// ---- tampering after signing --------------------------------------------------------------------

/// `bytes` with an incremental update made by `f` appended.
fn append(bytes: &[u8], f: impl FnOnce(&mut PdfDocument)) -> Vec<u8> {
    let mut d = PdfDocument::from_bytes(bytes).unwrap();
    f(&mut d);
    let mut o = PdfWriteOptions::default();
    o.set_incremental(true);
    let mut out = Vec::new();
    d.write_to_with_options(&mut out, o).unwrap();
    assert!(out.len() > bytes.len() && out.starts_with(bytes));
    out
}

fn push_annot(d: &mut PdfDocument, mut page: PdfObject, a: PdfObject) {
    match page.get_dict("Annots").unwrap() {
        Some(mut arr) => arr.array_push(a).unwrap(),
        None => {
            let mut arr = d.new_array().unwrap();
            arr.array_push(a).unwrap();
            page.dict_put("Annots", arr).unwrap();
        }
    }
}

/// A sticky note on page 1, added in a new revision.
fn with_comment(bytes: &[u8]) -> Vec<u8> {
    append(bytes, |d| {
        let page = d.find_page(0).unwrap();
        let o = d.new_object_from_str("<</Type/Annot/Subtype/Text/Rect[10 10 30 30]/Contents(after)>>").unwrap();
        let a = d.add_object(&o).unwrap();
        push_annot(d, page, a);
    })
}

fn signed(test: &str, certify: Option<u8>) -> Vec<u8> {
    let (eng, id, _) = open(test);
    let r = SignRequest { certify, ..req(0, [300.0, 600.0, 520.0, 680.0]) };
    sign_step(&eng, id, &ids().0, &r, "Sign".into()).unwrap();
    eng.current_bytes(id).unwrap()
}

#[test]
fn a_change_after_certifying_breaks_the_certification() {
    for p in [1, 2] {
        let s = &infos(&with_comment(&signed(&format!("certified-{p}"), Some(p))))[0];
        assert_eq!((s.status, s.intact, s.changes, s.certify), (Status::Invalid, true, Changes::Changed, Some(p)), "P={p}: {s:?}");
        assert!(s.summary.contains("certified"), "{s:?}");
    }
}

#[test]
fn an_update_without_eof_or_junk_after_a_signature_is_a_change() {
    // The update's %%EOF cut off: MuPDF still loads the comment through startxref.
    let mut t = with_comment(&signed("no-eof", None));
    let eof = t.windows(5).rposition(|w| w == b"%%EOF").unwrap();
    t.truncate(eof);
    let annots = PdfDocument::from_bytes(&t).unwrap().find_page(0).unwrap().get_dict("Annots").unwrap().unwrap();
    assert!(annots.len().unwrap() >= 1, "the unterminated update is what a viewer shows");
    let s = &infos(&t)[0];
    assert_eq!((s.status, s.changes), (Status::Warning, Changes::Changed), "{s:?}");

    // Junk after a second, real signature: neither signature covers it.
    let (eng, id, _) = open("junk");
    sign_step(&eng, id, &ids().0, &req(0, [72.0, 600.0, 280.0, 680.0]), "A".into()).unwrap();
    sign_step(&eng, id, &ids().1, &req(0, [300.0, 600.0, 520.0, 680.0]), "B".into()).unwrap();
    let mut t = eng.current_bytes(id).unwrap();
    assert!(infos(&t).iter().all(|s| s.intact && s.changes != Changes::Changed));
    t.extend_from_slice(b"\n9 0 obj\n<</Junk true>>\nendobj\n");
    for s in infos(&t) {
        assert_eq!((s.status, s.changes), (Status::Warning, Changes::Changed), "{s:?}");
    }
}

#[test]
fn a_signature_only_revision_cannot_rewrite_existing_objects() {
    let (eng, id, _) = open("forged");
    sign_step(&eng, id, &ids().0, &req(0, [72.0, 600.0, 280.0, 680.0]), "A".into()).unwrap();
    let prev = eng.current_bytes(id).unwrap();
    // A real second signature passes.
    sign_step(&eng, id, &ids().1, &req(0, [300.0, 600.0, 520.0, 680.0]), "B".into()).unwrap();
    assert!(pdf::signature_only_revision(&prev, &eng.current_bytes(id).unwrap()).unwrap());

    let sig_value = |d: &mut PdfDocument| {
        let v = d.new_object_from_str("<</Type/Sig/Filter/Adobe.PPKLite/SubFilter/ETSI.CAdES.detached>>").unwrap();
        d.add_object(&v).unwrap()
    };
    // The page's content stream rewritten as a "signed widget": the page goes blank.
    let forged = append(&prev, |d| {
        let page = d.find_page(0).unwrap();
        let mut contents = page.get_dict("Contents").unwrap().unwrap();
        assert!(contents.is_indirect().unwrap() && contents.is_stream().unwrap());
        let mut w = d.new_object_from_str("<</Type/Annot/Subtype/Widget/FT/Sig/T(Forged)/F 132/Rect[0 0 10 10]>>").unwrap();
        w.dict_put("V", sig_value(d)).unwrap();
        w.dict_put("P", page.clone()).unwrap();
        contents.write_object(&w).unwrap();
        push_annot(d, page, contents);
    });
    assert!(!pdf::signature_only_revision(&prev, &forged).unwrap());

    // A new widget whose /Parent is the page: the page becomes the "field", and changes freely.
    let forged = append(&prev, |d| {
        let mut page = d.find_page(0).unwrap();
        page.dict_put("FT", PdfObject::new_name("Sig").unwrap()).unwrap();
        page.dict_put("T", PdfObject::new_string("Forged").unwrap()).unwrap();
        page.dict_put("V", sig_value(d)).unwrap();
        page.dict_put("Rotate", PdfObject::new_int(90).unwrap()).unwrap();
        let w = d.new_object_from_str("<</Type/Annot/Subtype/Widget/F 132/Rect[0 0 10 10]>>").unwrap();
        let mut w = d.add_object(&w).unwrap();
        w.dict_put("Parent", page.clone()).unwrap();
        w.dict_put("P", page.clone()).unwrap();
        push_annot(d, page, w);
    });
    assert!(!pdf::signature_only_revision(&prev, &forged).unwrap());
}

#[test]
fn signing_an_existing_empty_field_after_another_signature_is_signatures_only() {
    let (eng, id, _) = open_bytes("field-second", &with_empty_field());
    sign_step(&eng, id, &ids().0, &req(0, [72.0, 600.0, 280.0, 680.0]), "A".into()).unwrap();
    let r = SignRequest { field: Some("Tenant".into()), ..req(0, [0.0; 4]) };
    sign_step(&eng, id, &ids().1, &r, "B".into()).unwrap();
    let i = infos(&eng.current_bytes(id).unwrap());
    let first = i.iter().find(|s| s.field != "Tenant").unwrap();
    assert_eq!((first.status, first.changes, codes(first)), (Status::Warning, Changes::SignaturesOnly, vec![WarningCode::LaterBox]), "{first:?}");
}

// ---- the signer's certificate and the CMS -------------------------------------------------------

#[test]
fn a_certificate_not_for_document_signing_is_never_valid() {
    let key = PrivateKey::from_pkcs8(ids().0.key.pkcs8()).unwrap();
    // A TLS server's certificate (extended key usage serverAuth only).
    let name = Name::build("tls.example.com", "", "", "", "");
    let cert = Certificate::self_signed_for(&name, &key, Time::from_unix(1_790_000_000), 2, &[5, 5, 5], &["1.3.6.1.5.5.7.3.1"]).unwrap();
    let tls = DigitalId { key, cert, chain: Vec::new(), friendly_name: None };
    let (eng, id, _) = open("tls");
    sign_step(&eng, id, &tls, &req(0, [300.0, 600.0, 520.0, 680.0]), "Sign".into()).unwrap();
    let bytes = eng.current_bytes(id).unwrap();
    let d = PdfDocument::from_bytes(&bytes).unwrap();
    // Even with a trust store that trusts everything.
    let s = &pdf::list(&d, &bytes, &|_: &Certificate, _: &[Certificate]| (true, String::new())).unwrap()[0];
    assert_eq!((s.status, s.intact, s.trusted), (Status::UnknownSigner, true, false), "{s:?}");
    assert!(s.trust_note.contains("not issued for signing documents"), "{s:?}");
}

/// `bytes` with its newest signature's CMS replaced by `make(the bytes it covers)`.
fn recms(bytes: &[u8], make: impl FnOnce(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let at = bytes.windows(10).rposition(|w| w == b"/ByteRange").unwrap();
    let open = at + bytes[at..].iter().position(|&c| c == b'[').unwrap();
    let close = open + bytes[open..].iter().position(|&c| c == b']').unwrap();
    let n: Vec<usize> = std::str::from_utf8(&bytes[open + 1..close]).unwrap().split_whitespace().map(|x| x.parse().unwrap()).collect();
    let [_, b, c, d] = n[..] else { panic!("{n:?}") };
    let hex: String = make(&[&bytes[..b], &bytes[c..c + d]].concat()).iter().map(|x| format!("{x:02X}")).collect();
    let mut out = bytes.to_vec();
    let gap = &mut out[b + 1..c - 1];
    assert!(hex.len() <= gap.len());
    gap.fill(b'0');
    gap[..hex.len()].copy_from_slice(hex.as_bytes());
    out
}

/// A CMS from the first test ID over the data, with `content_type`, naming `ess` as its certificate.
fn cms_by<'a>(ess: &'a Certificate, content_type: &'a str, alg: DigestAlg) -> impl FnOnce(&[u8]) -> Vec<u8> + 'a {
    move |data| {
        let a = &ids().0;
        cms::build(&a.key, &a.cert, &[], alg, content_type, None, &alg.digest(&[data]), ess).unwrap()
    }
}

/// A CMS from the first test ID built field by field: the document hashed with `digest`, signed
/// with SHA-256, with (CAdES) or without signed attributes.
fn by_hand(data: &[u8], digest: DigestAlg, attrs: bool) -> Vec<u8> {
    let a = &ids().0;
    let sha256 = DigestAlg::Sha256;
    let attr = |o: &str, v: &[u8]| der::seq(&[&der::oid(o), &der::set_of(&[v])]);
    let names = der::seq(&[&der::explicit(4, &a.cert.issuer.raw)]);
    let ess_id = der::seq(&[&der::octets(&sha256.digest(&[&a.cert.raw])), &der::seq(&[&names, &der::uint(&a.cert.serial)])]);
    let set = der::set_of(&[
        &attr("1.2.840.113549.1.9.3", &der::oid(cms::DATA)),
        &attr("1.2.840.113549.1.9.4", &der::octets(&digest.digest(&[data]))),
        &attr("1.2.840.113549.1.9.16.2.47", &der::seq(&[&der::seq(&[&ess_id])])),
    ]);
    let sid = der::seq(&[&a.cert.issuer.raw, &der::uint(&a.cert.serial)]);
    let signer = if attrs {
        let mut signed = set.clone();
        signed[0] = tag::ctx(0);
        let signature = a.key.sign(sha256, &set).unwrap();
        der::seq(&[&der::int(1), &sid, &digest.algorithm(), &signed, &a.key.signature_algorithm(sha256), &der::octets(&signature)])
    } else {
        let signature = a.key.sign(sha256, data).unwrap();
        der::seq(&[&der::int(1), &sid, &sha256.algorithm(), &a.key.signature_algorithm(sha256), &der::octets(&signature)])
    };
    let sd = der::seq(&[
        &der::int(1),
        &der::set_of(&[&digest.algorithm()]),
        &der::seq(&[&der::oid(cms::DATA)]),
        &der::tlv(tag::ctx(0), &a.cert.raw),
        &der::set_of(&[&signer]),
    ]);
    der::seq(&[&der::oid(cms::SIGNED_DATA), &der::explicit(0, &sd)])
}

#[test]
fn the_cms_must_name_its_certificate_carry_data_and_use_a_strong_hash() {
    let bytes = signed("cms", None);
    let (a, b) = ids();
    // The same CMS rebuilt is valid (the helper itself is sound).
    assert_eq!(infos(&recms(&bytes, cms_by(&a.cert, cms::DATA, DigestAlg::Sha256)))[0].status, Status::Valid);

    let s = &infos(&recms(&bytes, cms_by(&b.cert, cms::DATA, DigestAlg::Sha256)))[0];
    assert_eq!(s.status, Status::Invalid, "{s:?}");
    assert!(s.summary.contains("different certificate"), "{s:?}");

    let s = &infos(&recms(&bytes, cms_by(&a.cert, cms::SIGNED_DATA, DigestAlg::Sha256)))[0];
    assert_eq!(s.status, Status::Invalid, "{s:?}");
    assert!(s.summary.contains("content type"), "{s:?}");

    // The document hashed with SHA-1 (Inkwell itself never signs that way).
    let s = &infos(&recms(&bytes, |data| by_hand(data, DigestAlg::Sha1, true)))[0];
    assert_eq!((s.status, s.intact, s.digest.as_str()), (Status::Warning, true, "SHA-1"), "{s:?}");
    assert_eq!(codes(s), [WarningCode::Sha1], "{s:?}");

    // CAdES without signed attributes: the signature over the bytes is right, but nothing binds
    // the certificate, so it is refused.
    let s = &infos(&recms(&bytes, |data| by_hand(data, DigestAlg::Sha256, false)))[0];
    assert_eq!(s.status, Status::Invalid, "{s:?}");
    assert!(s.summary.contains("without signed attributes"), "{s:?}");
}

#[test]
fn a_damaged_signed_file_is_never_rewritten() {
    let mut bytes = signed("damaged-src", None);
    // Point the last startxref nowhere: MuPDF repairs the file to open it.
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
    let digit = at + bytes[at..].iter().position(u8::is_ascii_digit).unwrap();
    bytes[digit] = if bytes[digit] == b'1' { b'2' } else { b'1' };
    let (eng, id, path) = open_bytes("damaged", &bytes);
    assert_eq!(eng.current_bytes(id).unwrap(), bytes, "opened as it is, not rewritten");
    let e = add_one(&eng, id, 0, &NewAnnot::Note { at: [100.0, 100.0], text: "x".into() }, &Style::default(), "T", EditSource::User).unwrap_err();
    assert!(e.to_string().contains("damaged"), "{e}");
    assert!(sign_step(&eng, id, &ids().1, &req(0, [72.0, 600.0, 280.0, 680.0]), "B".into()).is_err());
    eng.save(id, None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn security_and_signatures_never_meet() {
    use crate::protect::{Changes as Rights, Cipher, Printing, ProtectSpec};
    let spec = || ProtectSpec {
        open_password: None,
        permissions_password: Some("owner only".into()),
        printing: Printing::High,
        changes: Rights::AllButExtract,
        copy: false,
        accessibility: true,
        cipher: Cipher::Aes256,
    };
    let r = req(0, [300.0, 600.0, 520.0, 680.0]);
    // A signed document refuses new security: the rewrite would break its signatures.
    let (eng, id, _) = open_bytes("secure-signed", &signed("secure-signed-src", None));
    let e = eng.protect(id, spec(), None, EditSource::User).unwrap_err();
    assert!(e.to_string().contains("signed"), "{e}");
    // Pending security refuses a signature (Save would rewrite it away) ...
    let (eng, id, path) = open("secure-pending");
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    assert!(sign_step(&eng, id, &ids().0, &r, "Sign".into()).unwrap_err().to_string().contains("security"));
    // ... and so does a saved one, and an encrypted file opened as it is.
    eng.save(id, None).unwrap();
    assert!(sign_step(&eng, id, &ids().0, &r, "Sign".into()).is_err());
    let locked = std::fs::read(&path).unwrap();
    let (eng, id, _) = open_bytes("secure-encrypted", &locked);
    let e = sign_step(&eng, id, &ids().0, &r, "Sign".into()).unwrap_err();
    assert!(e.to_string().contains("password-protected"), "{e}");
}

// ---- forged "signature-only" revisions (incremental saving and shadow attacks) -------------------
// Each attack is a real signature by the second test ID whose revision also carries something
// else. The first signature must report that its document changed.

/// No box: the attacker's signature draws nothing, so only the forgery itself is judged.
fn unseen() -> SignRequest {
    SignRequest { rect: None, ..req(0, [0.0; 4]) }
}

/// The first test ID's signature on `bytes` (a new field on page 1).
fn victim(bytes: &[u8]) -> Vec<u8> {
    pdf::sign(bytes, &ids().0, &req(0, [72.0, 600.0, 280.0, 680.0])).unwrap()
}

/// The second test ID signs `bytes` with `r`, and `tamper` adds to the same revision.
fn attack(bytes: &[u8], r: &SignRequest, mut tamper: impl FnMut(&mut PdfDocument, &mut PdfObject)) -> Vec<u8> {
    pdf::sign_with(bytes, &ids().1, r, &mut tamper).unwrap()
}

/// The victim's and the attacker's signatures, after the attack.
fn verdicts(bytes: &[u8]) -> (SignatureInfo, SignatureInfo) {
    let i = infos(bytes);
    let pick = |who: &str| i.iter().find(|s| s.signer.starts_with(who)).unwrap_or_else(|| panic!("{who}: {i:?}")).clone();
    (pick("Maple"), pick("Alex"))
}

/// The victim reads "changed"; the attacker's own signature is real.
fn assert_caught(bytes: &[u8]) {
    let (v, a) = verdicts(bytes);
    assert_eq!((v.status, v.intact, v.changes), (Status::Warning, true, Changes::Changed), "victim: {v:?}");
    assert_eq!((a.status, a.changes), (Status::Valid, Changes::None), "attacker: {a:?}");
}

fn page1() -> Vec<u8> {
    fixture::build(&Fixture { pages: 1, lines: 4, ..Default::default() })
}

fn write(d: &mut PdfDocument) -> Vec<u8> {
    let mut out = Vec::new();
    d.write_to(&mut out).unwrap();
    out
}

#[test]
fn an_honest_second_signature_is_signatures_only() {
    let v = victim(&page1());
    let (first, _) = verdicts(&attack(&v, &unseen(), |_, _| {}));
    assert_eq!((first.status, first.changes, codes(&first)), (Status::Valid, Changes::SignaturesOnly, vec![]), "{first:?}");
}

#[test]
fn forged_a_second_catalog_swapped_in() {
    // Acrobat's layout (the form its own object), with a second catalog planted before signing:
    // the same pages and form, plus an action that runs on open.
    let mut d = PdfDocument::from_bytes(&page1()).unwrap();
    let form = d.new_object_from_str("<</Fields[]>>").unwrap();
    let form = d.add_object(&form).unwrap();
    d.catalog().unwrap().dict_put("AcroForm", form).unwrap();
    let mut evil = d.catalog().unwrap().copy_dict().unwrap();
    evil.dict_put("OpenAction", d.new_object_from_str("<</S/JavaScript/JS(app.alert(1))>>").unwrap()).unwrap();
    let planted = d.add_object(&evil).unwrap().as_indirect().unwrap();
    let v = victim(&write(&mut d));

    let forged = attack(&v, &unseen(), |d, _| {
        let root = d.new_indirect(planted, 0).unwrap();
        d.trailer().unwrap().dict_put("Root", root).unwrap();
    });
    let swapped = PdfDocument::from_bytes(&forged).unwrap().catalog().unwrap().get_dict("OpenAction").unwrap();
    assert!(swapped.is_some(), "the forged file opens the planted catalog");
    assert_caught(&forged);
}

#[test]
fn forged_page_content_of_the_same_length() {
    // The page's text rewritten in place, its dictionary (and /Length) untouched.
    let v = victim(&page1());
    let forged = attack(&v, &unseen(), |d, _| {
        let mut contents = d.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
        let mut raw = contents.read_raw_stream().unwrap();
        let at = raw.windows(2).position(|w| w == b"Tj").unwrap() - 3;
        raw[at] = if raw[at] == b'9' { b'8' } else { b'9' };
        contents.write_raw_stream_buffer(&mupdf::Buffer::from_bytes(&raw).unwrap()).unwrap();
    });
    assert_caught(&forged);
}

#[test]
fn forged_an_inline_form_changed() {
    // Inkwell's own layout: the form lives inline in the catalog.
    let v = victim(&page1());
    for (key, value) in [("NeedAppearances", "true"), ("XFA", "(<xdp/>)"), ("DA", "(/Helv 0 Tf 1 0 0 rg)")] {
        let forged = attack(&v, &unseen(), |d, _| {
            let mut form = d.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap();
            form.dict_put(key, d.new_object_from_str(value).unwrap()).unwrap();
        });
        assert_caught(&forged);
    }
}

#[test]
fn a_later_visible_box_is_reported_and_a_large_one_flagged() {
    let v = victim(&page1());
    // A box the size of a signature: drawn over the page the first signer signed.
    let (first, _) = verdicts(&attack(&v, &req(0, [300.0, 600.0, 520.0, 680.0]), |_, _| {}));
    assert_eq!((first.status, first.changes, codes(&first)), (Status::Warning, Changes::SignaturesOnly, vec![WarningCode::LaterBox]), "{first:?}");
    assert!(first.warnings[0].text.contains("page 1"), "{first:?}");
    // The whole page ("sneaky signature"), also when /Rect has a fifth number that MuPDF ignores.
    for rect in ["[0 0 612 792]", "[0 0 612 792 0]"] {
        let forged = attack(&v, &req(0, [300.0, 600.0, 520.0, 680.0]), |d, w| {
            w.dict_put("Rect", d.new_object_from_str(rect).unwrap()).unwrap();
        });
        let (first, _) = verdicts(&forged);
        assert_eq!(codes(&first), [WarningCode::LaterBox, WarningCode::LaterBoxLarge], "{rect}: {first:?}");
    }
}

#[test]
fn forged_an_empty_field_that_is_an_annotation() {
    // A "signature field" planted before signing whose field dictionary is a visible comment: signing
    // it would hide (or redraw) the comment.
    let mut d = PdfDocument::from_bytes(&page1()).unwrap();
    let page = d.find_page(0).unwrap();
    let note = d.new_object_from_str("<</Type/Annot/Subtype/FreeText/T(Landlord)/Rect[72 300 300 340]/Contents(Rent: 1000)/DA(/Helv 12 Tf 0 g)>>").unwrap();
    let note = d.add_object(&note).unwrap();
    push_annot(&mut d, page.clone(), note.clone());
    let mut w = d.new_object_from_str("<</Type/Annot/Subtype/Widget/FT/Sig/F 132/Rect[0 0 0 0]>>").unwrap();
    w.dict_put("Parent", note.clone()).unwrap();
    let w = d.add_object(&w).unwrap();
    push_annot(&mut d, page.clone(), w.clone());
    let mut fields = d.new_array().unwrap();
    fields.array_push(w).unwrap();
    let mut form = d.new_dict().unwrap();
    form.dict_put("Fields", fields).unwrap();
    d.catalog().unwrap().dict_put("AcroForm", form).unwrap();
    let v = victim(&write(&mut d));
    let forged = attack(&v, &SignRequest { field: Some("Landlord".into()), ..unseen() }, |_, w| {
        let mut parent = w.get_dict("Parent").unwrap().unwrap();
        parent.dict_put("F", PdfObject::new_int(2).unwrap()).unwrap();
    });
    assert_caught(&forged);
}

#[test]
fn forged_a_field_that_is_not_a_widget_in_annots() {
    // The signature's field split off as its own object, and that object (not a widget) added to
    // the page's annotations.
    let v = victim(&page1());
    let forged = attack(&v, &unseen(), |d, w| {
        let mut field = d.new_object_from_str("<</FT/Sig/T(Split)>>").unwrap();
        field.dict_put("V", w.get_dict("V").unwrap().unwrap()).unwrap();
        let field = d.add_object(&field).unwrap();
        w.dict_delete("T").unwrap();
        w.dict_put("Parent", field.clone()).unwrap();
        let page = d.find_page(0).unwrap();
        push_annot(d, page, field);
    });
    assert_caught(&forged);
}

#[test]
fn forged_a_widget_with_an_action() {
    let v = victim(&page1());
    for key in ["AA", "A"] {
        let forged = attack(&v, &unseen(), |d, w| {
            let act = d.new_object_from_str("<</S/JavaScript/JS(app.alert(1))>>").unwrap();
            let act = if key == "AA" {
                let mut aa = d.new_dict().unwrap();
                aa.dict_put("PO", act).unwrap();
                aa
            } else {
                act
            };
            w.dict_put(key, act).unwrap();
        });
        assert_caught(&forged);
    }
}

#[test]
fn a_signature_shown_by_a_field_added_later_is_invalid() {
    // The victim signs page 2; a field on page 1 (first in page order) then shows the victim's
    // signature dictionary, or a copy of its byte range and value with other details.
    let doc = fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() });
    let v = pdf::sign(&doc, &ids().0, &req(1, [72.0, 600.0, 280.0, 680.0])).unwrap();
    for copy in [false, true] {
        let forged = append(&v, |d| {
            let form = d.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap();
            let mut fields = form.get_dict("Fields").unwrap().unwrap();
            let theirs = fields.get_array(0).unwrap().unwrap().get_dict("V").unwrap().unwrap();
            let value = if copy {
                let mut c = theirs.copy_dict().unwrap();
                c.dict_put("Reason", PdfObject::new_string("I agree to pay 9000").unwrap()).unwrap();
                d.add_object(&c).unwrap()
            } else {
                theirs
            };
            let page = d.find_page(0).unwrap();
            let mut w = d.new_object_from_str("<</Type/Annot/Subtype/Widget/FT/Sig/T(Forged)/F 132/Rect[72 72 280 150]>>").unwrap();
            w.dict_put("V", value).unwrap();
            w.dict_put("P", page.clone()).unwrap();
            let w = d.add_object(&w).unwrap();
            push_annot(d, page, w.clone());
            fields.array_push(w).unwrap();
        });
        let i = infos(&forged);
        let fake = i.iter().find(|s| s.field == "Forged").unwrap();
        let real = i.iter().find(|s| s.field != "Forged").unwrap();
        assert_eq!((fake.status, fake.intact), (Status::Invalid, false), "copy={copy}: {fake:?}");
        assert_eq!((real.status, real.intact, real.changes), (Status::Warning, true, Changes::Changed), "copy={copy}: {real:?}");
    }
}

#[test]
fn an_untrusted_signer_is_never_called_valid() {
    // Self-signed and hashed with SHA-1, then commented on: intact, with two cautions, but the
    // signer comes first.
    let bytes = with_comment(&recms(&signed("untrusted", None), |data| by_hand(data, DigestAlg::Sha1, true)));
    let d = PdfDocument::from_bytes(&bytes).unwrap();
    let s = &pdf::list(&d, &bytes, &|_: &Certificate, _: &[Certificate]| (false, "self-signed".into())).unwrap()[0];
    assert_eq!((s.status, s.intact, s.trusted), (Status::UnknownSigner, true, false), "{s:?}");
    assert_eq!(codes(s), [WarningCode::Changed, WarningCode::Sha1], "{s:?}");
    assert!(s.summary.contains("not trusted") && !s.summary.contains("valid"), "{s:?}");
}

/// MuPDF never writes an object stream into an update, so the attack writes `/Type/ObjStn` and
/// renames it in the bytes, then signs those again as the attacker.
fn with_objstm(v: &[u8], tamper: impl FnMut(&mut PdfDocument, &mut PdfObject)) -> Vec<u8> {
    let mut out = attack(v, &unseen(), tamper);
    let at = v.len() + out[v.len()..].windows(6).position(|w| w == b"ObjStn").unwrap();
    out[at..at + 6].copy_from_slice(b"ObjStm");
    recms(&out, |data| {
        let b = &ids().1;
        let alg = DigestAlg::Sha256;
        cms::build(&b.key, &b.cert, &[], alg, cms::DATA, None, &alg.digest(&[data]), &b.cert).unwrap()
    })
}

fn objstm(d: &mut PdfDocument) -> PdfObject {
    let dict = d.new_object_from_str("<</Type/ObjStn/N 0/First 0>>").unwrap();
    d.add_stream(&mupdf::Buffer::from_bytes(b"").unwrap(), Some(&dict), false).unwrap()
}

#[test]
fn an_object_stream_nothing_uses_is_structure_only() {
    let v = victim(&page1());
    let (first, second) = verdicts(&with_objstm(&v, |d, _| {
        objstm(d);
    }));
    assert_eq!(second.status, Status::Valid, "{second:?}");
    assert_eq!((first.status, first.changes), (Status::Valid, Changes::SignaturesOnly), "{first:?}");
    // Named by anything (here an allowed key of the new widget), it is no longer just structure.
    let forged = with_objstm(&v, |d, w| {
        let s = objstm(d);
        w.dict_put("TU", s).unwrap();
    });
    assert_caught(&forged);
}

#[test]
fn a_signed_field_without_sigflags_still_counts_as_signed() {
    // Signed, then /SigFlags dropped in a later update, then the file damaged so MuPDF repairs it:
    // still a signed file, so it is never rewritten.
    let bytes = append(&signed("noflags-src", None), |d| {
        d.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap().dict_delete("SigFlags").unwrap();
    });
    let mut bytes = bytes;
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
    let digit = at + bytes[at..].iter().position(u8::is_ascii_digit).unwrap();
    bytes[digit] = if bytes[digit] == b'1' { b'2' } else { b'1' };
    let (eng, id, _) = open_bytes("noflags", &bytes);
    assert_eq!(eng.current_bytes(id).unwrap(), bytes, "opened as it is, not rewritten");
}

// ---- third review: references planted in advance, lazy repair, deep chains, limits --------------

/// A small box, drawn hidden by the attacker so no "later box" warning shows.
fn hidden_box() -> SignRequest {
    req(0, [300.0, 100.0, 320.0, 110.0])
}

/// Names `x` in the resources of the widget's appearance, as an image would be.
fn name_in_ap(d: &mut PdfDocument, w: &PdfObject, x: PdfObject) {
    let mut n = w.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
    let mut xo = d.new_dict().unwrap();
    xo.dict_put("Z", x).unwrap();
    match n.get_dict("Resources").unwrap() {
        Some(mut r) => r.dict_put("XObject", xo).unwrap(),
        None => {
            let mut r = d.new_dict().unwrap();
            r.dict_put("XObject", xo).unwrap();
            n.dict_put("Resources", r).unwrap();
        }
    }
}

/// A blue sheet over the page, as a new stream object; returns it.
fn blue_sheet(d: &mut PdfDocument) -> PdfObject {
    d.add_stream(&mupdf::Buffer::from_bytes(b"0 0 1 rg 0 0 612 792 re f").unwrap(), None, false).unwrap()
}

/// The second test ID's signature redone over `bytes` (edited after signing).
fn resign(bytes: &[u8]) -> Vec<u8> {
    recms(bytes, |data| {
        let b = &ids().1;
        let alg = DigestAlg::Sha256;
        cms::build(&b.key, &b.cert, &[], alg, cms::DATA, None, &alg.digest(&[data]), &b.cert).unwrap()
    })
}

fn page_contents(d: &PdfDocument) -> Vec<u8> {
    let c = d.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
    let c = if c.is_array().unwrap() { c.get_array(c.len().unwrap() as i32 - 1).unwrap().unwrap() } else { c };
    c.read_stream().unwrap_or_default()
}

#[test]
fn forged_a_reference_planted_before_signing() {
    // The page's contents name an object that doesn't exist when the victim signs; the attacker's
    // signature revision defines it (hidden, reached through its appearance's resources).
    let num = std::cell::Cell::new(0);
    let mut tamper = |d: &mut PdfDocument, w: &mut PdfObject| {
        let s = blue_sheet(d);
        num.set(s.as_indirect().unwrap());
        w.dict_put("F", PdfObject::new_int(2).unwrap()).unwrap();
        name_in_ap(d, w, s);
    };
    // A dry run tells which number the attacker's object will get.
    attack(&victim(&page1()), &hidden_box(), &mut tamper);
    let n = num.get();
    let base = append(&page1(), |d| {
        let mut page = d.find_page(0).unwrap();
        let c = page.get_dict("Contents").unwrap().unwrap().as_indirect().unwrap();
        page.dict_put("Contents", d.new_object_from_str(&format!("[{c} 0 R {n} 0 R]")).unwrap()).unwrap();
    });
    let v = victim(&base);
    assert!(page_contents(&PdfDocument::from_bytes(&v).unwrap()).is_empty(), "signed with nothing there");
    let forged = attack(&v, &hidden_box(), &mut tamper);
    assert_eq!(num.get(), n);
    assert!(page_contents(&PdfDocument::from_bytes(&forged).unwrap()).starts_with(b"0 0 1 rg"), "the forged file draws the sheet");
    assert_caught(&forged);
}

#[test]
fn forged_an_object_shown_only_after_mupdf_repairs_the_file() {
    // The attacker's new object (an image's soft mask, which nothing reads before the comparison)
    // is renamed to the page content's number: its cross-reference entry then points at the wrong
    // object, MuPDF rebuilds the table when it first reads it, and the rebuilt table takes this
    // later copy as the page content, after the real one was compared.
    let v = victim(&page1());
    let c = PdfDocument::from_bytes(&v).unwrap().find_page(0).unwrap().get_dict("Contents").unwrap().unwrap().as_indirect().unwrap();
    let num = std::cell::Cell::new(0);
    let mut out = attack(&v, &hidden_box(), |d, w| {
        let s = blue_sheet(d);
        num.set(s.as_indirect().unwrap());
        let mut img = d.new_object_from_str("<</Type/XObject/Subtype/Image/Width 1/Height 1/ColorSpace/DeviceRGB/BitsPerComponent 8>>").unwrap();
        img.dict_put("SMask", s).unwrap();
        let img = d.add_stream(&mupdf::Buffer::from_bytes(&[0, 0, 0]).unwrap(), Some(&img), false).unwrap();
        w.dict_put("F", PdfObject::new_int(2).unwrap()).unwrap();
        name_in_ap(d, w, img);
    });
    let n = num.get();
    assert!(n > c);
    let head = format!("
{n} 0 obj");
    let at = v.len() + out[v.len()..].windows(head.len()).position(|w| w == head.as_bytes()).unwrap();
    let renamed = format!("
{c:>w$} 0 obj", w = n.to_string().len());
    out[at..at + head.len()].copy_from_slice(renamed.as_bytes());
    let forged = resign(&out);
    let d = PdfDocument::from_bytes(&forged).unwrap();
    let _ = d.new_indirect(n, 0).unwrap().resolve();
    assert!(page_contents(&d).starts_with(b"0 0 1 rg"), "once repaired, the forged file draws the sheet");
    let (victim, _) = verdicts(&forged);
    assert!(victim.status != Status::Valid && (victim.changes == Changes::Changed || victim.status == Status::Invalid), "victim: {victim:?}");
}

#[test]
fn a_deep_chain_in_a_signature_does_not_crash() {
    // 6 000 dictionaries, each naming the next, in the signature dictionary: checked on a 2 MB
    // stack (as the app's blocking pool runs it) without overflowing.
    let bytes = attack(&page1(), &unseen(), |d, w| {
        let mut next = PdfObject::new_null();
        for _ in 0..6000 {
            let mut o = d.new_dict().unwrap();
            o.dict_put("X", next).unwrap();
            next = d.add_object(&o).unwrap();
        }
        let field = if w.get_dict("V").unwrap().is_some() { w.clone() } else { w.get_dict("Parent").unwrap().unwrap() };
        field.get_dict("V").unwrap().unwrap().dict_put("X", next).unwrap();
    });
    let i = std::thread::Builder::new().stack_size(2 << 20).spawn(move || infos(&bytes)).unwrap().join().unwrap();
    assert_eq!(i.len(), 1);
    assert_eq!(i[0].status, Status::Invalid, "{:?}", i[0]);
}

#[test]
fn more_signatures_than_inkwell_checks_are_refused() {
    let mut b = page1();
    for _ in 0..65 {
        b = pdf::sign(&b, &ids().1, &unseen()).unwrap();
    }
    let d = PdfDocument::from_bytes(&b).unwrap();
    let e = pdf::list(&d, &b, &trust_tests).unwrap_err();
    assert!(e.to_string().contains("more than 64 signatures"), "{e}");
}

#[test]
fn a_hybrid_cross_reference_update_is_a_change() {
    // The attacker's trailer names a second cross-reference table (/XRefStm, here the victim's
    // own): readers that take one or the other may see different files. MuPDF doesn't write the
    // key, so it goes in by hand, and the signature's byte range grows to cover it.
    let v = victim(&page1());
    let at = v.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
    let prev: String = v[at..].iter().skip_while(|c| c.is_ascii_whitespace()).take_while(|c| c.is_ascii_digit()).map(|&c| c as char).collect();
    let out = attack(&v, &unseen(), |_, _| {});
    let t = out.windows(7).rposition(|w| w == b"trailer").unwrap();
    let open = t + out[t..].windows(2).position(|w| w == b"<<").unwrap() + 2;
    let key = format!("/XRefStm {prev}");
    let mut out = [&out[..open], key.as_bytes(), &out[open..]].concat();
    let br = out.windows(11).rposition(|w| w == b"/ByteRange[").unwrap() + 11;
    let close = br + out[br..].iter().position(|&c| c == b']').unwrap();
    let n: Vec<usize> = std::str::from_utf8(&out[br..close]).unwrap().split_whitespace().map(|x| x.parse().unwrap()).collect();
    let mut range = format!("0 {} {} {}", n[1], n[2], n[3] + key.len()).into_bytes();
    assert!(range.len() <= close - br);
    range.resize(close - br, b' ');
    out[br..close].copy_from_slice(&range);
    let (first, second) = verdicts(&resign(&out));
    assert_eq!(second.status, Status::Valid, "{second:?}");
    assert_eq!((first.status, first.changes), (Status::Warning, Changes::Changed), "{first:?}");
    assert!(codes(&first).contains(&WarningCode::HybridXref), "{first:?}");
}
