//! Deterministic PDF fixtures for tests and benchmarks (no binaries in the repo).

use std::fmt::Write;

pub struct Fixture {
    pub pages: usize,
    /// One bookmark per this many pages (0 = no outline).
    pub chapter_every: usize,
    /// Every n-th page also contains the word "needle" (0 = never).
    pub needle_every: usize,
    /// Body lines per page.
    pub lines: usize,
}

impl Default for Fixture {
    fn default() -> Self {
        Self { pages: 3, chapter_every: 0, needle_every: 0, lines: 12 }
    }
}

/// Page size in points of every fixture page.
pub const PAGE_W: f32 = 612.0;
pub const PAGE_H: f32 = 792.0;

pub fn needle_on(f: &Fixture, page: usize) -> bool {
    f.needle_every != 0 && page % f.needle_every == 0
}

pub fn build(f: &Fixture) -> Vec<u8> {
    // Object numbers: 1 catalog, 2 pages, 3 font, 4 outline root, then per page (page, content),
    // then outline items.
    let first_page_obj = 5usize;
    let page_obj = |i: usize| first_page_obj + i * 2;
    let content_obj = |i: usize| first_page_obj + i * 2 + 1;
    let outline_base = first_page_obj + f.pages * 2;
    let chapters = if f.chapter_every == 0 { 0 } else { f.pages.div_ceil(f.chapter_every) };

    let mut objs: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut cat = String::from("<< /Type /Catalog /Pages 2 0 R");
    if chapters > 0 {
        cat.push_str(" /Outlines 4 0 R /PageMode /UseOutlines");
    }
    cat.push_str(" >>");
    objs.push((1, cat.into_bytes()));

    let kids: String = (0..f.pages).map(|i| format!("{} 0 R ", page_obj(i))).collect();
    objs.push((2, format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", f.pages).into_bytes()));
    objs.push((3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()));

    if chapters > 0 {
        let first = outline_base;
        let last = outline_base + chapters - 1;
        objs.push((4, format!("<< /Type /Outlines /First {first} 0 R /Last {last} 0 R /Count {chapters} >>").into_bytes()));
        for c in 0..chapters {
            let mut d = format!(
                "<< /Title (Chapter {}) /Parent 4 0 R /Dest [{} 0 R /XYZ 0 {} 0]",
                c + 1,
                page_obj(c * f.chapter_every),
                PAGE_H as i32
            );
            if c > 0 {
                let _ = write!(d, " /Prev {} 0 R", outline_base + c - 1);
            }
            if c + 1 < chapters {
                let _ = write!(d, " /Next {} 0 R", outline_base + c + 1);
            }
            d.push_str(" >>");
            objs.push((outline_base + c, d.into_bytes()));
        }
    } else {
        objs.push((4, b"<< /Type /Outlines /Count 0 >>".to_vec()));
    }

    for i in 0..f.pages {
        objs.push((
            page_obj(i),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                content_obj(i)
            )
            .into_bytes(),
        ));
        let mut s = String::new();
        // Vector content so tiles are not blank: a tinted band and a rule.
        let shade = (i % 10) as f32 / 12.0;
        let _ = writeln!(s, "{shade:.2} 0.45 0.9 rg 36 700 540 56 re f");
        let _ = writeln!(s, "0 g 36 690 m 576 690 l S");
        let _ = writeln!(s, "BT /F1 28 Tf 48 716 Td (Inkwell fixture page {}) Tj ET", i + 1);
        for l in 0..f.lines {
            let y = 660 - l as i32 * 22;
            let extra = if l == 3 && needle_on(f, i) { " needle" } else { "" };
            let _ = writeln!(
                s,
                "BT /F1 12 Tf 48 {y} Td (The quick brown fox jumps over the lazy dog, line {l} of page {}{extra}.) Tj ET",
                i + 1
            );
        }
        let mut c = format!("<< /Length {} >>\nstream\n", s.len()).into_bytes();
        c.extend_from_slice(s.as_bytes());
        c.extend_from_slice(b"endstream");
        objs.push((content_obj(i), c));
    }

    objs.sort_by_key(|(n, _)| *n);
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0usize; objs.len() + 1];
    for (n, body) in &objs {
        offsets[*n] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let _ = write!(out_str(&mut out), "xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for o in offsets.iter().skip(1) {
        let _ = write!(out_str(&mut out), "{o:010} 00000 n \n");
    }
    let _ = write!(
        out_str(&mut out),
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    );
    out
}

/// Tiny adaptor so `write!` can append to a byte vector.
fn out_str(v: &mut Vec<u8>) -> impl Write + '_ {
    struct W<'a>(&'a mut Vec<u8>);
    impl Write for W<'_> {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }
    W(v)
}
