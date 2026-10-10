//! Compare tests on generated fixtures (no binaries in the repo) and on copies of the study samples.
//! Run with `--test-threads=1` (MuPDF contexts, shared render cache).

use super::*;
use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use std::time::Instant;

// ---- a tiny PDF writer ------------------------------------------------------------------------

#[derive(Clone, Default)]
struct PageSpec {
    /// (x, y, text) in PDF user space, 12 pt Helvetica.
    lines: Vec<(f32, f32, String)>,
    rotate: i32,
    crop: Option<[f32; 4]>,
    /// Annotation entries without the surrounding `<< >>`; /Type /Annot and /P are added.
    annots: Vec<String>,
    /// 8x8 RGB image painted at 100,300 200x200; the seed decides the pixels.
    image: Option<u8>,
    /// Invisible (render mode 3) text, an OCR layer.
    invisible: Option<String>,
    /// Filled gray rectangles [x, y, w, h], gray.
    boxes: Vec<([f32; 4], f32)>,
}

#[derive(Clone, Default)]
struct DocSpec {
    pages: Vec<PageSpec>,
    info: Vec<(String, String)>,
    bookmarks: Vec<(String, usize)>,
    attach: Vec<(String, Vec<u8>)>,
    /// (field name, value, page index)
    fields: Vec<(String, String, usize)>,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)")
}

fn build(d: &DocSpec) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = Vec::new();
    let alloc = |objs: &mut Vec<Vec<u8>>| -> usize {
        objs.push(Vec::new());
        objs.len()
    };
    let (cat, pages, font, info) = (alloc(&mut objs), alloc(&mut objs), alloc(&mut objs), alloc(&mut objs));
    let page_ids: Vec<usize> = d.pages.iter().map(|_| alloc(&mut objs)).collect();

    // bookmarks
    let outline_root = (!d.bookmarks.is_empty()).then(|| alloc(&mut objs));
    let bm_ids: Vec<usize> = d.bookmarks.iter().map(|_| alloc(&mut objs)).collect();
    if let Some(root) = outline_root {
        objs[root - 1] = format!("<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>", bm_ids[0], bm_ids[bm_ids.len() - 1], bm_ids.len()).into_bytes();
        for (k, (title, pg)) in d.bookmarks.iter().enumerate() {
            let mut s = format!("<< /Title ({}) /Parent {root} 0 R /Dest [{} 0 R /XYZ 0 792 0]", esc(title), page_ids[*pg]);
            if k > 0 {
                s += &format!(" /Prev {} 0 R", bm_ids[k - 1]);
            }
            if k + 1 < bm_ids.len() {
                s += &format!(" /Next {} 0 R", bm_ids[k + 1]);
            }
            objs[bm_ids[k] - 1] = (s + " >>").into_bytes();
        }
    }

    // attachments
    let mut names = String::new();
    for (name, data) in &d.attach {
        let ef = alloc(&mut objs);
        let fs = alloc(&mut objs);
        let mut s = format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n", data.len()).into_bytes();
        s.extend_from_slice(data);
        s.extend_from_slice(b"\nendstream");
        objs[ef - 1] = s;
        objs[fs - 1] = format!("<< /Type /Filespec /F ({0}) /UF ({0}) /EF << /F {ef} 0 R >> >>", esc(name)).into_bytes();
        names += &format!("({}) {fs} 0 R ", esc(name));
    }

    // fields (widgets)
    let mut widget_ids = Vec::new();
    let mut page_widgets: Vec<Vec<usize>> = vec![vec![]; d.pages.len()];
    for (name, value, pg) in &d.fields {
        let w = alloc(&mut objs);
        objs[w - 1] = format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({}) /V ({}) /Rect [50 {} 250 {}] /F 4 /P {} 0 R >>",
            esc(name),
            esc(value),
            600 - 30 * widget_ids.len(),
            620 - 30 * widget_ids.len(),
            page_ids[*pg]
        )
        .into_bytes();
        widget_ids.push(w);
        page_widgets[*pg].push(w);
    }

    for (pi, p) in d.pages.iter().enumerate() {
        let content = alloc(&mut objs);
        let mut c = String::new();
        for (r, g) in &p.boxes {
            c += &format!("{g} g {} {} {} {} re f 0 g\n", r[0], r[1], r[2], r[3]);
        }
        if p.image.is_some() {
            c += "q 200 0 0 200 100 300 cm /Im0 Do Q\n";
        }
        for (x, y, t) in &p.lines {
            c += &format!("BT /F1 12 Tf {x} {y} Td ({}) Tj ET\n", esc(t));
        }
        if let Some(t) = &p.invisible {
            c += &format!("BT 3 Tr /F1 12 Tf 50 50 Td ({}) Tj ET\n", esc(t));
        }
        objs[content - 1] = format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len() + 1).into_bytes();

        let mut res = String::from("/Font << /F1 3 0 R >>");
        if let Some(seed) = p.image {
            let im = alloc(&mut objs);
            let data: Vec<u8> = (0..8usize * 8 * 3).map(|i| (seed as usize * 37 + i * 11 + (i / 24) * 5) as u8).collect();
            let mut s = b"<< /Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 192 >>\nstream\n".to_vec();
            s.extend_from_slice(&data);
            s.extend_from_slice(b"\nendstream");
            objs[im - 1] = s;
            res += &format!(" /XObject << /Im0 {im} 0 R >>");
        }
        let mut annot_ids: Vec<usize> = page_widgets[pi].clone();
        for a in &p.annots {
            let id = alloc(&mut objs);
            objs[id - 1] = format!("<< /Type /Annot {a} /P {} 0 R >>", page_ids[pi]).into_bytes();
            annot_ids.push(id);
        }
        let mut s = format!("<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] /Resources << {res} >> /Contents {content} 0 R");
        if p.rotate != 0 {
            s += &format!(" /Rotate {}", p.rotate);
        }
        if let Some(c) = p.crop {
            s += &format!(" /CropBox [{} {} {} {}]", c[0], c[1], c[2], c[3]);
        }
        if !annot_ids.is_empty() {
            s += &format!(" /Annots [{}]", annot_ids.iter().map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" "));
        }
        objs[page_ids[pi] - 1] = (s + " >>").into_bytes();
    }

    let kids: String = page_ids.iter().map(|i| format!("{i} 0 R ")).collect();
    objs[pages - 1] = format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", page_ids.len()).into_bytes();
    objs[font - 1] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec();
    objs[info - 1] = format!("<< {} >>", d.info.iter().map(|(k, v)| format!("/{k} ({})", esc(v))).collect::<Vec<_>>().join(" ")).into_bytes();
    let mut c = format!("<< /Type /Catalog /Pages {pages} 0 R");
    if let Some(r) = outline_root {
        c += &format!(" /Outlines {r} 0 R");
    }
    if !names.is_empty() {
        c += &format!(" /Names << /EmbeddedFiles << /Names [{names}] >> >>");
    }
    if !widget_ids.is_empty() {
        c += &format!(" /AcroForm << /Fields [{}] /DA (/F1 12 Tf 0 g) /DR << /Font << /F1 {font} 0 R >> >> >>", widget_ids.iter().map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" "));
    }
    objs[cat - 1] = (c + " >>").into_bytes();

    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offs = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root {cat} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Appends an incremental update that redefines the given objects.
fn append_update(orig: &[u8], redefine: &[(usize, &str)]) -> Vec<u8> {
    let text = String::from_utf8_lossy(orig).into_owned();
    let prev: usize = text.rsplit("startxref").next().unwrap().trim().lines().next().unwrap().trim().parse().unwrap();
    let size: usize = text.rsplit("/Size ").next().unwrap().split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap();
    let root = text.rsplit("/Root ").next().unwrap().split(" R").next().unwrap().to_string() + " R";
    let info = text.rsplit("/Info ").next().unwrap().split(" R").next().unwrap().to_string() + " R";
    let mut out = orig.to_vec();
    let mut entries = Vec::new();
    for (n, body) in redefine {
        entries.push((*n, out.len()));
        out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n");
    for (n, off) in &entries {
        out.extend_from_slice(format!("{n} 1\n{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} /Root {root} /Info {info} /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

// ---- fixtures ---------------------------------------------------------------------------------

const WORDS: &[&str] = &[
    "river", "copper", "lantern", "meadow", "quartz", "harbor", "violet", "anchor", "timber", "orchid", "falcon", "marble", "saffron", "tundra", "ember",
    "glacier", "jasper", "willow", "cobalt", "thistle", "granite", "pepper", "velvet", "summit", "canyon", "driftwood", "pebble", "monsoon", "sparrow",
    "walnut", "beacon", "cinder", "juniper", "kestrel", "lagoon", "nectar", "oyster", "prairie", "quiver", "russet",
];

fn paragraph(seed: usize) -> Vec<(f32, f32, String)> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(12345);
    (0..6)
        .map(|l| {
            let words: Vec<&str> = (0..8)
                .map(|_| {
                    x = x.wrapping_mul(1103515245).wrapping_add(12345);
                    WORDS[(x >> 8) % WORDS.len()]
                })
                .collect();
            (72.0, 700.0 - 24.0 * l as f32, words.join(" "))
        })
        .collect()
}

fn base(n: usize) -> DocSpec {
    DocSpec {
        pages: (0..n)
            .map(|i| {
                let mut lines = vec![(72.0, 740.0, format!("Heading of section {}", i + 1))];
                lines.extend(paragraph(i + 1));
                PageSpec { lines, ..Default::default() }
            })
            .collect(),
        info: vec![("Title".into(), "Quarterly report".into()), ("Author".into(), "Berkay".into())],
        ..Default::default()
    }
}

fn cmp(a: &DocSpec, b: &DocSpec) -> Comparison {
    compare(&build(a), &build(b), &CompareOptions::default()).unwrap()
}

fn kinds(c: &Comparison) -> Vec<ChangeKind> {
    c.changes.iter().map(|c| c.kind).collect()
}

fn labels(c: &Comparison) -> Vec<String> {
    c.changes.iter().map(|c| c.label.clone()).collect()
}

// ---- identical / layout -----------------------------------------------------------------------

#[test]
fn identical_inputs_have_no_changes() {
    let docs = vec![
        base(3),
        {
            let mut d = base(2);
            d.bookmarks = vec![("One".into(), 0), ("Two".into(), 1)];
            d.attach = vec![("notes.txt".into(), b"hello".to_vec())];
            d.fields = vec![("Name".into(), "Ada".into(), 0)];
            d.pages[0].annots = vec!["/Subtype /Highlight /Rect [72 690 200 710] /C [1 1 0] /Contents (nice)".into()];
            d.pages[1].image = Some(3);
            d
        },
    ];
    for d in docs {
        let bytes = build(&d);
        let c = compare(&bytes, &bytes, &CompareOptions::default()).unwrap();
        assert!(c.identical && c.changes.is_empty());
        // equal content, built twice: goes through the full comparison
        let again = compare(&bytes, &build(&d), &CompareOptions::default()).unwrap();
        assert!(again.identical, "{:?}", labels(&again));
    }
    // the engine's generated fixtures too
    let f = crate::engine::fixture::build(&crate::engine::fixture::Fixture { pages: 4, chapter_every: 2, needle_every: 2, lines: 10 });
    assert!(compare(&f, &f.clone(), &CompareOptions::default()).unwrap().identical);
}

#[test]
fn different_byte_layout_gives_no_semantic_changes() {
    let mut d = base(3);
    d.bookmarks = vec![("One".into(), 0), ("Three".into(), 2)];
    d.attach = vec![("notes.txt".into(), b"hello".to_vec())];
    d.pages[1].image = Some(9);
    d.pages[2].annots = vec!["/Subtype /Text /Rect [100 100 120 120] /Contents (remember)".into()];
    let orig = build(&d);

    // (1) full rewrite by MuPDF: renumbered objects, compressed streams, garbage collected
    let doc = PdfDocument::from_bytes(&orig).unwrap();
    let mut opts = PdfWriteOptions::default();
    opts.set_compress(true).set_garbage_level(4).set_clean(false);
    let mut rewritten = Vec::new();
    doc.write_to_with_options(&mut rewritten, opts).unwrap();
    assert_ne!(rewritten, orig);
    let c = compare(&orig, &rewritten, &CompareOptions::default()).unwrap();
    assert!(c.identical, "rewrite: {:?}", labels(&c));

    // (2) incremental update that re-states the font with another key order
    let inc = append_update(&orig, &[(3, "<< /BaseFont /Helvetica /Subtype /Type1 /Type /Font >>")]);
    let c = compare(&orig, &inc, &CompareOptions::default()).unwrap();
    assert!(c.identical, "incremental: {:?}", labels(&c));

    // (3) uncompressed vs ASCII-hex pretty print
    let mut opts = PdfWriteOptions::default();
    opts.set_ascii(true).set_pretty(true).set_decompress(true);
    let mut pretty = Vec::new();
    PdfDocument::from_bytes(&rewritten).unwrap().write_to_with_options(&mut pretty, opts).unwrap();
    let c = compare(&rewritten, &pretty, &CompareOptions::default()).unwrap();
    assert!(c.identical, "pretty: {:?}", labels(&c));
}

// ---- pages ------------------------------------------------------------------------------------

#[test]
fn page_added() {
    let a = base(3);
    let mut b = base(3);
    b.pages.insert(1, PageSpec { lines: vec![(72.0, 700.0, "A brand new page about saffron".into())], ..Default::default() });
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::PageAdded]);
    assert_eq!(c.changes[0].b_page, Some(1));
    assert_eq!(c.changes[0].label, "Page 2 added");
    assert_eq!(c.pages.b_to_a, vec![Some(0), None, Some(1), Some(2)]);
}

#[test]
fn page_removed() {
    let a = base(4);
    let mut b = base(4);
    b.pages.remove(2);
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::PageRemoved]);
    assert_eq!(c.changes[0].label, "Page 3 removed");
    assert_eq!(c.pages.removed_a, vec![2]);
}

#[test]
fn page_replaced_is_one_change() {
    let a = base(3);
    let mut b = base(3);
    b.pages[1] = PageSpec { lines: vec![(72.0, 700.0, "An entirely different page about saffron".into())], ..Default::default() };
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::PageReplaced]);
    assert_eq!((c.changes[0].a_page, c.changes[0].b_page), (Some(1), Some(1)));
    assert_eq!(c.changes[0].label, "Page 2 replaced");
}

#[test]
fn page_moved_is_the_minimal_set() {
    let a = base(5);
    let mut b = base(5);
    let first = b.pages.remove(0);
    b.pages.push(first);
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::PageMoved]);
    assert_eq!(c.changes[0].label, "Page 1 moved to 5");
    assert_eq!(c.changes[0].detail, Detail::PageMoved { from: 0, to: 4 });
}

#[test]
fn moved_and_edited_page_is_still_matched() {
    let a = base(4);
    let mut b = base(4);
    b.pages[2].lines[3].2 = "an entirely different sentence here".into();
    b.pages.swap(0, 3);
    let c = cmp(&a, &b);
    assert!(c.pages.removed_a.is_empty() && c.pages.b_to_a.iter().all(|m| m.is_some()), "{:?}", c.pages);
    let k = kinds(&c);
    assert!(k.contains(&ChangeKind::TextChanged) && k.contains(&ChangeKind::PageMoved), "{:?}", labels(&c));
    assert!(!k.contains(&ChangeKind::PageAdded) && !k.contains(&ChangeKind::PageRemoved));
}

#[test]
fn page_rotated() {
    let a = base(2);
    let mut b = base(2);
    b.pages[1].rotate = 90;
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::PageRotated]);
    assert_eq!(c.changes[0].label, "Page 2 rotated from 0° to 90°");
}

#[test]
fn page_box_changed() {
    let a = base(2);
    let mut b = base(2);
    b.pages[0].crop = Some([36.0, 36.0, 576.0, 756.0]);
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::PageBoxChanged]);
    assert_eq!(c.changes[0].label, "CropBox of page 1 changed");
}

// ---- annotations and forms --------------------------------------------------------------------

const HL: &str = "/Subtype /Highlight /Rect [72 690 200 710] /QuadPoints [72 710 200 710 72 690 200 690] /C [1 1 0]";

#[test]
fn annotation_added_removed_changed() {
    let a = base(4);
    let mut b = base(4);
    b.pages[3].annots = vec![HL.into()];
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::AnnotationAdded]);
    assert_eq!(c.changes[0].label, "Highlight added on p. 4");
    assert_eq!(c.changes[0].b_page, Some(3));

    let c = cmp(&b, &a);
    assert_eq!(kinds(&c), vec![ChangeKind::AnnotationRemoved]);
    assert_eq!(c.changes[0].label, "Highlight removed on p. 4");

    let mut a = base(2);
    a.pages[0].annots = vec![format!("{HL} /Contents (first draft)")];
    let mut b = a.clone();
    b.pages[0].annots = vec!["/Subtype /Highlight /Rect [72 690 200 710] /QuadPoints [72 710 200 710 72 690 200 690] /C [0 1 0] /Contents (final)".into()];
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::AnnotationChanged]);
    assert_eq!(c.changes[0].label, "Highlight changed on p. 1: text 'first draft' -> 'final', colour");
}

#[test]
fn form_field_value_changed() {
    let mut a = base(2);
    a.fields = vec![("Name".into(), "".into(), 0), ("City".into(), "Lund".into(), 1)];
    let mut b = a.clone();
    b.fields[0].1 = "A. Novak".into();
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::FormFieldChanged]);
    assert_eq!(c.changes[0].label, "Form field 'Name' changed on p. 1: (empty) -> 'A. Novak'");
    assert_eq!(c.changes[0].detail, Detail::FormField { name: "Name".into(), old: Some("".into()), new: Some("A. Novak".into()) });
}

// ---- text, images, ocr, redaction -------------------------------------------------------------

#[test]
fn word_change() {
    let mut a = base(3);
    a.pages[1].lines.push((72.0, 500.0, "Signed by: blank on the last day".into()));
    let mut b = a.clone();
    b.pages[1].lines.last_mut().unwrap().2 = "Signed by: A. Novak on the last day".into();
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::TextChanged], "{:?}", labels(&c));
    let ch = &c.changes[0];
    assert_eq!(ch.label, "Text changed on p. 2: 'blank' -> 'A. Novak'");
    assert_eq!((ch.a_page, ch.b_page), (Some(1), Some(1)));
    assert!(ch.rect_a.is_some() && ch.rect_b.is_some());
    let Detail::Text { runs } = &ch.detail else { panic!() };
    let ops: Vec<RunOp> = runs.iter().map(|r| r.op).collect();
    assert_eq!(ops, vec![RunOp::Equal, RunOp::Delete, RunOp::Insert, RunOp::Equal]);
    assert_eq!(runs[1].boxes_a.len(), 1);
    assert_eq!(runs[2].boxes_b.len(), 2);
    assert_eq!(runs[0].boxes_a.len(), runs[0].boxes_b.len());
}

#[test]
fn nearby_edits_are_one_change_far_edits_are_two() {
    let a = base(1);
    let mut b = a.clone();
    // two edits on the same line, two words apart; one on another line
    let words: Vec<String> = b.pages[0].lines[1].2.split(' ').map(String::from).collect();
    let mut w = words.clone();
    w[1] = "AAA".into();
    w[3] = "BBB".into();
    b.pages[0].lines[1].2 = w.join(" ");
    b.pages[0].lines[5].2 = "completely replaced line of text".into();
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::TextChanged, ChangeKind::TextChanged], "{:?}", labels(&c));
}

#[test]
fn image_replaced() {
    let mut a = base(2);
    a.pages[0].image = Some(1);
    let mut b = a.clone();
    b.pages[0].image = Some(2);
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::ImageReplaced], "{:?}", labels(&c));
    assert_eq!(c.changes[0].label, "Image replaced on p. 1");
    assert!(c.changes[0].rect_a.is_some() && c.changes[0].rect_b.is_some());
}

#[test]
fn ocr_layer_added() {
    let mut a = base(1);
    a.pages.push(PageSpec { image: Some(5), ..Default::default() });
    let mut b = a.clone();
    b.pages[1].invisible = Some("scanned words that are now searchable text".into());
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::OcrLayerAdded], "{:?}", labels(&c));
    assert_eq!(c.changes[0].b_page, Some(1));
    assert_eq!(c.changes[0].label, "Text layer (OCR) added on p. 2");
    assert_eq!(c.pages.b_to_a, vec![Some(0), Some(1)]);
}

#[test]
fn redaction_by_removed_mark_and_content() {
    let mut a = base(2);
    a.pages[1].lines.push((100.0, 500.0, "secret number 12345".into()));
    a.pages[1].annots = vec!["/Subtype /Redact /Rect [95 495 260 515] /IC [0 0 0]".into()];
    let mut b = base(2);
    b.pages[1].lines.push((100.0, 500.0, "".into())); // keeps the content layout otherwise identical
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::RedactionApplied], "{:?}", labels(&c));
    assert_eq!(c.changes[0].label, "Redaction applied on p. 2 (3 words removed)");
    assert!(matches!(c.changes[0].detail, Detail::Redaction { via: RedactionVia::Annotation, .. }));
}

#[test]
fn redaction_by_opaque_box() {
    let mut a = base(2);
    a.pages[1].lines.push((100.0, 500.0, "secret number 12345".into()));
    let mut b = base(2);
    b.pages[1].boxes = vec![([95.0, 495.0, 170.0, 20.0], 0.0)];
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::RedactionApplied], "{:?}", labels(&c));
    assert!(matches!(c.changes[0].detail, Detail::Redaction { via: RedactionVia::OpaqueBox, words_removed: 3, .. }));
}

#[test]
fn redaction_applied_by_mupdf() {
    let mut a = base(2);
    a.pages[1].lines.push((100.0, 500.0, "secret number 12345".into()));
    a.pages[1].annots = vec!["/Subtype /Redact /Rect [95 495 260 515]".into()];
    let orig = build(&a);
    let doc = PdfDocument::from_bytes(&orig).unwrap();
    {
        let mut p = doc.load_pdf_page(1).unwrap();
        assert!(p.apply_redactions().unwrap());
    }
    let mut out = Vec::new();
    doc.write_to(&mut out).unwrap();
    let c = compare(&orig, &out, &CompareOptions::default()).unwrap();
    assert_eq!(kinds(&c), vec![ChangeKind::RedactionApplied], "{:?}", labels(&c));
}

#[test]
fn graphics_only_change_is_reported() {
    let a = base(2);
    let mut b = base(2);
    b.pages[0].boxes = vec![([10.0, 10.0, 50.0, 50.0], 0.5)];
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::PageContentChanged], "{:?}", labels(&c));
}

// ---- document level ---------------------------------------------------------------------------

#[test]
fn metadata_bookkeeping_only_is_no_change() {
    let a = base(1);
    let mut b = a.clone();
    for (k, v) in [("ModDate", "D:20261010120000Z"), ("Producer", "Inkwell 0.2"), ("Creator", "Word"), ("Trapped", "False")] {
        b.info.push((k.into(), v.into()));
    }
    let c = cmp(&a, &b);
    assert!(c.changes.is_empty(), "{:?}", labels(&c));
    assert!(c.identical);
}

#[test]
fn empty_metadata_fields_equal_missing_ones() {
    let a = base(1);
    let mut b = a.clone();
    for k in ["Keywords", "Subject"] {
        b.info.push((k.into(), "".into()));
    }
    let c = cmp(&a, &b);
    assert!(c.changes.is_empty(), "{:?}", labels(&c));
}

#[test]
fn metadata_changes_collapse_into_one() {
    let a = base(1);
    let mut b = a.clone();
    b.info[0].1 = "Annual report".into();
    b.info.push(("ModDate".into(), "D:20261010120000Z".into()));
    b.info.push(("Producer".into(), "Inkwell 0.2".into()));
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::MetadataChanged]);
    assert_eq!(c.changes[0].label, "Metadata changed (1)");
    let Detail::Metadata { fields } = &c.changes[0].detail else { panic!() };
    assert_eq!((fields[0].field.as_str(), fields[0].old.as_deref(), fields[0].new.as_deref()), ("Title", Some("Quarterly report"), Some("Annual report")));
    let all = compare(&build(&a), &build(&b), &CompareOptions { include_volatile_metadata: true, ..Default::default() }).unwrap();
    assert_eq!(all.changes[0].label, "Metadata changed (3)");
}

#[test]
fn metadata_noise_rules() {
    use super::types::MetadataSource::{Info, Xmp};
    use super::semantic::is_noise;
    for k in ["xmp:CreatorTool", "xmp:ModifyDate", "xmp:MetadataDate", "pdf:Producer", "pdfx:PTEX.Fullbanner", "xmpMM:DocumentID", "xmpMM:InstanceID", "xmpMM:VersionID", "xmpMM:History", "dc:format", "pdf:Trapped"] {
        assert!(is_noise(Xmp, k, false), "{k}");
    }
    assert!(!is_noise(Xmp, "dc:title", true));
    assert!(!is_noise(Info, "Title", true));
    assert!(is_noise(Xmp, "xmp:CreateDate", true) && !is_noise(Xmp, "xmp:CreateDate", false));
}

#[test]
fn bookmarks_changed() {
    let mut a = base(3);
    a.bookmarks = vec![("Intro".into(), 0), ("Methods".into(), 1)];
    let mut b = a.clone();
    b.bookmarks = vec![("Intro".into(), 0), ("Method".into(), 1), ("Results".into(), 2)];
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::BookmarksChanged]);
    assert_eq!(c.changes[0].label, "Bookmarks changed: 1 added, 1 renamed");
    // a bookmark following a moved page is not a change of the bookmark
    let mut m = a.clone();
    let p = m.pages.remove(0);
    m.pages.push(p);
    m.bookmarks = vec![("Intro".into(), 2), ("Methods".into(), 0)];
    let c = cmp(&a, &m);
    assert_eq!(kinds(&c), vec![ChangeKind::PageMoved], "{:?}", labels(&c));
}

#[test]
fn attachments_changed() {
    let mut a = base(1);
    a.attach = vec![("a.txt".into(), b"one".to_vec()), ("b.txt".into(), b"two".to_vec())];
    let mut b = a.clone();
    b.attach = vec![("a.txt".into(), b"ONE!".to_vec()), ("c.txt".into(), b"three".to_vec())];
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::AttachmentsChanged]);
    assert_eq!(c.changes[0].label, "Attachments: added c.txt; removed b.txt; changed a.txt");
}

#[test]
fn errors_are_clear() {
    let ok = build(&base(1));
    assert_eq!(compare(b"not a pdf", &ok, &CompareOptions::default()).unwrap_err(), DiffError::NotPdf { side: Side::A });
    assert_eq!(compare(&ok, b"%PDF-1.4 garbage", &CompareOptions::default()).unwrap_err(), DiffError::NotPdf { side: Side::B });
    // password protected
    let doc = PdfDocument::from_bytes(&ok).unwrap();
    let mut opts = PdfWriteOptions::default();
    opts.set_encryption(mupdf::pdf::Encryption::Aes256).set_user_password("pw").set_owner_password("owner");
    let mut enc = Vec::new();
    doc.write_to_with_options(&mut enc, opts).unwrap();
    assert_eq!(compare(&ok, &enc, &CompareOptions::default()).unwrap_err(), DiffError::NeedsPassword { side: Side::B });
    let wrong = CompareOptions { password_b: Some("nope".into()), ..Default::default() };
    assert_eq!(compare(&ok, &enc, &wrong).unwrap_err(), DiffError::BadPassword { side: Side::B });
    let right = CompareOptions { password_b: Some("pw".into()), ..Default::default() };
    assert!(compare(&ok, &enc, &right).unwrap().identical);
    let json = serde_json::to_string(&DiffError::NeedsPassword { side: Side::B }).unwrap();
    assert_eq!(json, r#"{"code":"needs-password","side":"b"}"#);
}

#[test]
fn changes_serialise_as_camel_case() {
    let a = base(2);
    let mut b = base(2);
    b.pages[0].lines[2].2 = "different words altogether now please".into();
    let json = serde_json::to_value(cmp(&a, &b)).unwrap();
    assert_eq!(json["changes"][0]["kind"], "textChanged");
    assert_eq!(json["changes"][0]["aPage"], 0);
    assert_eq!(json["changes"][0]["detail"]["type"], "text");
    assert_eq!(json["identical"], false);
    assert!(json["pages"]["bToA"].is_array());
}

// ---- visual -----------------------------------------------------------------------------------

#[test]
fn visual_diff_one_word_gives_one_box() {
    let mut a = base(1);
    a.pages[0].lines.push((72.0, 500.0, "Signed by: blank".into()));
    let mut b = a.clone();
    b.pages[0].lines.last_mut().unwrap().2 = "Signed by: A. Novak".into();
    let (ba, bb) = (build(&a), build(&b));
    let change = &compare(&ba, &bb, &CompareOptions::default()).unwrap().changes[0];
    let (ra, rb) = (change.rect_a.unwrap(), change.rect_b.unwrap());
    let word = [ra[0].min(rb[0]), ra[1].min(rb[1]), ra[2].max(rb[2]), ra[3].max(rb[3])];

    let v = visual_diff(&ba, 0, &bb, 0, 1.5).unwrap();
    assert_eq!((v.width, v.height), (918, 1188));
    assert_eq!(v.boxes.len(), 1, "{:?}", v.boxes);
    let bx = v.boxes[0];
    assert!(bx[0] <= word[0] + 4.0 && bx[1] <= word[1] + 4.0 && bx[2] + 4.0 >= word[2] && bx[3] + 4.0 >= word[3], "box {bx:?} must contain word {word:?}");
    assert!(bx[2] - bx[0] < word[2] - word[0] + 30.0 && bx[3] - bx[1] < word[3] - word[1] + 30.0, "box {bx:?} too loose around {word:?}");
    assert!(v.mask.iter().any(|&m| m == 255) && v.changed_pixels > 0);
    // identical render: nothing
    let same = visual_diff(&ba, 0, &ba, 0, 1.5).unwrap();
    assert!(same.boxes.is_empty() && same.changed_pixels == 0);
    // png export works
    let png = v.before.to_png().unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn visual_diff_ignores_subpixel_jitter() {
    let mut a = base(1);
    a.pages[0].lines.push((72.0, 500.0, "The quick brown fox jumps over the lazy dog".into()));
    a.pages[0].lines.push((72.0, 480.0, "Signed by: blank".into()));
    let mut b = a.clone();
    for l in &mut b.pages[0].lines {
        l.0 += 0.2;
    }
    let (ba, bb) = (build(&a), build(&b));
    let v = visual_diff(&ba, 0, &bb, 0, 1.5).unwrap();
    assert!(v.boxes.is_empty(), "{:?}", v.boxes);
    // a region is kept when a semantic change rect touches it
    let mut c = a.clone();
    c.pages[0].lines[1].2.push_str(" and more");
    let bc = build(&c);
    let strict = visual_diff(&ba, 0, &bc, 0, 1.5).unwrap().boxes.len();
    let hinted = visual_diff_hinted(&ba, 0, &bc, 0, 1.5, &[[72.0, 300.0, 400.0, 320.0]]).unwrap().boxes.len();
    assert!(hinted >= strict && hinted >= 1);
}

#[test]
fn visual_diff_pages_of_different_size_and_errors() {
    let a = build(&base(1));
    let mut spec = base(1);
    spec.pages[0].rotate = 90;
    let b = build(&spec);
    let v = visual_diff(&a, 0, &b, 0, 1.0).unwrap();
    assert_eq!((v.width, v.height), (792, 792));
    assert!(!v.boxes.is_empty());
    assert_eq!(visual_diff(&a, 5, &b, 0, 1.0).unwrap_err(), DiffError::BadPage { page: 5 });
}

#[test]
fn render_is_capped_at_max_pixels() {
    let mut a = base(1);
    a.pages[0].lines.push((72.0, 500.0, "Signed by: blank".into()));
    let mut b = a.clone();
    b.pages[0].lines.last_mut().unwrap().2 = "Signed by: A. Novak".into();
    let (ba, bb) = (build(&a), build(&b));
    // Letter at 12x would be 7344 x 9504 = 70 MP.
    let r = render_page(BytesId::of(&ba), &ba, 0, 12.0).unwrap();
    assert!((r.width as f32) * (r.height as f32) <= visual::MAX_PIXELS, "{} x {}", r.width, r.height);
    assert!(r.scale < 12.0 && (r.width as f32 - 612.0 * r.scale).abs() <= 1.0);
    // The diff reports the effective scale, and its boxes stay in page space.
    let v = visual_diff(&ba, 0, &bb, 0, 12.0).unwrap();
    assert_eq!(v.scale, r.scale);
    let small = visual_diff(&ba, 0, &bb, 0, 1.0).unwrap();
    assert_eq!(v.boxes.len(), small.boxes.len());
    assert!((v.boxes[0][0] - small.boxes[0][0]).abs() < 4.0, "{:?} vs {:?}", v.boxes, small.boxes);
}

#[test]
fn word_diff_has_a_deadline() {
    // Two pages of 30k words with nothing in common: unbounded Myers takes many seconds here.
    let a: Vec<String> = (0..30_000).map(|i| format!("a{i}")).collect();
    let b: Vec<String> = (0..30_000).map(|i| format!("b{i}")).collect();
    let t = Instant::now();
    let ops = semantic::seq_diff(&a, &b);
    assert!(t.elapsed().as_secs_f32() < 2.0, "{:?}", t.elapsed());
    let (del, ins) = ops.iter().fold((0, 0), |(d, i), op| match *op {
        similar::DiffOp::Delete { old_len, .. } => (d + old_len, i),
        similar::DiffOp::Insert { new_len, .. } => (d, i + new_len),
        similar::DiffOp::Replace { old_len, new_len, .. } => (d + old_len, i + new_len),
        similar::DiffOp::Equal { .. } => (d, i),
    });
    assert_eq!((del, ins), (30_000, 30_000), "still a complete diff");
}

#[test]
fn render_cache_is_shared_and_keyed() {
    clear_cache_for_test();
    let a = build(&base(2));
    let id = BytesId::of(&a);
    let r1 = render_page(id, &a, 0, 1.0).unwrap();
    let r2 = render_page(id, &a, 0, 1.0).unwrap();
    assert!(std::sync::Arc::ptr_eq(&r1, &r2));
    let r3 = render_page(id, &a, 1, 1.0).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&r1, &r3));
    let r4 = render_page(id, &a, 0, 2.0).unwrap();
    assert_eq!((r4.width, r4.height), (r1.width * 2, r1.height * 2));
    // from several threads at once
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                for p in 0..2 {
                    render_page(id, &a, p, 1.0).unwrap();
                }
            });
        }
    });
}

fn clear_cache_for_test() {
    visual::clear_cache();
}

#[test]
fn timing_report() {
    // A letter page full of text, one changed word. Numbers are printed; run with --nocapture (and --release for real figures).
    let mut a = base(1);
    for k in 0..30 {
        a.pages[0].lines.push((72.0, 520.0 - 14.0 * k as f32, paragraph(100 + k)[0].2.clone()));
    }
    let mut b = a.clone();
    b.pages[0].lines[10].2 = "a changed line right here in the middle".into();
    let (ba, bb) = (build(&a), build(&b));
    let t = Instant::now();
    let c = compare(&ba, &bb, &CompareOptions::default()).unwrap();
    let t_cmp = t.elapsed();
    visual::clear_cache();
    let t = Instant::now();
    let (ra, rb) = (render_page(BytesId::of(&ba), &ba, 0, 1.5).unwrap(), render_page(BytesId::of(&bb), &bb, 0, 1.5).unwrap());
    let t_render = t.elapsed();
    let t = Instant::now();
    let mut n = 0;
    for _ in 0..10 {
        n += diff_rasters(ra.clone(), rb.clone(), 1.5).boxes.len();
    }
    let t_diff = t.elapsed() / 10;
    println!("TIMING compare(1 page, 2 files)={t_cmp:?} render 2 pages @1.5x={t_render:?} pixel diff (cached renders)={t_diff:?} boxes={}", n / 10);
    assert_eq!(c.changes.len(), 1);
}

// ---- real samples (copies) ----------------------------------------------------------------------

fn sample_copy(name: &str) -> Vec<u8> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/study/samples").join(name);
    let dir = std::env::temp_dir().join("inkwell-diff-samples");
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join(name);
    std::fs::copy(&src, &dst).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    std::fs::read(&dst).unwrap()
}

#[test]
fn real_sample_pairs() {
    let pairs = [
        ("edit-source.pdf", "edit-out-01-typed-ZZ.pdf"),
        ("edit-source.pdf", "edit-out-02-addtext.pdf"),
        ("redact-source.pdf", "red-out-02-apply-sanitize.pdf"),
        ("form-source.pdf", "frm-out-01-autodetect.pdf"),
        ("pages-a.pdf", "pages-b.pdf"),
        ("pages-a.pdf", "org-out-01-ops.pdf"),
        ("ocr-in-01.pdf", "ocr-out-01-english.pdf"),
    ];
    for (a, b) in pairs {
        let (ba, bb) = (sample_copy(a), sample_copy(b));
        let t = Instant::now();
        let c = compare(&ba, &bb, &CompareOptions::default());
        let dt = t.elapsed();
        match c {
            Ok(c) => {
                println!("SAMPLE {a} -> {b}: {} pages -> {} pages, {} changes, identical={} ({dt:?})", c.pages.a_pages, c.pages.b_pages, c.changes.len(), c.identical);
                for ch in c.changes.iter().take(25) {
                    println!("    [{:?}] {}", ch.kind, ch.label);
                }
                if c.changes.len() > 25 {
                    println!("    ... {} more", c.changes.len() - 25);
                }
            }
            Err(e) => println!("SAMPLE {a} -> {b}: ERROR {e}"),
        }
    }
}

/// A form drawn under ten names from its own resources: without the per-page form budget this
/// expands 10^6 times and the compare hangs.
#[test]
fn self_referencing_forms_do_not_explode() {
    let names: String = (0..10).map(|i| format!("/F{i} 4 0 R ")).collect();
    let draws: String = (0..10).map(|i| format!("/F{i} Do ")).collect();
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources 5 0 R /Contents 6 0 R >>"),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources 5 0 R /Length {} >>
stream
{draws}
endstream", draws.len()),
        format!("<< /XObject << {names}>> >>"),
        format!("<< /Length {} >>
stream
{draws}
endstream", draws.len()),
    ];
    let mut pdf = b"%PDF-1.7
".to_vec();
    for (i, o) in objs.iter().enumerate() {
        pdf.extend(format!("{} 0 obj
{o}
endobj
", i + 1).bytes());
    }
    pdf.extend(b"trailer
<< /Root 1 0 R /Size 7 >>
%%EOF
");
    let t = std::time::Instant::now();
    let c = compare(&pdf, &pdf, &CompareOptions::default()).unwrap();
    assert!(c.identical);
    assert!(t.elapsed().as_secs_f32() < 3.0, "took {:?}", t.elapsed());
}
