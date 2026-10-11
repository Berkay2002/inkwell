//! "Audit space usage": where the bytes of a PDF go.
//!
//! Stream bytes are exact (the stored, still-encoded length). Non-stream objects are only known by their
//! serialized text, so their share is whatever the file has left after the streams, split in proportion to
//! that text. That keeps the categories adding up to the real file size even when objects live in
//! compressed object streams.

use mupdf::pdf::PdfDocument;
use mupdf::pdf::PdfObject;
use serde::Serialize;

use super::fonts::{self, FontUsage};
use super::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Images,
    Fonts,
    ContentStreams,
    /// Form fields (widgets, their appearances, the AcroForm dictionary).
    Forms,
    /// Annotations other than links and form fields, with their appearances.
    Comments,
    Links,
    Bookmarks,
    Thumbnails,
    EmbeddedFiles,
    Metadata,
    /// Page tree, resources, the cross-reference table, everything that holds the file together.
    Structure,
    Other,
}

const CATEGORIES: [Category; 12] = [
    Category::Images,
    Category::Fonts,
    Category::ContentStreams,
    Category::Forms,
    Category::Comments,
    Category::Links,
    Category::Bookmarks,
    Category::Thumbnails,
    Category::EmbeddedFiles,
    Category::Metadata,
    Category::Structure,
    Category::Other,
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryUsage {
    pub category: Category,
    pub bytes: u64,
    pub objects: u32,
    /// Share of the file, 0..=100.
    pub percent: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    pub xref: i32,
    pub category: Category,
    pub bytes: u64,
    pub what: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    pub total: u64,
    pub categories: Vec<CategoryUsage>,
    pub images: u32,
    pub fonts: u32,
    /// The largest streams, biggest first.
    pub largest: Vec<AuditItem>,
    /// Every font by name, largest first (what the fonts setting works on).
    pub font_list: Vec<FontUsage>,
}

impl Audit {
    pub fn bytes(&self, c: Category) -> u64 {
        self.categories.iter().find(|u| u.category == c).map_or(0, |u| u.bytes)
    }
}

fn name_of(o: &PdfObject, key: &str) -> Option<String> {
    let v = o.get_dict(key).ok()??;
    String::from_utf8(v.as_name().ok()?).ok()
}

fn indirect_num(o: &PdfObject) -> Option<usize> {
    o.is_indirect().ok()?.then(|| o.as_indirect().ok()).flatten().map(|n| n as usize)
}

/// The xref a dictionary value points at (None when it is a direct object or missing).
fn ref_of(o: &PdfObject, key: &str) -> Option<usize> {
    indirect_num(&o.get_dict(key).ok()??)
}

/// Gives object `xref` a category (an index outside the xref is ignored).
fn mark(cat: &mut [Category], xref: Option<usize>, c: Category) {
    if let Some(x) = xref.filter(|&x| x < cat.len()) {
        cat[x] = c;
    }
}

pub(crate) fn audit(doc: &PdfDocument, file_size: u64) -> Result<Audit, Error> {
    let n = doc.xref_len()?;
    let mut cat = vec![Category::Structure; n];

    // Streams that are only recognisable from who points at them.
    let pages = doc.page_count()?;
    for i in 0..pages {
        let Ok(page) = doc.find_page(i) else { continue };
        let Ok(Some(contents)) = page.get_dict("Contents") else { continue };
        if contents.is_array().unwrap_or(false) {
            for part in contents.array_iter()? {
                if let Ok(part) = part {
                    mark(&mut cat, indirect_num(&part), Category::ContentStreams);
                }
            }
        } else {
            mark(&mut cat, ref_of(&page, "Contents"), Category::ContentStreams);
        }
    }
    for i in 0..pages {
        let Ok(page) = doc.find_page(i) else { continue };
        mark(&mut cat, ref_of(&page, "Thumb"), Category::Thumbnails);
        let Ok(Some(annots)) = page.get_dict("Annots") else { continue };
        if !annots.is_array().unwrap_or(false) {
            continue;
        }
        for a in annots.array_iter()?.flatten() {
            let c = match name_of(&a, "Subtype").as_deref() {
                Some("Widget") => Category::Forms,
                Some("Link") => Category::Links,
                Some(_) => Category::Comments,
                None => continue,
            };
            mark(&mut cat, indirect_num(&a), c);
            // The appearance streams belong to the same annotation.
            let Ok(Some(ap)) = a.get_dict("AP") else { continue };
            for key in ["N", "R", "D"] {
                let Ok(Some(st)) = ap.get_dict(key) else { continue };
                if st.is_stream().unwrap_or(false) {
                    mark(&mut cat, indirect_num(&st), c);
                } else if st.is_dict().unwrap_or(false) {
                    for k in 0..st.dict_len().unwrap_or(0) as i32 {
                        if let Ok(Some(v)) = st.get_dict_val(k) {
                            mark(&mut cat, indirect_num(&v), c);
                        }
                    }
                }
            }
        }
    }
    if let Ok(catalog) = doc.catalog() {
        // Bookmarks: the outline root and every item reachable through First/Next.
        if let Ok(Some(root)) = catalog.get_dict("Outlines") {
            mark(&mut cat, indirect_num(&root), Category::Bookmarks);
            let mut stack = vec![root];
            let mut visited = 0usize;
            while let Some(o) = stack.pop() {
                visited += 1;
                if visited > 100_000 {
                    break;
                }
                for key in ["First", "Next"] {
                    if let Ok(Some(c)) = o.get_dict(key) {
                        if let Some(x) = indirect_num(&c).filter(|&x| x < n && cat[x] != Category::Bookmarks) {
                            cat[x] = Category::Bookmarks;
                            stack.push(c);
                        }
                    }
                }
            }
        }
        // Form fields: the AcroForm dictionary and the field tree (widgets were found above).
        if let Ok(Some(form)) = catalog.get_dict("AcroForm") {
            mark(&mut cat, indirect_num(&form), Category::Forms);
            let mut stack: Vec<PdfObject> = Vec::new();
            if let Ok(Some(fields)) = form.get_dict("Fields") {
                mark(&mut cat, indirect_num(&fields), Category::Forms);
                if fields.is_array().unwrap_or(false) {
                    stack.extend(fields.array_iter()?.flatten());
                }
            }
            let mut visited = 0usize;
            while let Some(f) = stack.pop() {
                visited += 1;
                if visited > 100_000 {
                    break;
                }
                if let Some(x) = indirect_num(&f).filter(|&x| x < n) {
                    if cat[x] == Category::Structure {
                        cat[x] = Category::Forms;
                    }
                }
                if let Ok(Some(kids)) = f.get_dict("Kids") {
                    if kids.is_array().unwrap_or(false) {
                        stack.extend(kids.array_iter()?.flatten());
                    }
                }
            }
        }
    }
    if let Ok(trailer) = doc.trailer() {
        mark(&mut cat, ref_of(&trailer, "Info"), Category::Metadata);
    }
    let mut objects = Vec::with_capacity(n);
    for x in 1..n {
        // The indirect handle, not the resolved dictionary: only it knows about the stream.
        let o = doc.new_indirect(x as i32, 0).ok().filter(|o| matches!(o.resolve(), Ok(Some(r)) if !r.is_null().unwrap_or(true)));
        let Some(o) = o else {
            objects.push(None);
            continue;
        };
        if o.is_dict().unwrap_or(false) || o.is_stream().unwrap_or(false) {
            match name_of(&o, "Type").as_deref() {
                Some("FontDescriptor") => {
                    mark(&mut cat, Some(x), Category::Fonts);
                    for key in ["FontFile", "FontFile2", "FontFile3"] {
                        mark(&mut cat, ref_of(&o, key), Category::Fonts);
                    }
                }
                Some("Font") => {
                    mark(&mut cat, Some(x), Category::Fonts);
                    for key in ["ToUnicode", "CIDToGIDMap"] {
                        mark(&mut cat, ref_of(&o, key), Category::Fonts);
                    }
                }
                Some("Metadata") => mark(&mut cat, Some(x), Category::Metadata),
                Some("Filespec") => {
                    mark(&mut cat, Some(x), Category::EmbeddedFiles);
                    if let Some(ef) = o.get_dict("EF").ok().flatten() {
                        for key in ["F", "UF"] {
                            mark(&mut cat, ref_of(&ef, key), Category::EmbeddedFiles);
                        }
                    }
                }
                _ => {}
            }
            // A dictionary that has no /Type but is a font file's parent.
            if o.get_dict("FontFile").ok().flatten().is_some()
                || o.get_dict("FontFile2").ok().flatten().is_some()
                || o.get_dict("FontFile3").ok().flatten().is_some()
            {
                for key in ["FontFile", "FontFile2", "FontFile3"] {
                    mark(&mut cat, ref_of(&o, key), Category::Fonts);
                }
            }
        }
        objects.push(Some(o));
    }

    let mut usage: Vec<CategoryUsage> =
        CATEGORIES.iter().map(|&category| CategoryUsage { category, bytes: 0, objects: 0, percent: 0.0 }).collect();
    let mut largest: Vec<AuditItem> = Vec::new();
    let (mut images, mut fonts) = (0u32, 0u32);
    let mut stream_bytes = 0u64;
    // (category index, text length) for the non-stream objects.
    let mut loose: Vec<(usize, u64)> = Vec::new();

    for (i, o) in objects.iter().enumerate() {
        let x = i + 1;
        let Some(o) = o else { continue };
        let mut c = cat[x];
        if o.is_stream().unwrap_or(false) {
            let subtype = name_of(o, "Subtype");
            let ty = name_of(o, "Type");
            let mut what = String::new();
            // What the pointing side already found (an annotation's appearance, a thumbnail) wins.
            let pinned = c != Category::Structure && c != Category::Other;
            match (subtype.as_deref(), ty.as_deref()) {
                (Some("Image"), _) => {
                    if !pinned {
                        c = Category::Images;
                    }
                    images += 1;
                    let dim = |k: &str| o.get_dict(k).ok().flatten().and_then(|v| v.as_int().ok()).unwrap_or(0);
                    let filter = o.get_dict("Filter").ok().flatten().map(|f| f.to_string()).unwrap_or_default();
                    what = format!("Image {}x{} {}", dim("Width"), dim("Height"), filter.trim_start_matches('/'));
                }
                (Some("Form"), _) if !pinned => c = Category::ContentStreams,
                (_, Some("ObjStm" | "XRef")) => c = Category::Structure,
                (_, Some("Metadata")) => c = Category::Metadata,
                (_, Some("EmbeddedFile")) => {
                    c = Category::EmbeddedFiles;
                    what = "Embedded file".into();
                }
                _ if c == Category::Structure => c = Category::Other,
                _ => {}
            }
            if c == Category::Fonts {
                fonts += 1;
                what = "Font file".into();
            }
            if what.is_empty() {
                what = format!("{c:?}");
            }
            let len = match o.get_dict("Length").ok().flatten().and_then(|l| l.as_int().ok()) {
                Some(l) if l > 0 => l as u64,
                _ => o.read_raw_stream().map_or(0, |b| b.len() as u64),
            };
            stream_bytes += len;
            let u = &mut usage[CATEGORIES.iter().position(|&k| k == c).unwrap()];
            u.bytes += len;
            u.objects += 1;
            largest.push(AuditItem { xref: x as i32, category: c, bytes: len, what });
        } else {
            let text = o.resolve().ok().flatten().map_or(0, |r| r.to_string().len()) as u64 + 12; // "N 0 obj ... endobj\n"
            loose.push((CATEGORIES.iter().position(|&k| k == c).unwrap(), text));
        }
    }

    // Header, xref table and trailer are part of "structure" too: a classic xref costs 20 B per object.
    let loose_text: u64 = loose.iter().map(|l| l.1).sum::<u64>() + 20 * n as u64;
    let budget = file_size.saturating_sub(stream_bytes);
    let scale = if loose_text == 0 { 0.0 } else { budget as f64 / loose_text as f64 };
    let mut assigned = 0u64;
    for &(ci, text) in &loose {
        let b = (text as f64 * scale) as u64;
        usage[ci].bytes += b;
        usage[ci].objects += 1;
        assigned += b;
    }
    // Rounding, and the xref overhead, go to structure so the total is the file size.
    let structure = CATEGORIES.iter().position(|&k| k == Category::Structure).unwrap();
    usage[structure].bytes += budget.saturating_sub(assigned);

    for u in &mut usage {
        u.percent = if file_size == 0 { 0.0 } else { u.bytes as f64 * 100.0 / file_size as f64 };
    }
    largest.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    largest.truncate(16);
    Ok(Audit {
        total: file_size,
        categories: usage,
        images,
        fonts,
        largest,
        font_list: fonts::list(doc)?,
    })
}
