//! Word (.docx) writer: writes the flowing page model (`flow.rs`) as paragraphs, tables and images.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/export/src/lib.rs and zip.rs @ b8ddf71.
//! The HTML writer is left out (Inkwell exports HTML from MuPDF's structured text); the RTF writer is
//! `rtf.rs`. The page model is filled from MuPDF's stext instead of pdfcraft's own engine (see `export.rs`).
#![allow(dead_code)]

use std::io::Write;

use super::flow::{allowed, hex, items, Block, Image, Item, Page, Table};

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}



/// A formatted text run (shared by paragraphs and table cells).
fn run_xml(text: &str, size: f64, bold: bool, italic: bool, color: [f64; 3]) -> String {
    let mut rpr = String::new();
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    if let Some(h) = hex(color) {
        rpr.push_str(&format!("<w:color w:val=\"{h}\"/>"));
    }
    let half_points = if size.is_finite() { (size * 2.0).round().clamp(2.0, 3276.0) as i64 } else { 24 };
    rpr.push_str(&format!("<w:sz w:val=\"{half_points}\"/>"));
    format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(text))
}

/// One Word table: explicit single borders (so it renders without a table style), a grid sized
/// from the detected column edges, `gridSpan` for spanning cells, empty cells for holes.
fn docx_table(t: &Table) -> String {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let widths: Vec<i64> = edges.windows(2).map(|w| ((w[1] - w[0]).max(20.0) * 20.0).round().clamp(60.0, 31680.0) as i64).collect();
    let border = "<w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>";
    let mut s = String::from("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>");
    for b in [
        border,
        "<w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
    ] {
        s.push_str(b);
    }
    s.push_str("</w:tblBorders></w:tblPr><w:tblGrid>");
    for w in &widths {
        s.push_str(&format!("<w:gridCol w:w=\"{w}\"/>"));
    }
    s.push_str("</w:tblGrid>");
    for row in &t.rows {
        s.push_str("<w:tr>");
        for c in row {
            let span = if c.span > 1 { format!("<w:gridSpan w:val=\"{}\"/>", c.span) } else { String::new() };
            let w: i64 = widths.iter().take(c.span.min(widths.len())).sum();
            s.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{span}</w:tcPr><w:p>{}</w:p></w:tc>",
                run_xml(&c.text, c.size, c.bold, c.italic, c.color)
            ));
        }
        // Pad the grid so every row covers all columns (Word rejects short rows).
        let used: usize = row.iter().map(|c| c.span).sum();
        for _ in used..t.cols.len() {
            s.push_str("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>");
        }
        s.push_str("</w:tr>");
    }
    s.push_str("</w:tbl>");
    s
}

/// A Word document (.docx, Office Open XML): Heading 1/2 and Normal paragraphs, images inline at
/// their size on the page, page breaks between pages.
pub fn docx(pages: &[Page], title: &str) -> Vec<u8> {
    // The first page's size, within what Word accepts (0.1 to 22 in; a long receipt is taller),
    // in twips. Margins are 1 in, less on small pages so text keeps room.
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    let (pw, ph) = pages.first().map_or((12240, 15840), |p| (twips(p.width), twips(p.height)));
    let (mx, my) = ((pw / 8).min(1440), (ph / 8).min(1440));
    // Images are at most the text width, in points.
    let text_w = (pw - 2 * mx) as f64 / 20.0;
    let mut body = String::new();
    let mut media: Vec<(String, &Image)> = Vec::new();
    // Word needs a paragraph between adjacent tables and after the last one in the body.
    let mut after_table = false;
    let run = |b: &Block| run_xml(&b.text, b.size, b.bold, b.italic, b.color);
    for it in items(pages) {
        match &it {
            Item::Para(b, lvl) => {
                let style = match lvl {
                    1 => "<w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>",
                    2 => "<w:pPr><w:pStyle w:val=\"Heading2\"/></w:pPr>",
                    _ => "",
                };
                body.push_str(&format!("<w:p>{style}{}</w:p>", run(b)));
            }
            Item::Table(t) => {
                if after_table {
                    body.push_str("<w:p/>");
                }
                body.push_str(&docx_table(t));
            }
            Item::Img(im) => {
                let n = media.len() + 1;
                let name = format!("image{n}.{}", im.ext);
                // Size on the page, in EMU (12700 per point), at most the text width.
                let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
                let k = (text_w / w).min(1.0);
                let (cx, cy) = ((w * k * 12700.0) as i64, (h * k * 12700.0) as i64);
                body.push_str(&format!(
                    "<w:p><w:r><w:drawing><wp:inline><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Picture {n}\"/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"rIdImg{n}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
                ));
                media.push((name, im));
            }
            Item::PageBreak => body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
        }
        after_table = matches!(it, Item::Table(_));
    }
    if after_table {
        body.push_str("<w:p/>");
    }
    let doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"><w:body>{body}\
<w:sectPr><w:pgSz w:w=\"{pw}\" w:h=\"{ph}\"/><w:pgMar w:top=\"{my}\" w:right=\"{mx}\" w:bottom=\"{my}\" w:left=\"{mx}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>",
        my / 2,
        my / 2
    );
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    );
    types.push_str("</Types>");
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";
    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );
    for (i, (name, _)) in media.iter().enumerate() {
        doc_rels.push_str(&format!(
            "<Relationship Id=\"rIdImg{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{name}\"/>",
            i + 1
        ));
    }
    doc_rels.push_str("</Relationships>");
    let styles = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"120\"/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"240\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"200\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
</w:styles>";
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );
    let mut z = Zip::default();
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("word/document.xml", doc.as_bytes(), true);
    z.add("word/styles.xml", styles.as_bytes(), true);
    z.add("word/_rels/document.xml.rels", doc_rels.as_bytes(), true);
    for (name, im) in &media {
        z.add(&format!("word/media/{name}"), &im.bytes, false);
    }
    z.finish()
}



fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Builds a zip archive in memory.
#[derive(Default)]
pub struct Zip {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl Zip {
    /// Add a file. Already-compressed data (images) is stored; the rest is deflated.
    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        let crc = crc32(data);
        let (method, body) = if compress {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            let _ = e.write_all(data);
            (8u16, e.finish().unwrap_or_default())
        } else {
            (0u16, data.to_vec())
        };
        let offset = self.out.len() as u32;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend(sig.to_le_bytes());
            if central {
                h.extend(20u16.to_le_bytes()); // made by
            }
            h.extend(20u16.to_le_bytes()); // version needed
            h.extend(0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend(method.to_le_bytes());
            h.extend([0, 0, 0x21, 0]); // 00:00, 1980-01-01 (a zero date is not a valid DOS date)
            h.extend(crc.to_le_bytes());
            h.extend((body.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes()); // extra
            if central {
                h.extend(0u16.to_le_bytes()); // comment
                h.extend(0u16.to_le_bytes()); // disk
                h.extend(0u16.to_le_bytes()); // internal attributes
                h.extend(0u32.to_le_bytes()); // external attributes
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        self.out.extend(header(0x0403_4b50, false));
        self.out.extend(&body);
        self.central.extend(header(0x0201_4b50, true));
        self.count += 1;
    }

    pub fn finish(mut self) -> Vec<u8> {
        let start = self.out.len() as u32;
        let size = self.central.len() as u32;
        self.out.append(&mut self.central);
        self.out.extend(0x0605_4b50u32.to_le_bytes());
        self.out.extend([0u8; 4]);
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(size.to_le_bytes());
        self.out.extend(start.to_le_bytes());
        self.out.extend(0u16.to_le_bytes());
        self.out
    }
}
