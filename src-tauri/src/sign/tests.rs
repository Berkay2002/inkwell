//! Signing through the edit lane and validating the result: incremental updates that keep earlier
//! signatures valid, DocMDP certification, tampering, empty fields, appearance pictures, and the
//! Windows CNG path (an ephemeral PFX key, never the user's certificate store).
//! Every ID here is a self-signed test ID made in the test.

use super::app::{list, sign_step, system_trust};
use super::crypto::cms::SignedData;
use super::crypto::{Certificate, DigitalId};
use super::pdf::{self, Changes, SignDate, SignRequest, SignatureInfo, Status};
use crate::annot::{NewAnnot, Style, add_one};
use crate::engine::edit::{EditOp, EditSource};
use crate::engine::fixture::{self, Fixture};
use crate::engine::{DocId, Engine};
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
    assert_eq!((first.status, first.changes), (Status::Valid, Changes::SignaturesOnly), "{first:?}");
    assert_eq!((second.status, second.changes), (Status::Valid, Changes::None), "{second:?}");

    // A comment after signing: both stay intact, both say the document changed.
    add_one(&eng, id, 1, &NewAnnot::Note { at: [100.0, 100.0], text: "after".into() }, &Style::default(), "T", EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    for s in infos(&std::fs::read(&path).unwrap()) {
        assert_eq!((s.status, s.intact, s.changes), (Status::Valid, true, Changes::Changed), "{s:?}");
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
