//! Compare tests on generated fixtures (no binaries in the repo) and on copies of the study samples.
//! Run with `--test-threads=1` (MuPDF contexts, shared render cache).

use super::*;
use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use std::sync::atomic::AtomicBool;
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
    /// Styled lines: (x, y, text, font size, rgb, font resource F1 Helvetica or F2 Times).
    styled: Vec<(f32, f32, String, f32, [f32; 3], &'static str)>,
}

#[derive(Clone, Default)]
struct DocSpec {
    pages: Vec<PageSpec>,
    info: Vec<(String, String)>,
    bookmarks: Vec<(String, usize)>,
    attach: Vec<(String, Vec<u8>)>,
    /// (field name, value, page index)
    fields: Vec<(String, String, usize)>,
    /// Names of the fields that are password fields.
    secret_fields: Vec<String>,
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
            "<< /Type /Annot /Subtype /Widget /FT /Tx{} /T ({}) /V ({}) /Rect [50 {} 250 {}] /F 4 /P {} 0 R >>",
            if d.secret_fields.contains(name) { " /Ff 8192" } else { "" },
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
        for (x, y, t, size, rgb, font) in &p.styled {
            c += &format!("{} {} {} rg BT /{font} {size} Tf {x} {y} Td ({}) Tj ET 0 g\n", rgb[0], rgb[1], rgb[2], esc(t));
        }
        if let Some(t) = &p.invisible {
            c += &format!("BT 3 Tr /F1 12 Tf 50 50 Td ({}) Tj ET\n", esc(t));
        }
        objs[content - 1] = format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len() + 1).into_bytes();

        let mut res = String::from("/Font << /F1 3 0 R /F2 << /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >> >>");
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
        info: vec![("Title".into(), "Quarterly report".into()), ("Author".into(), "Alex".into())],
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

#[test]
fn a_password_field_change_is_reported_without_its_values() {
    let mut a = base(1);
    a.fields = vec![("PIN".into(), "1234".into(), 0), ("City".into(), "Lund".into(), 0)];
    a.secret_fields = vec!["PIN".into()];
    let mut b = a.clone();
    b.fields[0].1 = "9876".into();
    b.fields[1].1 = "Malmo".into();
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::FormFieldChanged, ChangeKind::FormFieldChanged]);
    let pin = c.changes.iter().find(|x| x.label.contains("PIN")).unwrap();
    assert_eq!(pin.label, "Form field 'PIN' (password field) changed on p. 1");
    assert_eq!(pin.detail, Detail::FormField { name: "PIN".into(), old: Some("(password field)".into()), new: Some("(password field)".into()) });
    let all = format!("{:?} {:?}", c.changes.iter().map(|x| &x.label).collect::<Vec<_>>(), c.changes.iter().map(|x| &x.detail).collect::<Vec<_>>());
    assert!(!all.contains("1234") && !all.contains("9876"), "{all}");
    // the other field still shows its values
    assert!(c.changes.iter().any(|x| x.label == "Form field 'City' changed on p. 1: 'Lund' -> 'Malmo'"));
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

// ---- page ranges ------------------------------------------------------------------------------------

fn cmp_in(a: &DocSpec, pages_a: Option<&[u32]>, b: &DocSpec, pages_b: Option<&[u32]>) -> Result<Comparison> {
    let opts = CompareOptions { pages_a: pages_a.map(<[u32]>::to_vec), pages_b: pages_b.map(<[u32]>::to_vec), ..Default::default() };
    compare(&build(a), &build(b), &opts)
}

/// `a` = 8 pages; `b` = 8 pages whose pages 2-4 (0-based 1..=3) are `a`'s pages 4-6 (0-based 3..=5), the middle
/// one edited, and whose first page differs from `a`'s (outside both ranges).
fn shifted() -> (DocSpec, DocSpec) {
    let mut a = base(8);
    a.pages[4].lines.push((72.0, 500.0, "Signed by: blank on the last day".into()));
    let mut b = base(8);
    b.pages[0].lines.push((72.0, 500.0, "Only the first page of the new file says this".into()));
    for k in 0..3 {
        b.pages[1 + k] = a.pages[3 + k].clone();
    }
    b.pages[2].lines.last_mut().unwrap().2 = "Signed by: A. Novak on the last day".into();
    (a, b)
}

#[test]
fn a_range_on_each_side_pairs_the_picked_pages_under_their_own_numbers() {
    let (a, b) = shifted();
    let c = cmp_in(&a, Some(&[3, 4, 5]), &b, Some(&[1, 2, 3])).unwrap();
    assert_eq!(kinds(&c), vec![ChangeKind::TextChanged], "{:?}", labels(&c));
    let ch = &c.changes[0];
    assert_eq!((ch.a_page, ch.b_page), (Some(4), Some(2)));
    assert_eq!(ch.label, "Text changed on p. 3: 'blank' -> 'A. Novak'");
    let m = &c.pages;
    assert_eq!((m.a_pages, m.b_pages), (3, 3));
    assert_eq!((&m.a_ids[..], &m.b_ids[..]), (&[3, 4, 5][..], &[1, 2, 3][..]));
    assert_eq!(m.b_to_a, vec![Some(3), Some(4), Some(5)]);
    assert!(m.removed_a.is_empty());
    // without the ranges the files differ everywhere: the first page of B is no longer outside the comparison
    assert!(cmp(&a, &b).changes.len() > 1);
}

#[test]
fn a_range_on_one_side_only() {
    let (a, b) = shifted();
    // B as a file of its own: a's pages 4-6 (0-based 3..=5), the middle one edited
    let only = DocSpec { pages: b.pages[1..4].to_vec(), info: b.info.clone(), ..Default::default() };
    let c = cmp_in(&a, Some(&[3, 4, 5]), &only, None).unwrap();
    assert_eq!(c.pages.a_ids, vec![3, 4, 5]);
    assert_eq!(c.pages.b_ids, vec![0, 1, 2]);
    assert_eq!(c.pages.b_to_a, vec![Some(3), Some(4), Some(5)]);
    assert_eq!(kinds(&c), vec![ChangeKind::TextChanged], "{:?}", labels(&c));
    assert_eq!((c.changes[0].a_page, c.changes[0].b_page), (Some(4), Some(1)));
    // and the range on the right side only
    let old = DocSpec { pages: a.pages[3..6].to_vec(), info: a.info.clone(), ..Default::default() };
    let c = cmp_in(&old, None, &b, Some(&[1, 2, 3])).unwrap();
    assert_eq!(c.pages.a_ids, vec![0, 1, 2]);
    assert_eq!((c.changes[0].a_page, c.changes[0].b_page), (Some(1), Some(2)));
}

#[test]
fn ranges_of_different_lengths_report_the_unmatched_pages_by_their_numbers() {
    // the same bytes on both sides: the picked pages are what differ
    let a = base(6);
    let c = cmp_in(&a, Some(&[1, 2, 3, 4]), &a, Some(&[1, 2])).unwrap();
    assert_eq!(kinds(&c), vec![ChangeKind::PageRemoved, ChangeKind::PageRemoved]);
    assert_eq!(labels(&c), vec!["Page 4 removed", "Page 5 removed"]);
    assert_eq!(c.pages.removed_a, vec![3, 4]);
    assert_eq!((c.pages.a_pages, c.pages.b_pages), (4, 2));
    assert!(!c.identical);

    // the first pages are not the same pages on the two sides
    let c = cmp_in(&base(3), Some(&[0, 1]), &base(3), Some(&[1, 2])).unwrap();
    assert_eq!(labels(&c), vec!["Page 1 removed", "Page 3 added"]);
    assert_eq!(c.pages.b_to_a, vec![Some(1), None]);
}

#[test]
fn the_same_pages_of_the_same_file_are_identical_and_still_report_which_pages() {
    let a = base(5);
    for pages_b in [&[3, 1][..], &[1, 3, 3][..]] {
        let c = cmp_in(&a, Some(&[3, 1]), &a, Some(pages_b)).unwrap();
        assert!(c.identical);
        assert_eq!(c.pages.a_ids, vec![1, 3]);
        assert_eq!(c.pages.b_to_a, vec![Some(1), Some(3)]);
    }
}

#[test]
fn an_invalid_range_is_refused_with_a_message() {
    let (a, b) = (base(3), base(4));
    let e = cmp_in(&a, Some(&[1, 3]), &b, None).unwrap_err();
    assert_eq!(e, DiffError::PageOutOfRange { side: Side::A, page: 4, count: 3 });
    assert_eq!(e.to_string(), "the first file has 3 pages, so page 4 is out of range");
    let e = cmp_in(&a, None, &b, Some(&[0, 9])).unwrap_err();
    assert_eq!(e.to_string(), "the second file has 4 pages, so page 10 is out of range");
    assert_eq!(cmp_in(&a, Some(&[]), &b, None).unwrap_err(), DiffError::NoPages { side: Side::A });
    // identical bytes take no shortcut around the check
    let e = cmp_in(&a, Some(&[7]), &a, Some(&[7])).unwrap_err();
    assert_eq!(e, DiffError::PageOutOfRange { side: Side::A, page: 8, count: 3 });
    assert_eq!(page_count(&build(&b)).unwrap(), 4);
}

#[test]
fn a_bad_range_on_b_is_refused_before_any_page_of_a_is_read() {
    let (a, b) = (base(40), base(3));
    let opts = CompareOptions { pages_b: Some(vec![5]), ..Default::default() };
    let mut seen = Vec::new();
    let e = compare_with(&build(&a), &build(&b), &opts, &AtomicBool::new(false), &mut |p| seen.push(p)).unwrap_err();
    assert_eq!(e, DiffError::PageOutOfRange { side: Side::B, page: 6, count: 3 });
    assert!(seen.is_empty(), "A was read: {seen:?}");
}

#[test]
fn only_the_picked_pages_are_read_and_counted() {
    let (a, b) = shifted();
    let mut firsts = Vec::new();
    let opts = CompareOptions { pages_a: Some(vec![3, 4, 5]), pages_b: Some(vec![1, 2, 3]), ..Default::default() };
    compare_with(&build(&a), &build(&b), &opts, &AtomicBool::new(false), &mut |p| {
        if p.stage == Stage::First || p.stage == Stage::Second {
            firsts.push((p.stage, p.done, p.total));
        }
    })
    .unwrap();
    assert_eq!(firsts.len(), 6, "{firsts:?}");
    assert!(firsts.iter().all(|f| f.2 == 3), "{firsts:?}");
}

#[test]
fn bookmarks_to_pages_outside_the_range_are_not_compared() {
    let mut a = base(6);
    a.bookmarks = vec![("Intro".into(), 0), ("Results".into(), 4)];
    let mut b = a.clone();
    b.bookmarks = vec![("Intro".into(), 1), ("Results".into(), 4)];
    assert_eq!(kinds(&cmp(&a, &b)), vec![ChangeKind::BookmarksChanged]);
    let c = cmp_in(&a, Some(&[3, 4, 5]), &b, Some(&[3, 4, 5])).unwrap();
    assert!(c.identical, "{:?}", labels(&c));
}

#[test]
fn the_report_of_a_ranged_comparison_names_the_pages_as_the_files_number_them() {
    let (a, b) = shifted();
    let (ba, bb) = (build(&a), build(&b));
    let opts = CompareOptions { pages_a: Some(vec![3, 4, 5]), pages_b: Some(vec![1, 2, 3]), ..Default::default() };
    let c = compare(&ba, &bb, &opts).unwrap();
    let input = report::ReportInput { a: &ba, b: &bb, a_name: "old.pdf", b_name: "new.pdf", date: "12 Oct 2026" };
    let out = report::build(&input, &c, &AtomicBool::new(false), &mut |_| {}).unwrap();
    let (_, text) = pdf_text(&out);
    assert!(text.contains("1 difference"), "{text}");
    assert!(text.contains("Page 3"), "the list is grouped under B's page 3 (0-based 2): {text}");
    assert!(text.contains("Page 5 before, page 3 after"), "the picture page names both: {text}");
    assert!(text.contains("A. Novak"));
    assert!(!text.contains("Page 2 "), "no position in the range leaks into the report: {text}");
}

// ---- formatting, cancel, progress, report ---------------------------------------------------------

fn styled(text: &str, size: f32, rgb: [f32; 3], font: &'static str) -> PageSpec {
    PageSpec { styled: vec![(72.0, 600.0, text.into(), size, rgb, font)], ..Default::default() }
}

#[test]
fn format_change_size_and_colour_is_one_change() {
    let a = DocSpec { pages: vec![styled("The rent is 12400 SEK per month", 12.0, [0.0; 3], "F1")], ..Default::default() };
    let b = DocSpec { pages: vec![styled("The rent is 12400 SEK per month", 16.0, [1.0, 0.0, 0.0], "F1")], ..Default::default() };
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::FormatChanged], "{:?}", labels(&c));
    let ch = &c.changes[0];
    let Detail::Format { text, props } = &ch.detail else { panic!("{:?}", ch.detail) };
    assert_eq!(text, "The rent is 12400 SEK per month");
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["size", "colour"]);
    assert_eq!((props[0].old.as_str(), props[0].new.as_str()), ("12", "16"));
    assert_eq!((props[1].old.as_str(), props[1].new.as_str()), ("#000000", "#ff0000"));
    assert!(ch.rect_a.is_some() && ch.rect_b.is_some());
    assert!(ch.label.starts_with("Formatting changed on p. 1"), "{}", ch.label);
}

#[test]
fn format_change_font() {
    let a = DocSpec { pages: vec![styled("Either party may end this agreement", 12.0, [0.0; 3], "F1")], ..Default::default() };
    let b = DocSpec { pages: vec![styled("Either party may end this agreement", 12.0, [0.0; 3], "F2")], ..Default::default() };
    let c = cmp(&a, &b);
    assert_eq!(kinds(&c), vec![ChangeKind::FormatChanged], "{:?}", labels(&c));
    let Detail::Format { props, .. } = &c.changes[0].detail else { panic!() };
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].name, "font");
    assert_ne!(props[0].old, props[0].new);
}

#[test]
fn text_edit_and_format_change_are_separate_changes() {
    let mut a = DocSpec { pages: vec![styled("Header stays the same here", 12.0, [0.0; 3], "F1")], ..Default::default() };
    a.pages[0].lines = vec![(72.0, 500.0, "Signed by: blank on the last day".into())];
    let mut b = DocSpec { pages: vec![styled("Header stays the same here", 18.0, [0.0; 3], "F1")], ..Default::default() };
    b.pages[0].lines = vec![(72.0, 500.0, "Signed by: A. Novak on the last day".into())];
    let c = cmp(&a, &b);
    let mut k = kinds(&c);
    k.sort();
    assert_eq!(k, vec![ChangeKind::TextChanged, ChangeKind::FormatChanged], "{:?}", labels(&c));
}

#[test]
fn a_font_tweak_does_not_hide_a_graphics_edit() {
    let a = DocSpec { pages: vec![styled("Either party may end this agreement", 12.0, [0.0; 3], "F1")], ..Default::default() };
    let mut b = DocSpec { pages: vec![styled("Either party may end this agreement", 12.0, [0.0; 3], "F2")], ..Default::default() };
    // the font change alone is just the formatting change
    assert_eq!(kinds(&cmp(&a, &b)), vec![ChangeKind::FormatChanged]);
    // but a box added next to it is a change of its own
    b.pages[0].boxes = vec![([10.0, 10.0, 50.0, 50.0], 0.5)];
    let mut k = kinds(&cmp(&a, &b));
    k.sort();
    assert_eq!(k, vec![ChangeKind::FormatChanged, ChangeKind::PageContentChanged]);
}

#[test]
fn a_recolour_with_the_colour_set_outside_the_text_object_is_only_formatting() {
    // `boxes` paint with a grey level set before the text: the same box in both files, text recoloured in B
    let mut a = DocSpec { pages: vec![styled("Payment terms apply", 12.0, [0.0; 3], "F1")], ..Default::default() };
    a.pages[0].boxes = vec![([10.0, 10.0, 50.0, 50.0], 0.5)];
    let mut b = DocSpec { pages: vec![styled("Payment terms apply", 12.0, [0.8, 0.0, 0.0], "F1")], ..Default::default() };
    b.pages[0].boxes = a.pages[0].boxes.clone();
    assert_eq!(kinds(&cmp(&a, &b)), vec![ChangeKind::FormatChanged]);
    // a moved box is still a graphics edit
    b.pages[0].boxes = vec![([10.0, 10.0, 80.0, 50.0], 0.5)];
    let mut k = kinds(&cmp(&a, &b));
    k.sort();
    assert!(k.contains(&ChangeKind::PageContentChanged), "{k:?}");
}

#[test]
fn font_names_that_spell_one_face_differently_are_not_a_format_change() {
    use super::semantic::font_key;
    assert_eq!(font_key("Arial,Bold"), font_key("Arial-BoldMT"));
    assert_eq!(font_key("ArialMT"), font_key("Arial"));
    assert_eq!(font_key("Times New Roman,Italic"), font_key("TimesNewRomanPS-ItalicMT"));
    assert_eq!(font_key("Helvetica-Oblique"), font_key("Helvetica-Italic"));
    assert_ne!(font_key("Arial-Bold"), font_key("Arial"));
    assert_ne!(font_key("Arial"), font_key("Calibri"));
}

#[test]
fn same_style_is_no_format_change() {
    let a = DocSpec { pages: vec![styled("Quiet page", 12.0, [0.2, 0.2, 0.2], "F1")], ..Default::default() };
    let c = cmp(&a, &a.clone());
    assert!(c.identical);
}

#[test]
fn cancelled_comparison_stops_with_a_clear_error() {
    let mut d = base(5);
    d.pages[2].lines[1].2 = "changed here".into();
    let (a, b) = (build(&base(5)), build(&d));
    let stop = std::sync::atomic::AtomicBool::new(true);
    assert_eq!(compare_with(&a, &b, &CompareOptions::default(), &stop, &mut |_| {}), Err(DiffError::Cancelled));
    // cancelled from the progress callback, mid-run
    let flag = std::sync::atomic::AtomicBool::new(false);
    let r = compare_with(&a, &b, &CompareOptions::default(), &flag, &mut |p| {
        if p.stage == Stage::Second {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    assert_eq!(r, Err(DiffError::Cancelled));
}

#[test]
fn progress_walks_the_stages_in_order() {
    let a = build(&base(4));
    let mut d = base(4);
    d.pages[1].lines[2].2 = "an edited line of text".into();
    let b = build(&d);
    let mut seen: Vec<Progress> = Vec::new();
    let c = compare_with(&a, &b, &CompareOptions::default(), &std::sync::atomic::AtomicBool::new(false), &mut |p| seen.push(p)).unwrap();
    assert_eq!(c.changes.len(), 1);
    let order = |s: Stage| match s {
        Stage::First => 0,
        Stage::Second => 1,
        Stage::Pages => 2,
        Stage::Document => 3,
    };
    assert!(seen.windows(2).all(|w| order(w[0].stage) <= order(w[1].stage)), "{seen:?}");
    let first: Vec<&Progress> = seen.iter().filter(|p| p.stage == Stage::First).collect();
    assert_eq!(first.len(), 4);
    assert_eq!((first.last().unwrap().done, first.last().unwrap().total), (4, 4));
    assert!(seen.iter().any(|p| p.stage == Stage::Document));
}

fn pdf_text(bytes: &[u8]) -> (usize, String) {
    let doc = PdfDocument::from_bytes(bytes).unwrap();
    let n = doc.page_count().unwrap() as usize;
    let mut text = String::new();
    for i in 0..n {
        let tp = doc.load_page(i as i32).unwrap().to_text_page(mupdf::TextPageFlags::empty()).unwrap();
        text += &tp.to_text().unwrap();
    }
    (n, text)
}

fn report_of(a: &DocSpec, b: &DocSpec) -> (Comparison, Vec<u8>) {
    let (ba, bb) = (build(a), build(b));
    let c = compare(&ba, &bb, &CompareOptions::default()).unwrap();
    let input = report::ReportInput { a: &ba, b: &bb, a_name: "lease-v1.pdf", b_name: "lease-v2.pdf", date: "11 Oct 2026, 14:32" };
    let out = report::build(&input, &c, &std::sync::atomic::AtomicBool::new(false), &mut |_| {}).unwrap();
    if let Ok(dir) = std::env::var("DUMP_REPORT") {
        std::fs::write(format!("{dir}/report-{}.pdf", c.changes.len()), &out).unwrap();
        for pg in 0..3 {
            if let Ok(r) = render_page(BytesId::of(&out), &out, pg, 1.3) {
                std::fs::write(format!("{dir}/report-{}-p{pg}.png", c.changes.len()), r.to_png().unwrap()).unwrap();
            }
        }
    }
    (c, out)
}

#[test]
fn report_lists_the_changes_and_shows_the_pages() {
    let mut a = base(3);
    a.pages[1].lines.push((72.0, 500.0, "Signed by: blank on the last day".into()));
    let mut b = a.clone();
    b.pages[1].lines.last_mut().unwrap().2 = "Signed by: A. Novak on the last day".into();
    b.pages.push(PageSpec { lines: vec![(72.0, 700.0, "An appendix about saffron".into())], ..Default::default() });
    let (c, out) = report_of(&a, &b);
    assert_eq!(c.changes.len(), 2);
    let (n, text) = pdf_text(&out);
    // summary + list on page 1, then one picture page per changed page (2 and the appendix)
    assert_eq!(n, 3, "{text}");
    for want in ["Comparison report", "lease-v1.pdf", "lease-v2.pdf", "11 Oct 2026", "2 differences found", "Text", "Pages", "A. Novak", "Page 4 added"] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(text.contains("Before") && text.contains("After"));
    // the report is itself a document that opens and renders
    let again = compare(&out, &out.clone(), &CompareOptions::default()).unwrap();
    assert!(again.identical);
    let r = render_page(BytesId::of(&out), &out, 2, 1.0).unwrap();
    assert!(r.rgba.chunks_exact(4).any(|p| p[0] < 200), "the picture page has content");
}

#[test]
fn report_of_identical_files_says_so() {
    let a = base(2);
    let (c, out) = report_of(&a, &a.clone());
    assert!(c.identical);
    let (n, text) = pdf_text(&out);
    assert_eq!(n, 1);
    assert!(text.contains("No differences"), "{text}");
}

#[test]
fn report_with_many_changes_paginates_and_caps_pictures() {
    let a = base(120);
    let mut b = a.clone();
    for p in b.pages.iter_mut() {
        p.lines[2].2 = "completely different words on this line of text".into();
    }
    let (c, out) = report_of(&a, &b);
    assert_eq!(c.changes.len(), 120);
    let (n, text) = pdf_text(&out);
    assert!(n > 60 && n < 130, "{n} pages");
    assert!(text.contains(&format!("first {} of 120 changed pages", report::MAX_PICTURE_PAGES)), "cap note missing");
}

#[test]
fn report_can_be_cancelled() {
    let (ba, bb) = (build(&base(2)), build(&base(3)));
    let c = compare(&ba, &bb, &CompareOptions::default()).unwrap();
    let input = report::ReportInput { a: &ba, b: &bb, a_name: "a.pdf", b_name: "b.pdf", date: "today" };
    let r = report::build(&input, &c, &std::sync::atomic::AtomicBool::new(true), &mut |_| {});
    assert_eq!(r, Err(DiffError::Cancelled));
}

#[test]
fn five_hundred_pages_compare_report_and_cancel() {
    let a = base(500);
    let mut b = a.clone();
    for k in (0..500).step_by(25) {
        b.pages[k].lines[3].2 = format!("this line was rewritten on page {k}");
    }
    b.pages.insert(250, PageSpec { lines: vec![(72.0, 700.0, "An inserted page about saffron".into())], ..Default::default() });
    let (ba, bb) = (build(&a), build(&b));
    let t = Instant::now();
    let mut ticks = 0u32;
    let c = compare_with(&ba, &bb, &CompareOptions::default(), &std::sync::atomic::AtomicBool::new(false), &mut |_| ticks += 1).unwrap();
    let t_cmp = t.elapsed();
    assert_eq!(c.changes.len(), 20 + 1, "{:?}", labels(&c));
    assert!(ticks >= 1000, "progress is reported per page ({ticks})");
    let t = Instant::now();
    let input = report::ReportInput { a: &ba, b: &bb, a_name: "a.pdf", b_name: "b.pdf", date: "today" };
    let out = report::build(&input, &c, &std::sync::atomic::AtomicBool::new(false), &mut |_| {}).unwrap();
    let png = render_page(BytesId::of(&ba), &ba, 0, 1.0).unwrap().to_png().unwrap().len();
    println!("TIMING 500 pages: compare={t_cmp:?} report={:?} ({} KB, one png {png} B)", t.elapsed(), out.len() / 1024);
    // a cancel raised while the first file is read stops the run well before it would have finished
    let flag = std::sync::atomic::AtomicBool::new(false);
    let t = Instant::now();
    let r = compare_with(&ba, &bb, &CompareOptions::default(), &flag, &mut |p| {
        if p.stage == Stage::First && p.done == 10 {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    assert_eq!(r, Err(DiffError::Cancelled));
    assert!(t.elapsed() < t_cmp, "cancel took {:?} vs full {t_cmp:?}", t.elapsed());
}

/// `INKWELL_SEED_DIR=<dir> cargo test --lib seed_compare_screens -- --ignored`. Writes the pairs scripts/compare-shots.mjs
/// compares: `Lease v1.pdf` / `Lease v2.pdf` (text, formatting, image, added page, annotation) and `Big v1.pdf` / `Big v2.pdf`
/// (1200 pages, 48 edited) for progress and cancel.
#[test]
#[ignore]
fn seed_compare_screens() {
    let dir = std::path::PathBuf::from(std::env::var("INKWELL_SEED_DIR").expect("INKWELL_SEED_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut v1 = base(4);
    v1.pages[1].lines = vec![
        (72.0, 740.0, "2. PARTIES".into()),
        (72.0, 700.0, "This Residential Lease Agreement is made on 1 October 2026 between Maple Property Management".into()),
        (72.0, 684.0, "AB (the Landlord) and the Tenant named below.".into()),
        (72.0, 640.0, "2.3 Rent".into()),
        (72.0, 620.0, "The rent is 12 400 SEK per month, due on the 25th of each month.".into()),
        (72.0, 580.0, "2.4 Notice".into()),
        (72.0, 560.0, "Either party may end this agreement with three months written notice.".into()),
    ];
    v1.pages[1].styled = vec![(72.0, 500.0, "Payment terms".into(), 12.0, [0.0, 0.0, 0.0], "F1")];
    v1.pages[2].image = Some(40);
    let mut v2 = v1.clone();
    v2.pages[1].lines[4].2 = "The rent is 12 900 SEK per month, due on the 1st of each month.".into();
    v2.pages[1].lines[6].2 = "Either party may end this agreement with two months written notice.".into();
    v2.pages[1].styled = vec![(72.0, 500.0, "Payment terms".into(), 14.0, [0.18, 0.29, 0.56], "F2")];
    v2.pages[2].image = Some(200);
    v2.pages.push(PageSpec { lines: vec![(72.0, 740.0, "Signature page".into()), (72.0, 700.0, "Signed by the Tenant and the Landlord.".into())], ..Default::default() });
    std::fs::write(dir.join("Lease v1.pdf"), build(&v1)).unwrap();
    std::fs::write(dir.join("Lease v2.pdf"), build(&v2)).unwrap();
    let big1 = base(1200);
    let mut big2 = big1.clone();
    for k in (0..1200).step_by(25) {
        big2.pages[k].lines[3].2 = format!("this line was rewritten on page {}", k + 1);
    }
    std::fs::write(dir.join("Big v1.pdf"), build(&big1)).unwrap();
    std::fs::write(dir.join("Big v2.pdf"), build(&big2)).unwrap();
}
