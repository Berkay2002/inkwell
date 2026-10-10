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
        Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.needle == "123-45-6789" && l.place.contains("stream")), "{l:?}"),
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
        Err(RedactError::Leaked(l)) => assert!(l.iter().any(|l| l.critical && l.place.contains("stream")), "{l:?}"),
        r => panic!("expected a leak, got {r:?}"),

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
