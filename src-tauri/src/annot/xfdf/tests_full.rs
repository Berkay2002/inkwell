//! Appearances, rich text, carets and attached files in XFDF and FDF: round trips between two documents, and a
//! hostile file per new element.

use super::tests::{ONE, certified_with_a_field, one_page_pdf};
use super::*;
use crate::annot::stamps::place_one;
use crate::annot::stamps::tests::one_stamp;
use crate::annot::{Patch, edit_one, list_all};
use crate::outline::tests::{dir, open_bytes};
use std::io::Write;
use std::sync::Arc;

thread_local! {
    /// Makes `finish` fail after the appearance's objects were written (see `finish`).
    pub(super) static FAIL_AFTER_INSTALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02X}")).collect()
}

fn zlib(d: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(d).unwrap();
    z.finish().unwrap()
}

fn obj(n: usize, body: &str) -> Vec<u8> {
    format!("{n} 0 obj\n{body}\nendobj\n").into_bytes()
}

fn stream_obj(n: usize, dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("{n} 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream\nendobj\n");
    v
}

fn fdf_file(annots: &str, objs: &[Vec<u8>]) -> Vec<u8> {
    let mut f = format!("%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [{annots}] >> >>\nendobj\n").into_bytes();
    for o in objs {
        f.extend_from_slice(o);
    }
    f.extend_from_slice(b"trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    f
}

fn blank(test: &str) -> (Arc<Engine>, DocId, String) {
    let d = dir("xfdf", test);
    open_bytes(&d, "a.pdf", &one_page_pdf("", "", &[]))
}

fn run(eng: &Engine, id: DocId, bytes: &[u8]) -> Summary {
    import_one(eng, id, &parse(bytes).unwrap(), EditSource::User).unwrap().summary
}

fn reasons(v: &[Skipped]) -> Vec<(&str, usize)> {
    v.iter().map(|s| (s.reason.as_str(), s.count)).collect()
}

/// Every comment of the document as a file would carry it, with appearances of every type read.
fn read_all(eng: &Engine, id: DocId) -> Vec<Comment> {
    eng.read_custom(id, |d| {
        let b = Budget::new();
        let (v, _) = each(d, |p, a| {
            let sub = Sub::of_type(a.r#type()?);
            Ok(sub.map(|s| {
                let mut c = read(&a.object(), s, &b, true);
                c.page = i64::from(p);
                c.xref = a.xref().unwrap_or(0);
                c
            }))
        })?;
        Ok(v.into_iter().flatten().collect())
    })
    .unwrap()
}

/// The colour at (x, y) of page 1 as the document would be saved now.
fn pixel(eng: &Engine, id: DocId, x: usize, y: usize) -> [u8; 3] {
    let bytes = eng.unsaved_bytes(id).unwrap().expect("an edit is pending");
    let doc = PdfDocument::from_bytes(&bytes).unwrap();
    let pix = doc.load_page(0).unwrap().to_pixmap(&Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true).unwrap();
    let i = y * pix.stride() as usize + x * pix.n() as usize;
    let s = pix.samples();
    [s[i], s[i + 1], s[i + 2]]
}

const RED_FORM: &str = "/Type /XObject /Subtype /Form /BBox [0 0 100 50]";

fn freetext(n: usize, ap: usize) -> String {
    format!("<< /Type /Annot /Subtype /FreeText /Page 0 /Rect [100 600 200 650] /NM (ft{n}) /Contents (hi) /DA (/Helv 12 Tf 0 g) /AP << /N {ap} 0 R >> >>")
}

// ---- round trips ------------------------------------------------------------------------------

#[test]
fn a_caret_keeps_its_fringe_and_symbol_through_both_formats() {
    let file = br##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <caret page="0" rect="100,600,130,620" name="c1" title="Alex" color="#FF0000" fringe="1,2,3,4" symbol="P"><contents>insert here</contents></caret>
        </annots></xfdf>"##;
    let (a, ida, _) = blank("caret-a");
    let s = run(&a, ida, file);
    assert_eq!((s.added, s.skipped.len()), (1, 0), "{s:?}");
    let first = read_all(&a, ida);
    assert_eq!((first[0].sub, first[0].fringe, first[0].symbol.as_str(), first[0].contents.as_str()), (Sub::Caret, Some([1.0, 2.0, 3.0, 4.0]), "P", "insert here"));
    for (fmt, ext) in [(Format::Xfdf, "xfdf"), (Format::Fdf, "fdf")] {
        let (bytes, done) = export(&a, ida, fmt, "a.pdf").unwrap();
        assert_eq!((done.count, done.left_out), (1, 0), "{ext}");
        let (b, idb, _) = blank(&format!("caret-b-{ext}"));
        let s = run(&b, idb, &bytes);
        assert_eq!((s.added, s.skipped.len()), (1, 0), "{ext}: {s:?}");
        let got = read_all(&b, idb);
        assert_eq!((got[0].sub, got[0].fringe, got[0].symbol.as_str(), got[0].contents.as_str(), got[0].author.as_str()), (Sub::Caret, Some([1.0, 2.0, 3.0, 4.0]), "P", "insert here", "Alex"), "{ext}");
        close(&got[0].rect.unwrap(), &first[0].rect.unwrap(), "caret rect");
    }
}

fn close(a: &[f32], b: &[f32], what: &str) {
    super::tests::close(a, b, what);
}

#[test]
fn rich_text_is_rebuilt_from_a_whitelist_and_round_trips_with_the_plain_text_kept() {
    let rich = r##"<contents-richtext><body xmlns="http://www.w3.org/1999/xhtml" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/" xfa:APIVersion="Acrobat:8.0.0" xfa:spec="2.0.2">
        <p dir="ltr"><span style="font-size:12.0pt;color:#FF0000;background:url(http://x/y)">Hello </span><b>bold</b><script>alert(1)</script><img src="http://x/i.png"/><a href="javascript:alert(2)" onclick="x()">link</a></p><p>second</p></body></contents-richtext>"##;
    let file = format!(
        r##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <freetext page="0" rect="100,600,300,650" name="r1"><contents>plain kept</contents>{rich}<defaultappearance>/Helv 12 Tf 0 g</defaultappearance></freetext>
        <freetext page="0" rect="100,500,300,550" name="r2">{rich}<defaultappearance>/Helv 12 Tf 0 g</defaultappearance></freetext>
        </annots></xfdf>"##
    );
    let (a, ida, _) = blank("rich-a");
    let s = run(&a, ida, file.as_bytes());
    assert_eq!((s.added, s.skipped.len(), s.notes.len()), (2, 0, 0), "{s:?}");
    let got = read_all(&a, ida);
    let r = got[0].rich.as_deref().expect("rich text kept");
    assert!(r.contains("<b>bold</b>") && r.contains("font-size:12.0pt;color:#FF0000") && r.contains("dir=\"ltr\""), "{r}");
    for gone in ["script", "alert", "<img", "href", "onclick", "url(", "javascript"] {
        assert!(!r.contains(gone), "{gone} is not carried: {r}");
    }
    assert_eq!(got[0].contents, "plain kept", "/Contents stays what the file said");
    assert_eq!(got[1].contents, "Hello boldlink\nsecond", "plain text is derived when the file gave none");
    for (fmt, ext) in [(Format::Xfdf, "xfdf"), (Format::Fdf, "fdf")] {
        let (bytes, _) = export(&a, ida, fmt, "a.pdf").unwrap();
        let (b, idb, _) = blank(&format!("rich-b-{ext}"));
        run(&b, idb, &bytes);
        let again = read_all(&b, idb);
        assert_eq!((again[0].rich.as_deref(), again[0].contents.as_str()), (Some(r), "plain kept"), "{ext}");
        assert_eq!(again[1].rich, got[1].rich, "{ext}");
    }
    // An edit by Inkwell's own code replaces the plain text, so the rich text goes (it would show the old words).
    edit_one(&a, ida, 0, got[0].xref, &Patch { contents: Some("changed".into()), ..Default::default() }, "Alex", EditSource::User).unwrap();
    let after = read_all(&a, ida);
    assert_eq!((after[0].rich.is_none(), after[0].contents.as_str()), (true, "changed"));
}

#[test]
fn an_attached_file_round_trips_by_its_bytes_and_is_not_added_to_the_documents_files() {
    let data: Vec<u8> = (0..=255u8).chain(b"hello attachment".iter().copied()).collect();
    let file = format!(
        r##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <fileattachment page="0" rect="100,600,120,620" name="fa1" title="Alex" file="C:\evil\..\notes.txt" icon="Paperclip"><contents>see file</contents><data mode="raw" encoding="hex" length="{}" MIME-type="text/plain" modification="D:20260102030405Z" description="My notes">{}</data></fileattachment>
        </annots></xfdf>"##,
        data.len(),
        hex(&data)
    );
    let (a, ida, _) = blank("attach-a");
    let s = run(&a, ida, file.as_bytes());
    assert_eq!((s.added, s.files, s.skipped.len(), s.notes.len()), (1, 1, 0, 0), "{s:?}");
    let first = read_all(&a, ida);
    let f = first[0].file.as_ref().expect("file");
    assert_eq!((f.name.as_str(), f.desc.as_str(), f.mime.as_str(), f.modified.as_str()), ("notes.txt", "My notes", "text/plain", "D:20260102030405Z"));
    assert_eq!(f.data, data);
    assert_eq!((first[0].sub, first[0].icon.as_str(), first[0].contents.as_str()), (Sub::FileAttachment, "Paperclip", "see file"));
    let no_tree = |e: &Engine, i: DocId| e.read_custom(i, |d| Ok(get(&d.catalog()?, "Names").is_none())).unwrap();
    assert!(no_tree(&a, ida), "the document's own embedded files are untouched");

    for (fmt, ext) in [(Format::Xfdf, "xfdf"), (Format::Fdf, "fdf")] {
        let (bytes, done) = export(&a, ida, fmt, "a.pdf").unwrap();
        assert_eq!((done.count, done.files, done.left_out), (1, 1, 0), "{ext}");
        let (b, idb, _) = blank(&format!("attach-b-{ext}"));
        let s = run(&b, idb, &bytes);
        assert_eq!((s.added, s.files, s.skipped.len()), (1, 1, 0), "{ext}: {s:?}");
        let got = read_all(&b, idb);
        assert_eq!(got[0].file, first[0].file, "{ext}");
        assert!(no_tree(&b, idb), "{ext}");
        // The same file again updates the comment and leaves the one file it has.
        let s = run(&b, idb, &bytes);
        assert_eq!((s.added, s.updated, s.files), (0, 1, 0), "{ext}: {s:?}");
        assert_eq!(read_all(&b, idb).len(), 1);
    }
}

#[test]
fn a_picture_stamp_shows_the_same_picture_in_another_document_through_both_formats() {
    for (fmt, ext) in [(Format::Xfdf, "xfdf"), (Format::Fdf, "fdf")] {
        let d = dir("xfdf", &format!("stamp-{ext}"));
        let (_lib, s, png) = one_stamp(&d);
        let (a, ida, _) = open_bytes(&d, "a.pdf", &one_page_pdf("", "", &[]));
        place_one(&a, ida, 0, &s, &png, [300.0, 400.0], "Alex", EditSource::User).unwrap();
        let (bytes, done) = export(&a, ida, fmt, "a.pdf").unwrap();
        assert_eq!((done.count, done.appearances, done.notes.len()), (1, 1, 0), "{ext}: {done:?}");

        let (b, idb, _) = blank(&format!("stamp-{ext}-b"));
        let steps = b.history(idb).unwrap().len();
        let s = run(&b, idb, &bytes);
        assert_eq!((s.added, s.appearances, s.notes.len()), (1, 1, 0), "{ext}: {s:?}");
        assert_eq!(b.history(idb).unwrap().len(), steps + 1, "{ext}: one undo step");
        let l = list_all(&b, idb).unwrap();
        assert!(l[0].picture, "{ext}: the appearance is the file's picture");
        assert_eq!(read_all(&b, idb)[0].ap, read_all(&a, ida)[0].ap, "{ext}: the same appearance, not only one that exists");
        // Left half red, right half transparent: the picture is drawn, not the stamp's name.
        assert_eq!(pixel(&b, idb, 250, 400), [255, 0, 0], "{ext}");
        assert_eq!(pixel(&b, idb, 350, 400), [255, 255, 255], "{ext}");
        // It is in the bytes of the saved file too, not only in the open document.
        let saved = b.unsaved_bytes(idb).unwrap().unwrap();
        let reopened = PdfDocument::from_bytes(&saved).unwrap();
        assert!(crate::annot::list(&reopened).unwrap().items[0].picture, "{ext}");
        b.undo(idb).unwrap();
        assert!(list_all(&b, idb).unwrap().is_empty(), "{ext}");
    }
}

#[test]
fn the_files_appearance_draws_and_an_edit_by_inkwell_replaces_it() {
    let f = fdf_file("5 0 R", &[obj(5, &freetext(1, 6)), stream_obj(6, RED_FORM, b"1 0 0 rg 0 0 100 50 re f")]);
    let (eng, id, _) = blank("ap-draws");
    let steps = eng.history(id).unwrap().len();
    let s = run(&eng, id, &f);
    assert_eq!((s.added, s.appearances, s.notes.len()), (1, 1, 0), "{s:?}");
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    // Red box filling the /Rect (100..200 across, 600..650 up = 142..192 down).
    assert_eq!(pixel(&eng, id, 150, 167), [255, 0, 0]);
    let x = list_all(&eng, id).unwrap()[0].xref;
    edit_one(&eng, id, 0, x, &Patch { contents: Some("new words".into()), ..Default::default() }, "Alex", EditSource::User).unwrap();
    assert_ne!(pixel(&eng, id, 150, 167), [255, 0, 0], "Inkwell's own redraw replaces the file's");
}

// ---- hostile input ----------------------------------------------------------------------------

#[test]
fn an_appearance_that_cannot_be_taken_falls_back_to_inkwells_drawing_and_is_counted() {
    let big = vec![b' '; 5 << 20];
    let objs = vec![
        obj(10, &freetext(1, 20)),
        obj(11, &freetext(2, 21)),
        obj(12, &freetext(3, 22)),
        obj(13, &freetext(4, 23)),
        obj(14, &freetext(5, 24)),
        obj(15, &freetext(6, 25)),
        obj(16, &freetext(7, 26)),
        obj(17, &freetext(8, 27)),
        stream_obj(20, RED_FORM, b"BT /F9 12 Tf (x) Tj ET"),
        stream_obj(21, RED_FORM, b"/P1 cs /P1 scn 0 0 100 50 re f"),
        stream_obj(22, RED_FORM, b"q BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q"),
        stream_obj(23, &format!("{RED_FORM} /Filter /FlateDecode"), &zlib(&big)),
        stream_obj(
            24,
            &format!("{RED_FORM} /AA << /O << /S /JavaScript /JS (app.alert(1)) >> >> /OC 99 0 R /Metadata 99 0 R /Resources << /Font << /Q << /Type /Font /Subtype /Type3 >> >> >>"),
            b"BT /Q 12 Tf (x) Tj ET",
        ),
        stream_obj(25, &format!("{RED_FORM} /AA << /O << /S /JavaScript /JS (app.alert(1)) >> >> /OC 99 0 R"), b"0 1 0 rg 0 0 100 50 re f"),
        // Form content is not a place for an image codec; nor is a chain that ends in a filter that eats white space.
        stream_obj(26, &format!("{RED_FORM} /Filter /DCTDecode"), b"\xFF\xD8\xFF\xE0 not a picture"),
        stream_obj(27, &format!("{RED_FORM} /Filter [/FlateDecode /ASCIIHexDecode]"), &zlib(b"0 1 0 rg 0 0 100 50 re f")),
    ];
    let f = fdf_file("10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R 16 0 R 17 0 R", &objs);
    let (eng, id, _) = blank("ap-hostile");
    let s = run(&eng, id, &f);
    assert_eq!((s.added, s.appearances), (8, 1), "{s:?}");
    assert_eq!(s.skipped.len(), 0, "{s:?}");
    let mut want = vec![(ap::TOO_BIG, 1), (ap::UNRESOLVED, 1), (ap::UNSUPPORTED, 3), (ap::FILTER, 2)];
    want.sort_unstable();
    let mut got = reasons(&s.notes);
    got.sort_unstable();
    assert_eq!(got, want, "{s:?}");
    // The one that was taken carries none of the dictionary's actions or links.
    let keys = eng
        .read_custom(id, |d| {
            let mut all = Vec::new();
            each(d, |_, a| {
                let o = a.object();
                let n = get(&get(&o, "AP").unwrap(), "N").unwrap();
                if n.read_stream()?.starts_with(b"0 1 0 rg") {
                    for i in 0..n.dict_len()? as i32 {
                        all.push(String::from_utf8_lossy(&n.get_dict_key(i)?.unwrap().as_name()?).into_owned());
                    }
                }
                Ok(())
            })?;
            Ok(all)
        })
        .unwrap();
    assert!(!keys.is_empty() && keys.iter().all(|k| !matches!(k.as_str(), "AA" | "OC" | "Metadata" | "A")), "{keys:?}");
}

#[test]
fn a_bad_appearance_in_xfdf_is_counted_and_the_comment_is_still_made() {
    let file = br##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <square page="0" rect="100,600,200,700" name="s1"><appearance>!!! not base64 !!!</appearance></square>
        <square page="0" rect="100,500,200,550" name="s2"><appearance>JVBERi0xLjQ=</appearance></square>
        </annots></xfdf>"##;
    let (eng, id, _) = blank("ap-bad-xfdf");
    let s = run(&eng, id, file);
    assert_eq!((s.added, s.appearances, s.skipped.len()), (2, 0, 0), "{s:?}");
    assert_eq!(reasons(&s.notes), [(ap::DAMAGED, 2)]);
}

/// `<appearance>` holding `xml` (base64), on a square, as XFDF.
fn xfdf_with_appearances(xmls: &[&str]) -> String {
    use base64::Engine as _;
    let squares: String = xmls
        .iter()
        .enumerate()
        .map(|(i, x)| format!("<square page=\"0\" rect=\"100,600,200,700\" name=\"x{i}\"><appearance>{}</appearance></square>", base64::engine::general_purpose::STANDARD.encode(x)))
        .collect();
    format!("<xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><annots>{squares}</annots></xfdf>")
}

#[test]
fn the_appearance_element_holds_the_xml_form_and_the_old_fdf_form_is_still_read() {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let d = dir("xfdf", "ap-xml");
    let (_lib, s, png) = one_stamp(&d);
    let (a, ida, _) = open_bytes(&d, "a.pdf", &one_page_pdf("", "", &[]));
    place_one(&a, ida, 0, &s, &png, [300.0, 400.0], "Alex", EditSource::User).unwrap();
    let (bytes, _) = export(&a, ida, Format::Xfdf, "a.pdf").unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let enc = text.split("<appearance>").nth(1).unwrap().split("</appearance>").next().unwrap().to_string();
    let xml = String::from_utf8(b64.decode(&enc).unwrap()).unwrap();
    assert!(xml.starts_with("<?xml"), "{}", &xml[..80.min(xml.len())]);
    assert!(xml.contains("<DICT KEY=\"AP\"><STREAM KEY=\"N\">") && xml.contains("<DATA MODE=\"RAW\" ENCODING=\"HEX\""), "{}", &xml[..300.min(xml.len())]);
    // A file Inkwell wrote before: the base64 of an FDF file. Reads to the same appearance.
    let want = read_all(&a, ida)[0].ap.clone().unwrap();
    let old = text.replace(&enc, &b64.encode(ap::to_fdf(&want)));
    assert_ne!(old, text);
    let (b, idb, _) = blank("ap-xml-old");
    let r = run(&b, idb, old.as_bytes());
    assert_eq!((r.added, r.appearances, r.notes.len()), (1, 1, 0), "{r:?}");
    assert_eq!(read_all(&b, idb)[0].ap.as_ref(), Some(&want));
    let (c, idc, _) = blank("ap-xml-new");
    let r = run(&c, idc, text.as_bytes());
    assert_eq!((r.added, r.appearances, r.notes.len()), (1, 1, 0), "{r:?}");
    assert_eq!(read_all(&c, idc)[0].ap.as_ref(), Some(&want));
}

#[test]
fn a_hostile_xml_appearance_goes_through_the_same_whitelist_and_caps() {
    let form = |extra: &str, data: &str| {
        format!(
            "<DICT KEY=\"AP\"><STREAM KEY=\"N\"><NAME KEY=\"Subtype\" VAL=\"Form\"/><ARRAY KEY=\"BBox\"><INT VAL=\"0\"/><INT VAL=\"0\"/><INT VAL=\"100\"/><FIXED VAL=\"50.5\"/></ARRAY>{extra}{data}</STREAM></DICT>"
        )
    };
    let green = "<DATA ENCODING=\"ASCII\" MODE=\"RAW\">0 1 0 rg 0 0 100 50 re f</DATA>";
    let hexed = |b: &[u8]| format!("<DATA ENCODING=\"HEX\" MODE=\"FILTERED\">{}</DATA>", hex(b));
    let bomb = zlib(&vec![0u8; 100 << 20]);
    let xs = [
        form("", green),
        form("<NAME KEY=\"Filter\" VAL=\"DCTDecode\"/>", &hexed(b"\xFF\xD8\xFF\xE0")),
        form("<ARRAY KEY=\"Filter\"><NAME VAL=\"FlateDecode\"/><NAME VAL=\"ASCIIHexDecode\"/></ARRAY>", &hexed(&zlib(b"0 1 0 rg"))),
        form("<NAME KEY=\"Filter\" VAL=\"FlateDecode\"/>", &hexed(&bomb)),
        form("", "<DATA ENCODING=\"BASE64\">AAAA</DATA>"),
        "<ARRAY KEY=\"AP\"/>".to_string(),
        "<DICT KEY=\"AP\"><DICT KEY=\"D\"/></DICT>".to_string(),
        form("<STRING KEY=\"Z\" VAL=\"ignored\"/><DICT KEY=\"AA\"><NAME KEY=\"S\" VAL=\"JavaScript\"/></DICT>", green),
    ];
    let refs: Vec<&str> = xs.iter().map(String::as_str).collect();
    let (eng, id, _) = blank("ap-xml-hostile");
    let s = run(&eng, id, xfdf_with_appearances(&refs).as_bytes());
    assert_eq!((s.added, s.appearances), (8, 2), "{s:?}");
    let mut got = reasons(&s.notes);
    got.sort_unstable();
    let mut want = vec![(ap::FILTER, 2), (ap::TOO_BIG, 1), (ap::DAMAGED, 2), (ap::NOT_STREAM, 1)];
    want.sort_unstable();
    assert_eq!(got, want, "{s:?}");
    // A DOCTYPE in the appearance is as refused as in the file.
    let (eng, id, _) = blank("ap-xml-doctype");
    let s = run(&eng, id, xfdf_with_appearances(&["<!DOCTYPE x [<!ENTITY e \"y\">]><DICT KEY=\"AP\"/>"]).as_bytes());
    assert_eq!((s.added, reasons(&s.notes)), (1, vec![(ap::DAMAGED, 1)]));
}

#[test]
fn a_comment_that_fails_leaves_no_new_objects_in_the_document() {
    let f = fdf_file("5 0 R", &[obj(5, &freetext(1, 6)), stream_obj(6, RED_FORM, b"1 0 0 rg 0 0 100 50 re f")]);
    let (a, ida, _) = blank("orphans-src");
    run(&a, ida, &f);
    let c = read_all(&a, ida).remove(0);
    assert!(c.ap.is_some());
    let mut doc = PdfDocument::from_bytes(&one_page_pdf("", "", &[])).unwrap();
    let before = doc.count_objects().unwrap();
    // The comment it would be written to is not there: nothing may be written for it first.
    assert!(finish(&mut doc, 0, 4242, &c).is_err());
    assert_eq!(doc.count_objects().unwrap(), before);
}

#[test]
fn a_failure_after_the_appearance_was_written_takes_it_back_out() {
    let f = fdf_file("5 0 R", &[obj(5, &freetext(1, 6)), stream_obj(6, RED_FORM, b"1 0 0 rg 0 0 100 50 re f")]);
    let (a, ida, _) = blank("orphans-late-src");
    run(&a, ida, &f);
    let c = read_all(&a, ida).remove(0);
    let annot = "<< /Type /Annot /Subtype /FreeText /Rect [100 600 200 650] /NM (ft1) /Contents (hi) /DA (/Helv 12 Tf 0 g) /P 3 0 R >>";
    let mut doc = PdfDocument::from_bytes(&one_page_pdf("", "/Annots [4 0 R]", &[annot])).unwrap();
    let xref = doc.load_pdf_page(0).unwrap().annotations().next().unwrap().xref().unwrap();
    let before = doc.count_objects().unwrap();
    FAIL_AFTER_INSTALL.with(|f| f.set(true));
    let r = finish(&mut doc, 0, xref, &c);
    FAIL_AFTER_INSTALL.with(|f| f.set(false));
    assert!(r.is_err());
    let after = doc.count_objects().unwrap();
    assert!(after > before, "the appearance was written before it failed");
    for n in before..after {
        let gone = doc.new_indirect(n as i32, 0).unwrap().resolve().ok().flatten().is_none_or(|o| o.is_null().unwrap_or(true));
        assert!(gone, "object {n} is still in the document");
    }
    let (_p, a) = find(&doc, 0, xref).unwrap();
    assert!(get(&a.object(), "AP").is_none(), "the annotation does not point at a removed appearance");
    // And without the failure the same call writes it.
    assert!(finish(&mut doc, 0, xref, &c).unwrap());
}

#[test]
fn a_predictor_that_does_not_match_the_image_is_refused_and_a_matching_one_kept() {
    let form = format!("{RED_FORM} /Resources << /XObject << /Im 7 0 R >> >>");
    let image = |parms: &str| {
        format!("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms {parms}")
    };
    let rows = zlib(&[0, 10, 20, 0, 30, 40]);
    let cases: [(&str, Option<ap::Why>); 6] = [
        ("<< /Predictor 12 /Columns 2 >>", None),
        ("<< /Predictor 2 /Columns 2 /Colors 1 /BitsPerComponent 8 >>", None),
        ("<< /Predictor 12 /Columns 70000 >>", Some(ap::DAMAGED)),
        ("<< /Predictor 12 /Columns 2 /Colors 4 >>", Some(ap::DAMAGED)),
        ("<< /Predictor 12 /Columns 2 /BitsPerComponent 16 >>", Some(ap::DAMAGED)),
        ("<< /Predictor 3 /Columns 2 >>", Some(ap::UNSUPPORTED)),
    ];
    for (i, (parms, refused)) in cases.iter().enumerate() {
        let objs = vec![obj(5, &freetext(1, 6)), stream_obj(6, &form, b"q 100 0 0 50 0 0 cm /Im Do Q"), stream_obj(7, &image(parms), &rows)];
        let (eng, id, _) = blank(&format!("ap-predictor-{i}"));
        let s = run(&eng, id, &fdf_file("5 0 R", &objs));
        assert_eq!(s.added, 1, "{parms}: {s:?}");
        match refused {
            Some(w) => assert_eq!((s.appearances, reasons(&s.notes)), (0, vec![(*w, 1)]), "{parms}"),
            None => {
                assert_eq!((s.appearances, s.notes.len()), (1, 0), "{parms}: {s:?}");
                let ap = read_all(&eng, id).remove(0).ap.unwrap();
                let kept = ap.objs.iter().find_map(|o| o.dict.iter().find(|(k, _)| k == "DecodeParms")).map(|(_, v)| v.clone());
                assert!(matches!(kept, Some(ap::Val::Dict(d)) if d.iter().any(|(k, v)| k == "Columns" && *v == ap::Val::Int(2))), "{parms}");
            }
        }
    }
}

#[test]
fn an_oversized_attachment_and_the_total_are_left_out_and_counted() {
    let mb4 = 4 << 20;
    let one = |name: &str, len: usize| {
        format!(r##"<fileattachment page="0" rect="100,600,120,620" name="{name}" file="{name}.bin"><data mode="raw" encoding="hex">{}</data></fileattachment>"##, "00".repeat(len))
    };
    let file = format!(
        r##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>{}{}{}{}{}</annots></xfdf>"##,
        one("over", mb4 + 1),
        one("a", mb4),
        one("b", mb4),
        one("c", mb4),
        // Longer than the reader takes of one element at all.
        format!(
            r##"<fileattachment page="0" rect="100,600,120,620" name="huge" file="huge.bin"><data mode="raw" encoding="hex">{}</data></fileattachment>"##,
            "0".repeat(10 << 20)
        )
    );
    let (eng, id, _) = blank("attach-caps");
    let s = run(&eng, id, file.as_bytes());
    assert_eq!((s.added, s.files), (2, 2), "{s:?}");
    let mut got = reasons(&s.skipped);
    got.sort_unstable();
    let mut want = vec![(attach::OVER, 2), (attach::NO_ROOM, 1)];
    want.sort_unstable();
    assert_eq!(got, want, "{s:?}");
    // What was taken is the exact bytes, and a written file can be read again (no file is over what a file may hold).
    let (bytes, done) = export(&eng, id, Format::Xfdf, "a.pdf").unwrap();
    assert_eq!((done.count, done.files, done.left_out), (2, 2, 0));
    assert!(parse(&bytes).is_ok());
}

#[test]
fn an_attachment_that_inflates_past_the_cap_is_left_out_in_both_formats() {
    let zeros = vec![0u8; 100 << 20];
    let z = zlib(&zeros);
    let (eng, id, _) = blank("attach-bomb");
    let xfdf = format!(
        r##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><fileattachment page="0" rect="100,600,120,620" name="bomb" file="b.bin"><data mode="filtered" encoding="hex">{}</data></fileattachment></annots></xfdf>"##,
        hex(&z)
    );
    let s = run(&eng, id, xfdf.as_bytes());
    assert_eq!((s.added, reasons(&s.skipped)), (0, vec![(attach::OVER, 1)]), "{s:?}");
    let objs = vec![
        obj(5, "<< /Type /Annot /Subtype /FileAttachment /Page 0 /Rect [100 600 120 620] /NM (bomb) /FS 6 0 R >>"),
        obj(6, "<< /Type /Filespec /F (b.bin) /EF << /F 7 0 R >> >>"),
        stream_obj(7, "/Type /EmbeddedFile /Filter /FlateDecode", &z),
        // A link to a file on the disk is never followed.
        obj(8, "<< /Type /Annot /Subtype /FileAttachment /Page 0 /Rect [100 500 120 520] /NM (link) /FS << /Type /Filespec /F (C:\\\\Windows\\\\notepad.exe) >> >>"),
    ];
    let s = run(&eng, id, &fdf_file("5 0 R 8 0 R", &objs));
    assert_eq!(s.added, 0, "{s:?}");
    let mut got = reasons(&s.skipped);
    got.sort_unstable();
    let mut want = vec![(attach::OVER, 1), (attach::EXTERNAL, 1)];
    want.sort_unstable();
    assert_eq!(got, want, "{s:?}");
    assert!(list_all(&eng, id).unwrap().is_empty());
}

#[test]
fn rich_text_that_is_huge_or_deep_is_dropped_and_the_plain_text_kept() {
    let deep = format!("{}x{}", "<span>".repeat(40), "</span>".repeat(40));
    let huge = "word ".repeat(40_000);
    let file = format!(
        r##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <freetext page="0" rect="100,600,300,650" name="d"><contents>deep plain</contents><contents-richtext><body xmlns="http://www.w3.org/1999/xhtml"><p>{deep}</p></body></contents-richtext></freetext>
        <freetext page="0" rect="100,500,300,550" name="h"><contents>huge plain</contents><contents-richtext><body xmlns="http://www.w3.org/1999/xhtml"><p>{huge}</p></body></contents-richtext></freetext>
        </annots></xfdf>"##
    );
    let (eng, id, _) = blank("rich-hostile");
    let s = run(&eng, id, file.as_bytes());
    assert_eq!((s.added, s.skipped.len()), (2, 0), "{s:?}");
    assert_eq!(reasons(&s.notes), [(rich::TOO_BIG, 2)]);
    let got = read_all(&eng, id);
    assert!(got.iter().all(|c| c.rich.is_none()));
    assert_eq!((got[0].contents.as_str(), got[1].contents.as_str()), ("deep plain", "huge plain"));
    // An FDF's /RC nested past what the XML reader takes is damaged rich text, not a refused file.
    let nested = format!("<body>{}{}</body>", "<span>".repeat(200), "</span>".repeat(200));
    let f = fdf_file("5 0 R", &[obj(5, &format!("<< /Type /Annot /Subtype /FreeText /Page 0 /Rect [10 10 100 40] /NM (n) /Contents (kept) /RC ({nested}) >>"))]);
    let s = run(&eng, id, &f);
    assert_eq!((s.added, reasons(&s.notes)), (1, vec![(rich::DAMAGED, 1)]), "{s:?}");
}

#[test]
fn a_certified_document_refuses_an_import_with_appearances_and_a_signed_one_takes_it_incrementally() {
    let f = fdf_file("5 0 R", &[obj(5, &freetext(1, 6)), stream_obj(6, RED_FORM, b"1 0 0 rg 0 0 100 50 re f")]);
    for p in [1, 2] {
        let d = dir("xfdf", &format!("ap-certified-{p}"));
        let (eng, id, _) = open_bytes(&d, "c.pdf", &certified_with_a_field(p));
        let before = eng.edit_state(id).unwrap();
        match import_one(&eng, id, &parse(&f).unwrap(), EditSource::User) {
            Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "P {p}: {m}"),
            other => panic!("P {p}: {:?}", other.map(|r| r.summary)),
        }
        let after = eng.edit_state(id).unwrap();
        assert_eq!((after.rev, after.undo, after.dirty), (before.rev, before.undo, before.dirty));
    }
    let cat = "/AcroForm << /SigFlags 3 /Fields [4 0 R] >>";
    let sig = "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>";
    let d = dir("xfdf", "ap-signed");
    let (eng, id, path) = open_bytes(&d, "s.pdf", &one_page_pdf(cat, "", &[sig]));
    let original = std::fs::read(&path).unwrap();
    let r = import_one(&eng, id, &parse(&f).unwrap(), EditSource::User).unwrap();
    assert!(r.signed);
    assert_eq!((r.summary.added, r.summary.appearances), (1, 1));
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original));
    // And the plain file of the existing tests still reads as it did.
    assert!(parse(ONE).is_ok());
}

/// `n` FileAttachment comments whose /FS is one file: its stream has the given /Filter and bytes. Returns the
/// summary and how long the whole import took.
fn shared_file(test: &str, n: usize, filter: &str, data: &[u8]) -> (Summary, std::time::Duration) {
    let mut objs = vec![obj(5, "<< /Type /Filespec /F (b.bin) /EF << /F 6 0 R >> >>"), stream_obj(6, &format!("/Type /EmbeddedFile /Filter {filter}"), data)];
    let refs: String = (0..n).map(|i| format!("{} 0 R ", 10 + i)).collect();
    objs.extend((0..n).map(|i| obj(10 + i, &format!("<< /Type /Annot /Subtype /FileAttachment /Page 0 /Rect [100 600 120 620] /NM (b{i}) /FS 5 0 R >>"))));
    let (eng, id, _) = blank(test);
    let f = fdf_file(&refs, &objs);
    let t = std::time::Instant::now();
    let s = run(&eng, id, &f);
    (s, t.elapsed())
}

#[test]
fn one_bomb_shared_by_many_comments_is_decoded_once() {
    // A file that inflates to 100 MB, shared by 2000 comments: decoded once (stopped at its cap), then refused from
    // what was learned.
    let z = zlib(&vec![0u8; 100 << 20]);
    let (s, took) = shared_file("attach-shared-bomb", 2000, "/FlateDecode", &z);
    assert_eq!(s.added, 0, "{:?}", s.skipped);
    assert_eq!(reasons(&s.skipped), vec![(attach::OVER, 2000)]);
    assert!(took.as_secs() < 2, "{took:?}");
}

#[test]
fn a_white_space_bomb_behind_an_ascii_filter_is_not_decoded_at_all() {
    // Flate then ASCII hex: the white space inflates to gigabytes and the filter returns one byte, which no cap on
    // the result sees. The chain is refused before anything runs, however many comments share it.
    let z = zlib(&vec![b' '; 100 << 20]);
    for filter in ["[/FlateDecode /ASCIIHexDecode]", "[/Fl /AHx]", "[/Fl /A85]", "[/ASCIIHexDecode /ASCIIHexDecode]"] {
        let (s, took) = shared_file("attach-ws-bomb", 2000, filter, &z);
        assert_eq!(s.added, 0, "{filter}: {:?}", s.skipped);
        assert_eq!(reasons(&s.skipped), vec![(attach::DAMAGED, 2000)], "{filter}");
        assert!(took.as_secs() < 2, "{filter}: {took:?}");
    }
}

#[test]
fn filter_arrays_are_checked_to_the_end_and_a_stage_is_charged_where_it_runs() {
    // More than three filters: refused whatever they are (the old check looked at the first 16 names only).
    let long = format!("[{}/DCTDecode]", "/FlateDecode ".repeat(20));
    let (s, _) = shared_file("attach-long-chain", 3, &long, b"x");
    assert_eq!(reasons(&s.skipped), vec![(attach::DAMAGED, 3)]);
    // ASCII hex first, then Flate: the shape that is carried.
    let data = b"hello attached file";
    let hexed = format!("{}>", hex(&zlib(data)));
    let (s, _) = shared_file("attach-ahx-fl", 3, "[/ASCIIHexDecode /FlateDecode]", hexed.as_bytes());
    assert_eq!((s.added, s.files), (3, 3), "{:?}", s.skipped);
}

#[test]
fn a_refused_appearance_shared_by_many_comments_is_charged_at_each_use() {
    // 2 MB of content that ends in `sh` (not carried): decoded once, but each comment copies and scans it, so the
    // allowance (256 MB) ends the work after about 128 of them, not after all.
    let mut content = "0 ".repeat(1 << 20).into_bytes();
    content.extend_from_slice(b" sh");
    let n = 150;
    let mut objs = vec![stream_obj(5, RED_FORM, &content)];
    objs.extend((0..n).map(|i| obj(10 + i, &freetext(i, 5))));
    let refs: String = (0..n).map(|i| format!("{} 0 R ", 10 + i)).collect();
    let (eng, id, _) = blank("ap-shared-refused");
    let s = run(&eng, id, &fdf_file(&refs, &objs));
    assert_eq!((s.added, s.appearances), (n, 0), "{s:?}");
    let count = |w: &str| s.notes.iter().find(|r| r.reason == w).map_or(0, |r| r.count);
    let (full, cut) = (count(ap::UNSUPPORTED), count(ap::NO_ROOM));
    assert_eq!(full + cut, n, "{:?}", s.notes);
    assert!((100..=130).contains(&full) && cut >= 20, "{full} scanned in full, {cut} cut off by the allowance");
}

#[test]
fn many_small_attachments_are_not_held_back_by_the_decode_allowance() {
    let n = 300;
    let refs: String = (0..n).map(|i| format!("{} 0 R ", 10 + 2 * i)).collect();
    let mut objs = Vec::new();
    for i in 0..n {
        let spec = 11 + 2 * i;
        objs.push(obj(10 + 2 * i, &format!("<< /Type /Annot /Subtype /FileAttachment /Page 0 /Rect [100 600 120 620] /NM (s{i}) /FS {spec} 0 R >>")));
        objs.push(obj(spec, &format!("<< /Type /Filespec /F (s{i}.txt) /EF << /F {} 0 R >> >>", 10 + 2 * n + i)));
    }
    for i in 0..n {
        objs.push(stream_obj(10 + 2 * n + i, "/Type /EmbeddedFile", format!("file {i}").as_bytes()));
    }
    let (eng, id, _) = blank("attach-many");
    let s = run(&eng, id, &fdf_file(&refs, &objs));
    assert_eq!((s.added, s.files, s.skipped.len()), (n, n, 0), "{s:?}");
}

#[test]
fn an_embedded_image_codec_is_not_run_to_export_a_file_and_line_breaks_survive_xfdf() {
    // A file stored as a JPEG stream would be exported as pixels: it is left out of the import instead.
    let f = fdf_file(
        "5 0 R",
        &[
            obj(5, "<< /Type /Annot /Subtype /FileAttachment /Page 0 /Rect [100 600 120 620] /NM (j) /FS 6 0 R >>"),
            obj(6, "<< /Type /Filespec /F (p.jpg) /EF << /F 7 0 R >> >>"),
            stream_obj(7, "/Type /EmbeddedFile /Filter /DCTDecode", b"\xFF\xD8\xFF\xE0 not a picture"),
        ],
    );
    let (eng, id, _) = blank("attach-dct");
    let s = run(&eng, id, &f);
    assert_eq!((s.added, reasons(&s.skipped)), (0, vec![(attach::DAMAGED, 1)]), "{s:?}");

    let file = "<xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><annots><fileattachment page=\"0\" rect=\"100,600,120,620\" name=\"n\" file=\"a.txt\"><data mode=\"raw\" encoding=\"hex\" description=\"line one&#10;line two\">41</data></fileattachment></annots></xfdf>";
    let (a, ida, _) = blank("attach-desc-a");
    run(&a, ida, file.as_bytes());
    assert_eq!(read_all(&a, ida)[0].file.as_ref().unwrap().desc, "line one\nline two");
    let (bytes, _) = export(&a, ida, Format::Xfdf, "a.pdf").unwrap();
    let (b, idb, _) = blank("attach-desc-b");
    run(&b, idb, &bytes);
    assert_eq!(read_all(&b, idb)[0].file.as_ref().unwrap().desc, "line one\nline two");
}

#[test]
fn a_font_shared_by_many_appearances_is_charged_for_every_copy() {
    // 3000 comments, each with a form that uses the same font with 1024 widths: each is a copy in memory, so the
    // file's allowance for appearance structure ends it, and the rest get Inkwell's own drawing and are counted.
    let n = 3000;
    let widths = format!("[{}]", "500 ".repeat(1024));
    let refs: String = (0..n).map(|i| format!("{} 0 R ", 10 + i)).collect();
    let mut objs = vec![
        obj(5, &format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 1023 /Widths {widths} >>")),
        stream_obj(6, &format!("{RED_FORM} /Resources << /Font << /F1 5 0 R >> >>"), b"BT /F1 12 Tf (x) Tj ET"),
    ];
    objs.extend((0..n).map(|i| obj(10 + i, &freetext(i, 6))));
    let (eng, id, _) = blank("ap-shared-font");
    let t = std::time::Instant::now();
    let s = run(&eng, id, &fdf_file(&refs, &objs));
    assert_eq!((s.added, s.skipped.len()), (n, 0), "{:?}", s.skipped);
    assert!(s.appearances > 100 && s.appearances < n, "{}", s.appearances);
    assert_eq!(s.notes.iter().map(|r| r.count).sum::<usize>(), n - s.appearances, "{:?}", s.notes);
    assert!(t.elapsed().as_secs() < 60, "{:?}", t.elapsed());
}

#[test]
fn a_file_for_a_comment_already_in_the_document_is_not_replaced_and_says_so() {
    let file = |bytes: &str| {
        format!(r##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><fileattachment page="0" rect="100,600,120,620" name="same" file="a.txt"><data mode="raw" encoding="hex">{bytes}</data></fileattachment></annots></xfdf>"##)
    };
    let (eng, id, _) = blank("attach-kept");
    run(&eng, id, file("41").as_bytes());
    let s = run(&eng, id, file("4242").as_bytes());
    assert_eq!((s.added, s.updated), (0, 1), "{s:?}");
    assert_eq!(reasons(&s.notes), [(attach::KEPT, 1)]);
    assert_eq!(read_all(&eng, id)[0].file.as_ref().unwrap().data, b"A");
}
