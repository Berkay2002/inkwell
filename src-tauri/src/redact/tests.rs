//! Redaction tests on the study sample (`docs/study/samples/redact-source.pdf`, loaded from memory,
//! never written) and on adversarial fixtures. Outputs land in `target/redact-out/` for pdfinspect.

use super::*;
use mupdf::pdf::PdfDocument;

const SAMPLE: &[u8] = include_bytes!("../../../docs/study/samples/redact-source.pdf");

fn sample() -> PdfDocument {
    PdfDocument::from_bytes(SAMPLE).unwrap()
}

fn out(name: &str, bytes: &[u8]) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/redact-out");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
}

/// Builds a PDF from numbered object bodies (object n = bodies[n-1]).
fn pdf(bodies: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut v = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offs = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offs.push(v.len());
        v.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        v.extend_from_slice(b);
        v.extend_from_slice(b"\nendobj\n");
    }
    let xref = v.len();
    v.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offs {
        v.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    v.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1)
            .as_bytes(),
    );
    v
}

fn obj(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

fn page_text(doc: &Document) -> String {
    (0..doc.page_count().unwrap()).map(|p| index_page(&doc.load_page(p).unwrap()).unwrap().text).collect::<Vec<_>>().join("\n")
}

/// Every place a string can hide: page text, object strings, decoded streams, raw bytes.
fn leaks(bytes: &[u8], secrets: &[&str]) -> Vec<Leak> {
    let t = Targets { pages: Vec::new(), needles: secrets.iter().map(|s| s.to_string()).collect() };
    verify_bytes(bytes, &t).unwrap().leaks
}

fn hits(doc: &Document, p: Pattern) -> Vec<String> {
    find(doc, &Query::Pattern(p)).unwrap().into_iter().map(|h| h.text).collect()
}

#[test]
fn find_patterns_on_sample() {
    let doc = sample();
    let mut ssn = hits(&doc, Pattern::Ssn);
    ssn.sort();
    assert_eq!(ssn, ["123-45-6789", "123-45-6789", "123-45-6789", "222-33-4444", "987-65-4321"]);
    assert_eq!(hits(&doc, Pattern::Card), ["4111 1111 1111 1111"]);
    let mut mail = hits(&doc, Pattern::Email);
    mail.sort();
    assert_eq!(mail, ["jane.tester@example.com", "jane.tester@example.com", "legal@example.com"]);
    let phone = hits(&doc, Pattern::Phone);
    assert!(phone.contains(&"(555) 010-4477".to_string()), "{phone:?}");
    assert!(!phone.iter().any(|p| p.contains("2026") || p.contains("1990")), "dates are not phones: {phone:?}");

    let q = |needle: &str, cs, ww| Query::Text { needle: needle.into(), case_sensitive: cs, whole_word: ww };
    assert_eq!(find(&doc, &q("blueheron", false, true)).unwrap().len(), 3);
    assert_eq!(find(&doc, &q("blueheron", true, true)).unwrap().len(), 0);
    assert_eq!(find(&doc, &q("Tester", false, true)).unwrap().len(), 5); // incl. jane.tester@
    assert_eq!(find(&doc, &q("Test", false, true)).unwrap().len(), 0);
    let re = Query::Regex { pattern: r"ACCT-\d{4}-[A-Z]{2}".into(), case_sensitive: true };
    assert_eq!(find(&doc, &re).unwrap()[0].text, "ACCT-0042-ZX");
}

#[test]
fn find_across_lines_and_hyphenation() {
    let content = b"BT /F1 12 Tf 72 700 Td (The account is confi-) Tj ET\n\
BT /F1 12 Tf 72 686 Td (dential: IBAN SE45 5000 0000 0583 9825 7466, pnr 811218-9876, ssn 123-45-) Tj ET\n\
BT /F1 12 Tf 72 672 Td (6789 and the end of the line.) Tj ET";
    let bytes = pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R >>"),
            obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
            obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>"),
            stream("", content),
        ],
        "",
    );
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    let h = find(&doc, &Query::Text { needle: "confidential".into(), case_sensitive: false, whole_word: true }).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].quads.len(), 2, "one quad per line, hyphen included");
    let h = find(&doc, &Query::Text { needle: "the account is".into(), case_sensitive: false, whole_word: false }).unwrap();
    assert_eq!(h.len(), 1);
    let h = find(&doc, &Query::Pattern(Pattern::Ssn)).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].text, "123-45-6789");
    assert_eq!(h[0].quads.len(), 2);
    assert_eq!(hits(&doc, Pattern::Iban), ["SE45 5000 0000 0583 9825 7466"]);
    assert_eq!(hits(&doc, Pattern::Personnummer), ["811218-9876"]);

    // Marking the split hit and applying removes both halves.
    let mut doc = doc;
    mark_hits(&mut doc, &h, &MarkStyle::default()).unwrap();
    apply(&mut doc, &ApplyOptions::default()).unwrap();
    let text = page_text(&doc);
    assert!(!text.contains("123-45") && !text.contains("6789"), "{text}");
    assert!(text.contains("and the end of the line"));
}

#[test]
fn marks_roundtrip() {
    let mut doc = sample();
    let hits = find(&doc, &Query::Pattern(Pattern::Email)).unwrap();
    let style = MarkStyle { fill: [1.0, 0.0, 0.0], overlay_text: Some("(b)(6)".into()) };
    let ids = mark_hits(&mut doc, &hits, &style).unwrap();
    let page_mark = mark_page(&mut doc, 2, &MarkStyle::default()).unwrap();
    let marks = list_marks(&doc).unwrap();
    assert_eq!(marks.len(), 4);
    assert!(marks.iter().filter(|m| m.xref != page_mark).all(|m| m.style == style));
    assert!(remove_mark(&mut doc, marks[0].page, ids[0]).unwrap());
    assert!(!remove_mark(&mut doc, marks[0].page, ids[0]).unwrap());
    assert_eq!(list_marks(&doc).unwrap().len(), 3);
    // Nothing has changed in the content yet.
    assert!(page_text(&doc).contains("jane.tester@example.com"));
}

/// Acrobat run B (out-02): SSN pattern + "redact pages" on page 3, sanitize on.
#[test]
fn apply_sample_with_sanitize() {
    let mut doc = sample();
    let hits = find(&doc, &Query::Pattern(Pattern::Ssn)).unwrap();
    mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    mark_page(&mut doc, 2, &MarkStyle::default()).unwrap();
    let rep = apply(&mut doc, &ApplyOptions::default()).unwrap();
    assert_eq!(rep.marks, 6);
    assert!(rep.verify.is_clean());

    let text = page_text(&doc);
    for s in ["123-45-6789", "987-65-4321", "222-33-4444", "Final page", "1990-04-12"] {
        assert!(!text.contains(s), "{s} still in text");
    }
    assert_eq!(text.matches("BLUEHERON").count(), 2, "unmarked text stays");
    assert!(text.contains("jane.tester@example.com"));
    assert!(list_marks(&doc).unwrap().is_empty());

    let s = rep.sanitize;
    assert_eq!(s.metadata, 1, "docinfo (the sample has no XMP)");
    assert_eq!(s.hidden_layer_blocks, 1);
    assert_eq!(s.embedded_files, 2, "name tree entry + catalog /AF (Acrobat keeps /AF)");
    assert_eq!(s.bookmarks, 3);
    assert_eq!((s.annotations, s.scripts_actions), (1, 3), "sticky note; OpenAction + 2 links");

    let bytes = save_to_vec(&doc).unwrap();
    out("red-inkwell-02-apply-sanitize.pdf", &bytes);
    let secrets = [
        "123-45-6789",
        "987-65-4321",
        "222-33-4444",
        "HIDDEN-LAYER-SECRET",
        "555-66-7777",
        "Attachment body",
        "secret-notes",
        "555-010-4477",
        "Jane Q. Tester signed", // page 1 text stays...
    ];
    let l = leaks(&bytes, &secrets[..8]);
    assert!(l.is_empty(), "{l:?}");
    let re = PdfDocument::from_bytes(&bytes).unwrap();
    assert!(page_text(&re).contains(secrets[8]));
    let cat = re.catalog().unwrap();
    for k in ["AF", "OCProperties", "OpenAction", "Outlines", "Metadata"] {
        assert!(cat.get_dict(k).unwrap().is_none(), "catalog /{k} left");
    }
    let names = cat.get_dict("Names").unwrap().unwrap(); // named destinations stay
    assert!(names.get_dict("EmbeddedFiles").unwrap().is_none() && names.get_dict("JavaScript").unwrap().is_none());
    assert!(re.trailer().unwrap().get_dict("Info").unwrap().is_none());
    for p in 0..3 {
        assert!(annots_of(&re.find_page(p).unwrap()).unwrap().is_empty(), "page {p} still has annotations (incl. orphan popups)");
    }
}

/// Acrobat run A (out-01): every BLUEHERON marked, sanitize off: only the text goes.
#[test]
fn apply_sample_without_sanitize() {
    let mut doc = sample();
    let hits = find(&doc, &Query::Text { needle: "BLUEHERON".into(), case_sensitive: true, whole_word: true }).unwrap();
    mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    let rep = apply(&mut doc, &ApplyOptions { sanitize: Sanitize::NONE, ..Default::default() }).unwrap();
    assert_eq!(rep.sanitize, SanitizeReport::default());
    assert!(rep.verify.leaks.is_empty(), "{:?}", rep.verify);
    let bytes = save_to_vec(&doc).unwrap();
    out("red-inkwell-01-apply-nosanitize.pdf", &bytes);
    assert!(leaks(&bytes, &["BLUEHERON"]).is_empty());
    let re = PdfDocument::from_bytes(&bytes).unwrap();
    assert!(re.catalog().unwrap().get_dict("OCProperties").unwrap().is_some());
    assert_eq!(re.embedded_files().unwrap().len(), 1);
    assert!(page_text(&re).contains("123-45-6789"));
}

/// Marking one of three occurrences: apply succeeds and reports the other two as unmarked.
#[test]
fn unmarked_occurrences_are_reported_not_fatal() {
    let mut doc = sample();
    let hits = find(&doc, &Query::Text { needle: "BLUEHERON".into(), case_sensitive: true, whole_word: true }).unwrap();
    mark_hits(&mut doc, &hits[..1], &MarkStyle::default()).unwrap();
    let rep = apply(&mut doc, &ApplyOptions::default()).unwrap();
    assert!(rep.verify.is_clean());
    assert!(rep.verify.leaks.iter().any(|l| l.needle == "BLUEHERON" && l.place.contains("unmarked")), "{:?}", rep.verify);
    assert_eq!(page_text(&doc).matches("BLUEHERON").count(), 2);
}

/// Acrobat's sanitize-only run (out-03), without its gaps.
#[test]
fn sanitize_only_sample() {
    let mut doc = sample();
    let before = page_text(&doc);
    let rep = sanitize(&mut doc, &Sanitize::default(), &[]).unwrap();
    assert_eq!(rep.hidden_layer_blocks, 1);
    assert_eq!(rep.embedded_files, 2);
    assert_eq!(rep.bookmarks, 3);
    assert_eq!(rep.hidden_text_pages, 0);
    let bytes = save_to_vec(&doc).unwrap();
    out("red-inkwell-03-sanitize-only.pdf", &bytes);
    let l = leaks(&bytes, &["HIDDEN-LAYER-SECRET", "Attachment body", "555-010-4477", "confidential, SSN", "Jane Q. Tester\""]);
    assert!(l.is_empty(), "{l:?}");
    assert_eq!(page_text(&PdfDocument::from_bytes(&bytes).unwrap()), before);
}

/// A redaction that leaves text behind must be caught.
#[test]
fn verify_catches_incomplete_redaction() {
    // Glyphs not removed (text method None): the text is still under the mark.
    let mut doc = sample();
    let hits = find(&doc, &Query::Pattern(Pattern::Ssn)).unwrap();
    mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    let t = collect_targets(&doc, true).unwrap();
    assert!(t.needles.contains(&"123-45-6789".to_string()));
    for pm in &t.pages {
        let opts = PdfRedactOptions { text: PdfRedactTextMethod::None, ..PdfRedactOptions::mupdf_default() };
        doc.load_pdf_page(pm.page).unwrap().redact_with_options(opts).unwrap();
    }
    let rep = verify_bytes(&save_to_vec(&doc).unwrap(), &t).unwrap();
    assert!(!rep.is_clean());
    assert!(rep.leaks.iter().any(|l| l.critical && l.place.contains("under a mark")), "{rep:?}");

    // Text removed, but the attachment still quotes the SSN and sanitize is off: apply refuses.
    let mut doc = sample();
    mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    match apply(&mut doc, &ApplyOptions { sanitize: Sanitize::NONE, ..Default::default() }) {
        Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.needle == "123-45-6789" && l.place.contains("attachment")), "{l:?}"),
        r => panic!("expected a leak, got {r:?}"),
    }
}

fn adversarial() -> Vec<u8> {
    let content = b"BT /F1 12 Tf 72 700 Td (VISIBLE-SECRET) Tj ET\n\
BT /F1 12 Tf 300 700 Td (KEEP-ME) Tj ET\n\
BT 3 Tr /F1 12 Tf 72 650 Td (INVISIBLE-SECRET) Tj ET\n\
q 1 0 0 1 72 600 cm /Fm1 Do Q\n\
BT /T3 12 Tf 72 550 Td (SECRET) Tj ET\n\
q 100 0 0 100 200 400 cm /Im1 Do Q\n\
0 0 1 rg 72 450 50 20 re f\n\
0 1 0 rg 300 450 50 20 re f";
    let red: Vec<u8> = [255u8, 0, 0].repeat(64);
    pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R /Outlines 15 0 R /AcroForm << /Fields [13 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>"),
            obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
            obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R /T3 7 0 R >> /XObject << /Fm1 6 0 R /Im1 9 0 R >> >> /Contents 5 0 R /Annots [10 0 R 12 0 R 13 0 R] >>"),
            stream("", content),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /Font << /F1 3 0 R >> >>", b"BT /F1 12 Tf 0 10 Td (FORM-SECRET) Tj ET"),
            obj("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /C 8 0 R /E 8 0 R /R 8 0 R /S 8 0 R /T 8 0 R >> /Encoding << /Type /Encoding /Differences [67 /C 69 /E 82 /R /S /T] >> /FirstChar 67 /LastChar 84 /Widths [600 0 600 0 0 0 0 0 0 0 0 0 0 0 0 600 600 600] /Resources << >> >>"),
            stream("", b"600 0 0 0 500 700 d1 0 0 500 700 re f"),
            stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8", &red),
            obj("<< /Type /Annot /Subtype /Stamp /Rect [72 350 272 380] /Contents (ANNOT-SECRET) /AP << /N 11 0 R >> >>"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 200 30] /Resources << /Font << /F1 3 0 R >> >>", b"BT /F1 12 Tf 5 10 Td (ANNOT-SECRET) Tj ET"),
            obj("<< /Type /Annot /Subtype /Text /Rect [500 100 520 120] /Contents (see VISIBLE-SECRET) >>"),
            obj("<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (FIELD-SECRET) /Rect [72 300 272 320] /DA (/Helv 10 Tf 0 g) /AP << /N 14 0 R >> /P 4 0 R >>"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 200 20] /Resources << /Font << /Helv 3 0 R >> >>", b"/Tx BMC BT /Helv 10 Tf 2 5 Td (FIELD-SECRET) Tj ET EMC"),
            obj("<< /Type /Outlines /First 16 0 R /Last 16 0 R /Count 1 >>"),
            obj("<< /Title (VISIBLE-SECRET section) /Parent 15 0 R /Dest [4 0 R /XYZ 0 792 0] >>"),
            obj("<< /Title (VISIBLE-SECRET report) /Author (Tester) >>"),
        ],
        "/Info 17 0 R",
    )
}

/// Text in a form XObject, a Type3 font, invisible text, an annotation appearance stream, a form
/// field, a partly covered image and covered line art; quotes of the text in a comment, a bookmark
/// and docinfo. All sanitize categories off except form fields: the security path must not
/// depend on them.
#[test]
fn adversarial_redaction() {
    let mut doc = PdfDocument::from_bytes(&adversarial()).unwrap();
    let before = page_text(&doc);
    for s in ["VISIBLE-SECRET", "INVISIBLE-SECRET", "FORM-SECRET", "SECRET"] {
        assert!(before.contains(s), "fixture: {s} not extractable in {before}");
    }
    let hits = find(&doc, &Query::Text { needle: "VISIBLE-SECRET".into(), case_sensitive: true, whole_word: true }).unwrap();
    assert_eq!(hits.len(), 1);
    mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    // PDF y 280..670 x 60..280 in page space (y down).
    let area = Rect { x0: 60.0, y0: 792.0 - 670.0, x1: 280.0, y1: 792.0 - 280.0 };
    add_area_mark(&mut doc, 0, area, &MarkStyle { fill: [0.2, 0.2, 0.2], overlay_text: Some("REDACTED".into()) }).unwrap();

    let opts = ApplyOptions { sanitize: Sanitize { form_fields_under_marks: true, ..Sanitize::NONE }, ..Default::default() };
    let rep = apply(&mut doc, &opts).unwrap();
    assert_eq!(rep.sanitize.form_fields, 1);
    assert!(rep.sanitize.annotations >= 2, "stamp under the mark + comment quoting the text: {:?}", rep.sanitize);
    assert_eq!(rep.sanitize.bookmarks, 1);
    assert_eq!(rep.sanitize.metadata, 1);

    let bytes = save_to_vec(&doc).unwrap();
    out("red-inkwell-adversarial.pdf", &bytes);
    let secrets = ["VISIBLE-SECRET", "INVISIBLE-SECRET", "FORM-SECRET", "ANNOT-SECRET", "FIELD-SECRET"];
    let l = leaks(&bytes, &secrets);
    assert!(l.is_empty(), "{l:?}");
    let re = PdfDocument::from_bytes(&bytes).unwrap();
    let text = page_text(&re);
    assert!(!text.contains("SECRET"), "Type3 text left: {text}");
    assert!(text.contains("KEEP-ME") && text.contains("REDACTED"), "{text}");
    assert!(re.trailer().unwrap().get_dict("Info").unwrap().unwrap().get_dict("Author").unwrap().is_some());
    let af = re.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap();
    assert_eq!(af.get_dict("Fields").unwrap().unwrap().len().unwrap(), 0);

    // Image: the covered columns (x 200..280 of 200..300) are blanked in the image data itself.
    let page = re.find_page(0).unwrap();
    let xo = page.get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap();
    let mut images = 0;
    for i in 0..xo.dict_len().unwrap() as i32 {
        let im = xo.get_dict_val(i).unwrap().unwrap();
        if !sanitize::is_image(&im).unwrap() {
            continue;
        }
        images += 1;
        let (w, h) = (im.get_dict("Width").unwrap().unwrap().as_int().unwrap() as usize, im.get_dict("Height").unwrap().unwrap().as_int().unwrap() as usize);
        let data = re.xref_stream(im.as_indirect().unwrap()).unwrap();
        let n = data.len() / (w * h);
        assert!(n >= 3, "{} bytes for {w}x{h}", data.len());
        let px = |x: usize, y: usize| &data[(y * w + x) * n..(y * w + x) * n + 3];
        for y in 0..h {
            assert_ne!(px(0, y), [255, 0, 0], "covered pixel kept at row {y}");
            assert_eq!(px(w - 1, y), [255, 0, 0], "uncovered pixel changed at row {y}");
        }
    }
    assert_eq!(images, 1);

    // Line art: the blue box under the mark is gone, the green one outside stays.
    let content = String::from_utf8_lossy(&re.load_pdf_page(0).unwrap().contents().unwrap().map(|c| {
        if c.is_array().unwrap() {
            (0..c.len().unwrap() as i32).flat_map(|i| c.get_array(i).unwrap().unwrap().read_stream().unwrap()).collect()
        } else {
            c.read_stream().unwrap()
        }
    }).unwrap()).into_owned();
    assert!(!content.contains("0 0 1 rg"), "covered line art kept");
    assert!(content.contains("0 1 0 rg"), "uncovered line art removed");
    assert!(content.contains("0.200 0.200 0.200 rg"), "overlay fill missing");
}

#[test]
fn hidden_text_sanitize() {
    let mut doc = PdfDocument::from_bytes(&adversarial()).unwrap();
    let rep = sanitize(&mut doc, &Sanitize { hidden_text: true, ..Sanitize::NONE }, &[]).unwrap();
    assert_eq!(rep.hidden_text_pages, 1);
    let text = page_text(&doc);
    assert!(!text.contains("INVISIBLE-SECRET") && text.contains("VISIBLE-SECRET"), "{text}");
}

/// A fully covered tiling pattern is no longer drawn; its resource must not survive either.
#[test]
fn covered_pattern_text_is_gone() {
    let content = b"/Pattern cs /P1 scn 72 600 300 100 re f";
    let bytes = pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R >>"),
            obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
            obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Pattern << /P1 6 0 R >> >> /Contents 5 0 R >>"),
            stream("", content),
            stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 100] /XStep 300 /YStep 100 /Matrix [1 0 0 1 72 600] /Resources << /Font << /F1 3 0 R >> >>", b"BT /F1 12 Tf 10 40 Td (PATTERN-SECRET) Tj ET"),
        ],
        "",
    );
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    assert_eq!(page_text(&doc), "PATTERN-SECRET");
    add_area_mark(&mut doc, 0, Rect { x0: 60.0, y0: 792.0 - 710.0, x1: 400.0, y1: 792.0 - 590.0 }, &MarkStyle::default()).unwrap();
    apply(&mut doc, &ApplyOptions::default()).unwrap();
    assert!(leaks(&save_to_vec(&doc).unwrap(), &["PATTERN-SECRET"]).is_empty());
}

/// Hidden-layer content in every place sanitize must look, including constructs it can't fully
/// evaluate (those fail closed).
#[test]
fn hidden_layers_fail_closed() {
    let t = |y: u32, s: &str| format!("BT /F1 12 Tf 72 {y} Td ({s}) Tj ET");
    let content = format!(
        "/OC /L1 BDC {} EMC\n/OC /M1 BDC {} EMC\n/OC /L2 BDC {} EMC\n/OC /L3 BDC {} EMC\n/OC <</Type/OCMD/OCGs 8 0 R>> BDC {} EMC\n/Fm2 Do\n/Pattern cs /P1 scn 72 300 300 100 re f",
        t(700, "OFF-SECRET"), t(680, "VE-SECRET"), t(660, "USAGE-SECRET"), t(640, "ON-TEXT"), t(620, "INLINE-SECRET")
    );
    let bytes = pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R 7 0 R 8 0 R] /D << /OFF [6 0 R] >> >> >>"),
            obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
            obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> /Properties << /L1 6 0 R /L2 7 0 R /L3 8 0 R /M1 9 0 R >> /XObject << /Fm2 10 0 R >> /Pattern << /P1 11 0 R >> >> /Contents 5 0 R /Annots [12 0 R] >>"),
            stream("", content.as_bytes()),
            obj("<< /Type /OCG /Name (off) >>"),
            obj("<< /Type /OCG /Name (usage) /Usage << /View << /ViewState /OFF >> >> >>"),
            obj("<< /Type /OCG /Name (on) >>"),
            obj("<< /Type /OCMD /VE [/Not 8 0 R] >>"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /OC 6 0 R /Resources << /Font << /F1 3 0 R >> >>", t(10, "XOBJ-SECRET").as_bytes()),
            stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 100] /XStep 300 /YStep 100 /Resources << /Font << /F1 3 0 R >> /Properties << /L1 6 0 R >> >>", format!("/OC /L1 BDC {} EMC", t(40, "PATTERN-SECRET")).as_bytes()),
            obj("<< /Type /Annot /Subtype /Square /Rect [72 100 272 130] /AP << /N 13 0 R >> >>"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 200 30] /Resources << /Font << /F1 3 0 R >> /Properties << /L1 6 0 R >> >>", format!("/OC /L1 BDC {} EMC", t(10, "AP-SECRET")).as_bytes()),
        ],
        "",
    );
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    let rep = sanitize(&mut doc, &Sanitize { hidden_layers: true, ..Sanitize::NONE }, &[]).unwrap();
    assert!(rep.hidden_layer_blocks >= 7, "{rep:?}");
    let saved = save_to_vec(&doc).unwrap();
    let secrets = ["OFF-SECRET", "VE-SECRET", "USAGE-SECRET", "INLINE-SECRET", "XOBJ-SECRET", "PATTERN-SECRET", "AP-SECRET"];
    let l = leaks(&saved, &secrets);
    assert!(l.is_empty(), "{l:?}");
    assert!(page_text(&PdfDocument::from_bytes(&saved).unwrap()).contains("ON-TEXT"));
}

/// A /Parent cycle in the field tree must not hang field removal.
#[test]
fn remove_field_survives_parent_cycle() {
    let bytes = pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>"),
            obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R] >>"),
            obj("<< /Type /Annot /Subtype /Widget /FT /Tx /Rect [0 0 10 10] /Parent 5 0 R >>"),
            obj("<< /T (a) /Kids [4 0 R] /Parent 6 0 R >>"),
            obj("<< /T (b) /Kids [5 0 R] /Parent 5 0 R >>"),
        ],
        "",
    );
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    let w = doc.new_indirect(4, 0).unwrap();
    sanitize::remove_field(&doc, &w).unwrap();
}

/// An unmarked visible copy of the text must not excuse a copy in an attachment.
#[test]
fn attachment_copy_stays_critical() {
    let bytes = pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(a.txt) 6 0 R] >> >> >>"),
            obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
            obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>"),
            stream("", b"BT /F1 12 Tf 72 700 Td (CODE-77881) Tj ET BT /F1 12 Tf 72 600 Td (CODE-77881) Tj ET"),
            obj("<< /Type /Filespec /F (a.txt) /EF << /F 7 0 R >> >>"),
            stream("/Type /EmbeddedFile", b"copy of CODE-77881"),
        ],
        "",
    );
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    let hits = find(&doc, &Query::Text { needle: "CODE-77881".into(), case_sensitive: true, whole_word: true }).unwrap();
    assert_eq!(hits.len(), 2);
    mark_hits(&mut doc, &hits[..1], &MarkStyle::default()).unwrap();
    match apply(&mut doc, &ApplyOptions { sanitize: Sanitize::NONE, ..Default::default() }) {
        Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.critical && l.place.contains("attachment")), "{l:?}"),
        r => panic!("expected a leak, got {r:?}"),
    }
}

fn one_page(content: &[u8], page_extra: &str, more: &[Vec<u8>], catalog_extra: &str) -> Vec<u8> {
    let mut bodies = vec![
        obj(&format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>")),
        obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
        obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
        obj(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R {page_extra} >>")),
        stream("", content),
    ];
    bodies.extend_from_slice(more);
    pdf(&bodies, "")
}

/// "Redact pages" must also take content outside the crop box and off the page: viewers hide it,
/// but it is in the file and comes back when the crop box is reset.
#[test]
fn redact_page_takes_content_outside_the_crop_box() {
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (OUTSIDECROP-SECRET) Tj ET BT /F1 12 Tf -400 300 Td (OFFPAGE-SECRET) Tj ET\n\
BT /F1 12 Tf 72 400 Td (ONPAGE-SECRET) Tj ET",
        "/CropBox [0 0 612 500]",
        &[],
        "",
    );
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    mark_page(&mut doc, 0, &MarkStyle::default()).unwrap();
    apply(&mut doc, &ApplyOptions::default()).unwrap();
    let out = save_to_vec(&doc).unwrap();
    let l = leaks(&out, &["OUTSIDECROP-SECRET", "OFFPAGE-SECRET", "ONPAGE-SECRET"]);
    assert!(l.is_empty(), "{l:?}");
}

/// A page thumbnail is a picture of the unredacted page; it must go with the redaction, also
/// with sanitize off.
#[test]
fn redaction_drops_the_page_thumbnail() {
    let thumb = stream("/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &[0]);
    let bytes = one_page(b"BT /F1 12 Tf 72 700 Td (THUMB-SECRET) Tj ET", "/Thumb 6 0 R /PieceInfo << >>", &[thumb], "");
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    let hits = find(&doc, &Query::Text { needle: "THUMB-SECRET".into(), case_sensitive: true, whole_word: false }).unwrap();
    mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    apply(&mut doc, &ApplyOptions { sanitize: Sanitize::NONE, ..Default::default() }).unwrap();
    let re = PdfDocument::from_bytes(&save_to_vec(&doc).unwrap()).unwrap();
    let page = re.find_page(0).unwrap();
    assert!(page.get_dict("Thumb").unwrap().is_none() && page.get_dict("PieceInfo").unwrap().is_none());
}

/// Binary strings (an attachment checksum, a signature's /Contents) hold NUL bytes; scanning
/// them used to fail apply with InvalidUtf8.
#[test]
fn binary_strings_do_not_break_apply() {
    let sig = obj("<< /Type /Sig /Filter /Adobe.PPKLite /Contents <3082000500ff00000000> >>");
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (BINARY-SECRET) Tj ET",
        "/Annots [7 0 R]",
        &[sig, obj("<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig) /V 6 0 R /Rect [300 100 400 150] /P 4 0 R >>")],
        "/AcroForm << /Fields [7 0 R] >>",
    );
    for sanitize in [Sanitize::default(), Sanitize::NONE] {
        let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
        let hits = find(&doc, &Query::Text { needle: "BINARY-SECRET".into(), case_sensitive: true, whole_word: false }).unwrap();
        mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
        apply(&mut doc, &ApplyOptions { sanitize, ..Default::default() }).unwrap();
        assert!(!page_text(&doc).contains("BINARY-SECRET"));
    }
}

/// MuPDF cuts neither soft-mask groups nor tiling patterns: an image inside one, partly under a
/// mark, would keep its pixels. Apply must refuse instead of reporting success.
#[test]
fn image_in_soft_mask_or_pattern_is_refused() {
    let img = stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &[0, 255]);
    let smask = one_page(
        b"q /GS1 gs 0 0 1 rg 0 0 612 792 re f Q",
        "",
        &[img.clone(), stream("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /XObject << /Im1 6 0 R >> >>", b"q 100 0 0 100 200 400 cm /Im1 Do Q")],
        "",
    );
    let smask = String::from_utf8_lossy(&smask).replace("/Font << /F1 3 0 R >>", "/ExtGState << /GS1 << /SMask << /S /Luminosity /G 7 0 R >> >> >>");
    let pattern = one_page(
        b"/Pattern cs /P1 scn 200 400 100 100 re f",
        "",
        &[img, stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 100] /XStep 100 /YStep 100 /Matrix [1 0 0 1 200 400] /Resources << /XObject << /Im1 6 0 R >> >>", b"q 100 0 0 100 0 0 cm /Im1 Do Q")],
        "",
    );
    let pattern = String::from_utf8_lossy(&pattern).replace("/Font << /F1 3 0 R >>", "/Pattern << /P1 7 0 R >>");
    for bytes in [smask, pattern] {
        let mut doc = PdfDocument::from_bytes(bytes.as_bytes()).unwrap();
        add_area_mark(&mut doc, 0, Rect { x0: 190.0, y0: 282.0, x1: 250.0, y1: 402.0 }, &MarkStyle::default()).unwrap();
        match apply(&mut doc, &ApplyOptions::default()) {
            Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.place.contains("soft mask or tiling pattern")), "{l:?}"),
            r => panic!("expected a refusal, got {r:?}"),
        }
    }
}

/// A page with layer 6 (off) and `res` added to its resources.
fn layered(content: &[u8], res: &str, more: &[Vec<u8>]) -> Vec<u8> {
    let mut objs = vec![obj("<< /Type /OCG /Name (L) >>")];
    objs.extend_from_slice(more);
    let b = one_page(content, "", &objs, "/OCProperties << /OCGs [6 0 R] /D << /OFF [6 0 R] >> >>");
    // Same length replacement target is not needed: MuPDF repairs the shifted xref.
    String::from_utf8_lossy(&b).replace("/Font << /F1 3 0 R >>", &format!("/Font << /F1 3 0 R >> /Properties << /oc1 6 0 R >> {res}")).into_bytes()
}

fn strip_layers(bytes: &[u8]) -> (PdfDocument, Result<SanitizeReport>) {
    let mut doc = PdfDocument::from_bytes(bytes).unwrap();
    let r = sanitize(&mut doc, &Sanitize { hidden_layers: true, ..Sanitize::NONE }, &[]);
    (doc, r)
}

/// Hidden content in a soft-mask group used to stay in the file and show once /OCProperties went.
#[test]
fn hidden_layer_in_soft_mask_group_is_stripped() {
    let grp = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 3 0 R >> /Properties << /oc1 6 0 R >> >>",
        b"1 g 0 0 612 792 re f /OC /oc1 BDC 0 g BT /F1 24 Tf 72 700 Td (HIDDEN-SMASK) Tj ET EMC",
    );
    let bytes = layered(b"q /GS1 gs 0 0 1 rg 0 0 612 792 re f Q", "/ExtGState << /GS1 << /SMask << /S /Luminosity /G 7 0 R >> >> >>", &[grp]);
    let (doc, r) = strip_layers(&bytes);
    assert_eq!(r.unwrap().hidden_layer_blocks, 1);
    assert!(leaks(&save_to_vec(&doc).unwrap(), &["HIDDEN-SMASK"]).is_empty());
}

/// Inline image data holding " EI EMC " fools the content lexer into closing the hidden block
/// early; MuPDF reads the image by its size and then shows the text. Sanitize must refuse.
#[test]
fn hidden_layer_lexer_differential_fails_closed() {
    let mut c = b"/OC /oc1 BDC BI /W 16 /H 1 /BPC 8 /CS /G ID ".to_vec();
    c.extend_from_slice(b" EI EMC aaaaaaaa EI BT /F1 12 Tf 72 700 Td (HIDDEN-INLINE) Tj ET EMC");
    let (_, r) = strip_layers(&layered(&c, "", &[]));
    assert!(matches!(r, Err(RedactError::Leaked(_))), "{r:?}");
}

/// /VE sub-expressions shared by reference used to be evaluated fanout^depth times (4^11: 4.5 s,
/// 8^16: years). Now bounded; an over-budget expression counts as hidden.
#[test]
fn ocmd_visibility_expression_is_bounded() {
    let depth = 16;
    let mut more = vec![obj("<< /Type /OCMD /VE 8 0 R >>")];
    for d in 0..depth {
        let next = if d + 1 == depth { "6 0 R".to_string() } else { format!("{} 0 R", 9 + d) };
        more.push(obj(&format!("[/Or {}]", vec![next; 8].join(" "))));
    }
    let bytes = layered(b"/OC /oc2 BDC BT /F1 12 Tf 72 700 Td (VE-HIDDEN) Tj ET EMC", "", &more);
    let bytes = String::from_utf8_lossy(&bytes).replace("/oc1 6 0 R", "/oc1 6 0 R /oc2 7 0 R").into_bytes();
    let t = std::time::Instant::now();
    let (doc, r) = strip_layers(&bytes);
    assert_eq!(r.unwrap().hidden_layer_blocks, 1);
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    assert!(leaks(&save_to_vec(&doc).unwrap(), &["VE-HIDDEN"]).is_empty());
}

fn text_q(needle: &str) -> Query {
    Query::Text { needle: needle.into(), case_sensitive: true, whole_word: false }
}

/// Marks every hit of `needle` and applies with sanitize off.
fn redact_none(bytes: &[u8], needle: &str) -> (PdfDocument, Result<ApplyReport>) {
    let mut doc = PdfDocument::from_bytes(bytes).unwrap();
    let hits = find(&doc, &text_q(needle)).unwrap();
    assert!(!hits.is_empty(), "fixture: {needle} not found");
    mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    let r = apply(&mut doc, &ApplyOptions { sanitize: Sanitize::NONE, ..Default::default() });
    (doc, r)
}

/// Tagged-PDF alt text, a page label prefix, a link URI and rich text quoting the redacted string
/// are removed (they used to make apply refuse the file).
#[test]
fn tagged_file_quotes_are_stripped() {
    let bytes = one_page(
        b"/P << /MCID 0 >> BDC BT /F1 12 Tf 72 700 Td (TAGGED-SECRET) Tj ET EMC",
        "/Annots [8 0 R 9 0 R] /StructParents 0",
        &[
            obj("<< /Type /StructTreeRoot /K 7 0 R >>"),
            obj("<< /Type /StructElem /S /P /P 6 0 R /Pg 4 0 R /K 0 /ActualText (TAGGED-SECRET) /Alt (alt TAGGED-SECRET) /T (TAGGED-SECRET title) /E (TAGGED-SECRET) >>"),
            obj("<< /Type /Annot /Subtype /Link /Rect [72 100 200 120] /A 10 0 R >>"),
            obj("<< /Type /Annot /Subtype /FreeText /Rect [300 100 500 150] /Contents (note) /DA (/Helv 10 Tf 0 g) /RC 11 0 R >>"),
            obj("<< /S /URI /URI (https://example.com/TAGGED-SECRET) >>"),
            stream("", b"<body><p>TAGGED-SECRET</p></body>"),
        ],
        "/StructTreeRoot 6 0 R /MarkInfo << /Marked true >> /PageLabels << /Nums [0 << /S /D /P (TAGGED-SECRET-) >>] >>",
    );
    let (doc, r) = redact_none(&bytes, "TAGGED-SECRET");
    assert!(r.unwrap().sanitize.quoting >= 5);
    let l = leaks(&save_to_vec(&doc).unwrap(), &["TAGGED-SECRET"]);
    assert!(l.is_empty(), "{l:?}");
}

/// Removing invisible text must keep the page's links and free-text annotations: MuPDF drops
/// every one a redaction touches, and the page-sized pass touches all of them.
#[test]
fn hidden_text_keeps_annotations() {
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (VISIBLE) Tj ET BT 3 Tr /F1 12 Tf 72 650 Td (OCR-LAYER) Tj ET",
        "/Annots [6 0 R 7 0 R]",
        &[
            obj("<< /Type /Annot /Subtype /Link /Rect [72 100 200 120] /A << /S /URI /URI (https://example.com) >> >>"),
            obj("<< /Type /Annot /Subtype /FreeText /Rect [300 100 500 150] /Contents (note) /DA (/Helv 10 Tf 0 g) >>"),
        ],
        "",
    );
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    let r = sanitize(&mut doc, &Sanitize { hidden_text: true, ..Sanitize::NONE }, &[]).unwrap();
    assert_eq!(r.hidden_text_pages, 1);
    let saved = save_to_vec(&doc).unwrap();
    assert!(leaks(&saved, &["OCR-LAYER"]).is_empty());
    let re = PdfDocument::from_bytes(&saved).unwrap();
    assert_eq!(re.find_page(0).unwrap().get_dict("Annots").unwrap().unwrap().len().unwrap(), 2);
}

/// Short names (a /Dests key) are checked for 5+ char needles, and sanitize removes the key.
#[test]
fn short_name_quoting_is_found_and_stripped() {
    let bytes = one_page(b"BT /F1 12 Tf 72 700 Td (KODEX) Tj ET", "", &[], "/Dests << /KODEX [4 0 R /Fit] >>");
    assert!(leaks(&bytes, &["KODEX"]).iter().any(|l| l.place.contains("name")));
    let (doc, r) = redact_none(&bytes, "KODEX");
    assert_eq!(r.unwrap().sanitize.quoting, 1);
    let l = leaks(&save_to_vec(&doc).unwrap(), &["KODEX"]);
    assert!(l.is_empty(), "{l:?}");
}

/// A zip archive with one entry, deflated or stored.
fn zip(name: &str, data: &[u8]) -> Vec<u8> {
    zip_as(name, data, false)
}

fn zip_as(name: &str, data: &[u8], stored: bool) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    let packed = if stored { data.to_vec() } else { e.finish().unwrap() };
    let method = if stored { 0 } else { 8 };
    let mut crc = flate2::Crc::new();
    crc.update(data);
    let (crc, n, c, u) = (crc.sum().to_le_bytes(), (name.len() as u16).to_le_bytes(), (packed.len() as u32).to_le_bytes(), (data.len() as u32).to_le_bytes());
    // Version, flags, method 8, time/date, crc, sizes, name length, extra length.
    let common = [&[20, 0, 0, 0, method, 0, 0, 0, 0, 0][..], &crc, &c, &u, &n, &[0, 0]].concat();
    let mut z = [b"PK\x03\x04", &common[..], name.as_bytes(), &packed].concat();
    let cd = z.len() as u32;
    // Central directory: version made by, the common fields, comment length, disk, attributes, offset 0.
    z.extend_from_slice(&[b"PK\x01\x02", &[20, 0][..], &common, &[0; 10], &0u32.to_le_bytes(), name.as_bytes()].concat());
    let cd_len = z.len() as u32 - cd;
    z.extend_from_slice(&[b"PK\x05\x06", &[0, 0, 0, 0, 1, 0, 1, 0][..], &cd_len.to_le_bytes(), &cd.to_le_bytes(), &[0, 0]].concat());
    z
}

/// Compressed and binary attachments: verify unpacks gzip, zip and attached PDFs, and fails
/// closed on what it cannot read.
#[test]
fn packed_attachments_are_scanned() {
    use std::io::Write;
    let secret = b"copy of ATTACH-SECRET";
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(secret).unwrap();
    let mut zl = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    zl.write_all(b"BT /F1 12 Tf 72 700 Td (ATTACH-SECRET) Tj ET").unwrap();
    // An attached PDF whose page text is in a compressed stream: only a nested verify sees it.
    let nested = pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R >>"),
            obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
            obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>"),
            stream("/Filter /FlateDecode", &zl.finish().unwrap()),
        ],
        "",
    );
    let gz = gz.finish().unwrap();
    // A harmless first gzip member: `gunzip` reads on into the second.
    let mut first = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    first.write_all(b"harmless").unwrap();
    let multi = [first.finish().unwrap(), gz.clone()].concat();
    // A deflated local entry the central directory does not list.
    let (listed, unlisted) = (zip("a.txt", b"harmless"), zip("b.txt", secret));
    let cd = |z: &[u8]| u32::from_le_bytes(z[z.len() - 6..z.len() - 2].try_into().unwrap()) as usize;
    let extra = &unlisted[..cd(&unlisted)];
    let mut hidden_zip = [&listed[..cd(&listed)], extra, &listed[cd(&listed)..]].concat();
    let n = hidden_zip.len();
    hidden_zip[n - 6..n - 2].copy_from_slice(&((cd(&listed) + extra.len()) as u32).to_le_bytes());
    let cases: [(&str, Vec<u8>, &str); 8] = [
        ("gzip", gz.clone(), "/Type /EmbeddedFile"),
        ("gzip without /Type", gz.clone(), ""),
        ("multi-member gzip", multi, "/Type /EmbeddedFile"),
        ("zip", zip("a.txt", secret), "/Type /EmbeddedFile"),
        ("unlisted zip entry", hidden_zip, "/Type /EmbeddedFile"),
        ("binary", vec![0, 1, 2, 3, 0xff, 0x10], "/Type /EmbeddedFile"),
        // "ATTACH" in EBCDIC: no control bytes, but not UTF-8.
        ("ebcdic", vec![0xC1, 0xE3, 0xE3, 0xC1, 0xC3, 0xC8], "/Type /EmbeddedFile"),
        ("pdf", nested, "/Type /EmbeddedFile"),
    ];
    for (what, data, dict) in cases {
        let bytes = one_page(
            b"BT /F1 12 Tf 72 700 Td (ATTACH-SECRET) Tj ET",
            "",
            &[obj("<< /Type /Filespec /F (a) /EF << /F 7 0 R >> >>"), stream(dict, &data)],
            "/Names << /EmbeddedFiles << /Names [(a) 6 0 R] >> >>",
        );
        match redact_none(&bytes, "ATTACH-SECRET").1 {
            Err(RedactError::Leaked(l)) => {
                assert!(l.iter().any(|l| l.critical && l.place.contains("attachment")), "{what}: {l:?}");
                // Zips are read, not reported unreadable.
                if what.contains("zip") {
                    assert!(l.iter().all(|l| !l.place.contains("cannot read")) && l.iter().any(|l| l.needle == "ATTACH-SECRET"), "{what}: {l:?}");
                }
            }
            r => panic!("{what}: expected a leak, got {r:?}"),
        }
        // One budget for all unpacking: past it, attachments count as unread.
        if what == "gzip" {
            let t = Targets { pages: Vec::new(), needles: vec!["ATTACH-SECRET".into()] };
            let l = verify_at(&bytes, &t, 0, &std::cell::Cell::new(5)).unwrap().leaks;
            assert!(l.iter().any(|l| l.critical && l.place.contains("scan budget")), "{l:?}");
        }
    }
}

/// Letter-spaced text (spaces or kerning gaps between letters) and a WinAnsi soft hyphen.
#[test]
fn find_letter_spaced_and_soft_hyphen() {
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (K E R N) Tj ET BT /F1 12 Tf 72 650 Td [(K) -900 (E) -900 (R) -900 (N)] TJ ET\n\
BT /F1 12 Tf 72 600 Td (redac\\255tion) Tj ET",
        "",
        &[],
        "",
    );
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    assert_eq!(find(&doc, &text_q("KERN")).unwrap().len(), 2, "{}", page_text(&doc));
    assert_eq!(find(&doc, &text_q("redaction")).unwrap().len(), 1, "{}", page_text(&doc));
}

/// JBIG2 (MMR-coded generic region, 16x16 all black): covered pixels turn white.
#[test]
fn jbig2_image_under_mark_is_blanked() {
    // T.6: row 0 = horizontal mode, white run 0, black run 16; every later row = V0 V0.
    let bits = format!("001{}{}{}", "00110101", "0000010111", "11".repeat(15));
    let mmr: Vec<u8> =
        bits.as_bytes().chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, &x)| b | ((x - b'0') << (7 - i)))).collect();
    let be = |n: u32| n.to_be_bytes();
    let region = [be(16), be(16), be(0), be(0)].concat();
    // Page information segment (type 48), then an immediate lossless generic region (type 38, MMR).
    let j = [
        &be(0)[..], &[48, 0, 1], &be(19), &region, &[0, 0, 0],
        &be(1), &[38, 0, 1], &be(18 + mmr.len() as u32), &region, &[0, 1], &mmr,
    ]
    .concat();
    let bytes = pdf(
        &[
            obj("<< /Type /Catalog /Pages 2 0 R >>"),
            obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im1 5 0 R >> >> /Contents 4 0 R >>"),
            stream("", b"q 100 0 0 100 200 400 cm /Im1 Do Q"),
            stream("/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode", &j),
        ],
        "",
    );
    // Whether pixel (x, y) of the page's image is white.
    let white = |doc: &PdfDocument, x: usize, y: usize| {
        let xo = doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap();
        let im = xo.get_dict_val(0).unwrap().unwrap();
        let bpc = im.get_dict("BitsPerComponent").unwrap().unwrap().as_int().unwrap() as usize;
        let data = doc.xref_stream(im.as_indirect().unwrap()).unwrap();
        let bit = x * bpc;
        let v = (data[y * (16 * bpc).div_ceil(8) + bit / 8] >> (8 - bpc - bit % 8)) as usize & ((1 << bpc) - 1);
        v == (1 << bpc) - 1
    };
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    assert!(!white(&doc, 0, 8) && !white(&doc, 15, 8), "fixture does not decode to black");
    add_area_mark(&mut doc, 0, Rect { x0: 190.0, y0: 282.0, x1: 250.0, y1: 402.0 }, &MarkStyle::default()).unwrap();
    apply(&mut doc, &ApplyOptions::default()).unwrap();
    let re = PdfDocument::from_bytes(&save_to_vec(&doc).unwrap()).unwrap();
    for y in 0..16 {
        assert!(white(&re, 0, y), "covered pixel kept at row {y}");
        assert!(!white(&re, 15, y), "uncovered pixel changed at row {y}");
    }
}

/// An unused XObject in a hidden layer and a widget in a hidden layer (with its field value) go.
#[test]
fn hidden_layer_unused_xobject_and_widget_go() {
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 50] /OC 6 0 R /Resources << /Font << /F1 3 0 R >> >>",
        b"BT /F1 12 Tf 0 10 Td (UNUSED-HIDDEN) Tj ET",
    );
    let widget = obj("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /V (WIDGET-HIDDEN) /OC 6 0 R /Rect [72 100 200 120] /P 4 0 R >>");
    let bytes = layered(b"BT /F1 12 Tf 72 700 Td (SHOWN) Tj ET", "/XObject << /Fx 7 0 R >>", &[form, widget]);
    let bytes = String::from_utf8_lossy(&bytes)
        .replace("/Contents 5 0 R", "/Contents 5 0 R /Annots [8 0 R]")
        .replace("/OCProperties", "/AcroForm << /Fields [8 0 R] >> /OCProperties")
        .into_bytes();
    let (doc, r) = strip_layers(&bytes);
    r.unwrap();
    let l = leaks(&save_to_vec(&doc).unwrap(), &["UNUSED-HIDDEN", "WIDGET-HIDDEN"]);
    assert!(l.is_empty(), "{l:?}");
}

/// A clean zip passes; a password-protected attached PDF and a stored entry whose directory size
/// hides its tail count as unread.
#[test]
fn attachment_edge_cases() {
    let attach = |data: &[u8]| {
        one_page(
            b"BT /F1 12 Tf 72 700 Td (ATTACH-SECRET) Tj ET",
            "",
            &[obj("<< /Type /Filespec /F (a) /EF << /F 7 0 R >> >>"), stream("/Type /EmbeddedFile", data)],
            "/Names << /EmbeddedFiles << /Names [(a) 6 0 R] >> >>",
        )
    };
    let (_, r) = redact_none(&attach(&zip("a.txt", b"hello world")), "ATTACH-SECRET");
    r.unwrap();

    let unread = |data: &[u8], why: &str| match redact_none(&attach(data), "ATTACH-SECRET").1 {
        Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.critical && l.place.contains(why)), "{why}: {l:?}"),
        r => panic!("{why}: expected a leak, got {r:?}"),
    };
    // Standard security handler with a user password: the empty password does not open it.
    let locked = pdf(
        &[obj("<< /Type /Catalog /Pages 2 0 R >>"), obj("<< /Type /Pages /Kids [] /Count 0 >>")],
        &format!(
            "/Encrypt << /Filter /Standard /V 1 /R 2 /O <{0}> /U <{0}> /P -4 >> /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>]",
            "ab".repeat(32)
        ),
    );
    unread(&locked, "needs a password");

    // A stored entry whose directory sizes (8) cut it short of the local header's.
    let mut cut = zip_as("a.txt", b"harmless text, ATTACH-SECRET in the tail", true);
    let n = cut.len();
    let cd = u32::from_le_bytes(cut[n - 6..n - 2].try_into().unwrap()) as usize;
    cut[cd + 20..cd + 28].copy_from_slice(&[8, 0, 0, 0, 8, 0, 0, 0]);
    unread(&cut, "cannot read");
}

/// Name-tree entries quoting the needle go; /Limits are recomputed and emptied leaves dropped.
#[test]
fn name_tree_limits_are_recomputed() {
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (LIMIT-SECRET) Tj ET",
        "",
        &[
            obj("<< /Kids [7 0 R] >>"),
            obj("<< /Kids [8 0 R 9 0 R] /Limits [(LIMIT-SECRET) (zzz)] >>"),
            obj("<< /Names [(LIMIT-SECRET) [4 0 R /Fit]] /Limits [(LIMIT-SECRET) (LIMIT-SECRET)] >>"),
            obj("<< /Names [(yyy) [4 0 R /Fit] (zzz) [4 0 R /Fit]] /Limits [(yyy) (zzz)] >>"),
        ],
        "/Names << /Dests 6 0 R >>",
    );
    let (doc, r) = redact_none(&bytes, "LIMIT-SECRET");
    r.unwrap();
    let names = doc.catalog().unwrap().get_dict("Names").unwrap().unwrap();
    let mid = names.get_dict("Dests").unwrap().unwrap().get_dict("Kids").unwrap().unwrap().get_array(0).unwrap().unwrap();
    assert_eq!(mid.get_dict("Kids").unwrap().unwrap().len().unwrap(), 1);
    let limits = mid.get_dict("Limits").unwrap().unwrap();
    assert_eq!(text_of(&limits.get_array(0).unwrap().unwrap()).unwrap(), "yyy");
    assert_eq!(text_of(&limits.get_array(1).unwrap().unwrap()).unwrap(), "zzz");
    let l = leaks(&save_to_vec(&doc).unwrap(), &["LIMIT-SECRET"]);
    assert!(l.is_empty(), "{l:?}");
}

/// A field's /T is required: a quoting one is replaced, not deleted.
#[test]
fn quoting_field_name_gets_a_placeholder() {
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (FIELDNAME-SECRET) Tj ET",
        "/Annots [7 0 R]",
        &[
            obj("<< /FT /Tx /T (FIELDNAME-SECRET) /Kids [7 0 R] >>"),
            obj("<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [300 100 400 120] /P 4 0 R >>"),
        ],
        "/AcroForm << /Fields [6 0 R] >>",
    );
    let (doc, r) = redact_none(&bytes, "FIELDNAME-SECRET");
    r.unwrap();
    let af = doc.catalog().unwrap().get_dict("AcroForm").unwrap().unwrap();
    let t = af.get_dict("Fields").unwrap().unwrap().get_array(0).unwrap().unwrap().get_dict("T").unwrap().expect("/T kept");
    assert!(text_of(&t).unwrap().starts_with("redacted-"));
}

/// Invisible text in an annotation the annotation switch removes must not fail sanitize.
#[test]
fn invisible_text_in_removed_annotation() {
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (VISIBLE) Tj ET",
        "/Annots [6 0 R]",
        &[
            obj("<< /Type /Annot /Subtype /Stamp /Rect [72 300 272 330] /AP << /N 7 0 R >> >>"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 200 30] /Resources << /Font << /F1 3 0 R >> >>", b"BT 3 Tr /F1 12 Tf 5 10 Td (STAMP-OCR) Tj ET"),
        ],
        "",
    );
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    sanitize(&mut doc, &Sanitize { hidden_text: true, annotations: true, ..Sanitize::NONE }, &[]).unwrap();
    assert!(leaks(&save_to_vec(&doc).unwrap(), &["STAMP-OCR"]).is_empty());
}

/// A word hyphenated across two text blocks.
#[test]
fn find_hyphen_across_blocks() {
    let bytes = one_page(b"BT /F1 12 Tf 72 700 Td (redac-) Tj ET BT /F1 12 Tf 72 300 Td (tion) Tj ET", "", &[], "");
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    assert_eq!(find(&doc, &text_q("redaction")).unwrap().len(), 1, "{:?}", page_text(&doc));
}

/// The hidden-layer oracle compares glyph positions: removed visible text with the same letters
/// must not cancel out revealed text elsewhere.
#[test]
fn oracle_compares_positions() {
    let g = |v: &[(char, i32, i32)]| v.iter().map(|&k| (k, 1)).collect::<sanitize::Glyphs>();
    assert!(!sanitize::revealed(&g(&[('A', 0, 0), ('B', 10, 0)]), &g(&[('A', 0, 0)])));
    assert!(sanitize::revealed(&g(&[('A', 0, 0)]), &g(&[('A', 0, 50)])));
}

/// Scrub fails closed on direct nesting past 32 levels instead of skipping the rest.
#[test]
fn scrub_fails_closed_past_depth_32() {
    let deep = format!("/X {}{}", "[".repeat(40), "]".repeat(40));
    let mut doc = PdfDocument::from_bytes(&one_page(b"", "", &[], &deep)).unwrap();
    let r = sanitize(&mut doc, &Sanitize::NONE, &[]);
    assert!(matches!(r, Err(RedactError::Leaked(_))), "{r:?}");
}

/// A listed deflated entry whose stated size runs past its deflate stream over another local entry:
/// the walk for unlisted entries skips that one as inside the first, so it must fail closed.
#[test]
fn zip_entry_size_covering_another_entry_fails_closed() {
    let a = zip("a.txt", b"harmless");
    let b = zip("b.txt", b"copy of ATTACH-SECRET");
    let cd = |z: &[u8]| u32::from_le_bytes(z[z.len() - 6..z.len() - 2].try_into().unwrap()) as usize;
    let (a_local, a_cd, b_local) = (&a[..cd(&a)], &a[cd(&a)..a.len() - 22], &b[..cd(&b)]);
    let size = (u32::from_le_bytes(a_local[18..22].try_into().unwrap()) as usize + b_local.len()) as u32;
    let mut z = [a_local, b_local, a_cd, &a[a.len() - 22..]].concat();
    let new_cd = a_local.len() + b_local.len();
    z[18..22].copy_from_slice(&size.to_le_bytes());
    z[new_cd + 20..new_cd + 24].copy_from_slice(&size.to_le_bytes());
    let n = z.len();
    z[n - 6..n - 2].copy_from_slice(&(new_cd as u32).to_le_bytes());
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (ATTACH-SECRET) Tj ET",
        "",
        &[obj("<< /Type /Filespec /F (a) /EF << /F 7 0 R >> >>"), stream("/Type /EmbeddedFile", &z)],
        "/Names << /EmbeddedFiles << /Names [(a) 6 0 R] >> >>",
    );
    match redact_none(&bytes, "ATTACH-SECRET").1 {
        Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.critical && l.place.contains("attachment")), "{l:?}"),
        r => panic!("expected a leak, got {r:?}"),
    }
}

/// A name tree listing the same kid twice per level would be walked 2^depth times; shared nodes
/// fail closed instead.
#[test]
fn name_tree_with_shared_kids_fails_closed() {
    let bytes = one_page(
        b"BT /F1 12 Tf 72 700 Td (SHARED-SECRET) Tj ET",
        "",
        &[obj("<< /Kids [7 0 R 7 0 R] >>"), obj("<< /Names [(SHARED-SECRET) [4 0 R /Fit]] /Limits [(SHARED-SECRET) (SHARED-SECRET)] >>")],
        "/Names << /Dests 6 0 R >>",
    );
    match redact_none(&bytes, "SHARED-SECRET").1 {
        Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.place.contains("shared nodes")), "{l:?}"),
        r => panic!("expected a refusal, got {r:?}"),
    }
}
