//! "Audit space usage": where the bytes of a PDF go.
//!
//! Stream bytes are exact (the stored, still-encoded length). Non-stream objects are only known by their
//! serialized text, so their share is whatever the file has left after the streams, split in proportion to
//! that text. That keeps the categories adding up to the real file size even when objects live in
//! compressed object streams.

use mupdf::pdf::PdfDocument;
use mupdf::pdf::PdfObject;
use serde::Serialize;

use super::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Images,
    Fonts,
    ContentStreams,
    Metadata,
    Structure,
    Other,
}

const CATEGORIES: [Category; 6] = [
    Category::Images,
    Category::Fonts,
    Category::ContentStreams,
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

pub(crate) fn audit(doc: &PdfDocument, file_size: u64) -> Result<Audit, Error> {
    let n = doc.xref_len()?;
    let mut cat = vec![Category::Structure; n];
    let mut set = |xref: Option<usize>, c: Category| {
        if let Some(x) = xref.filter(|&x| x < n) {
            cat[x] = c;
        }
    };

    // Streams that are only recognisable from who points at them.
    let pages = doc.page_count()?;
    for i in 0..pages {
        let Ok(page) = doc.find_page(i) else { continue };
        let Ok(Some(contents)) = page.get_dict("Contents") else { continue };
        if contents.is_array().unwrap_or(false) {
            for part in contents.array_iter()? {
                if let Ok(part) = part {
                    set(indirect_num(&part), Category::ContentStreams);
                }
            }
        } else {
            set(ref_of(&page, "Contents"), Category::ContentStreams);
        }
    }
    if let Ok(trailer) = doc.trailer() {
        set(ref_of(&trailer, "Info"), Category::Metadata);
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
                    set(Some(x), Category::Fonts);
                    for key in ["FontFile", "FontFile2", "FontFile3"] {
                        set(ref_of(&o, key), Category::Fonts);
                    }
                }
                Some("Font") => {
                    set(Some(x), Category::Fonts);
                    for key in ["ToUnicode", "CIDToGIDMap"] {
                        set(ref_of(&o, key), Category::Fonts);
                    }
                }
                Some("Metadata") => set(Some(x), Category::Metadata),
                _ => {}
            }
            // A dictionary that has no /Type but is a font file's parent.
            if o.get_dict("FontFile").ok().flatten().is_some()
                || o.get_dict("FontFile2").ok().flatten().is_some()
                || o.get_dict("FontFile3").ok().flatten().is_some()
            {
                for key in ["FontFile", "FontFile2", "FontFile3"] {
                    set(ref_of(&o, key), Category::Fonts);
                }
            }
        }
        objects.push(Some(o));
    }

    let mut usage: Vec<CategoryUsage> =
        CATEGORIES.iter().map(|&category| CategoryUsage { category, bytes: 0, objects: 0 }).collect();
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
            match (subtype.as_deref(), ty.as_deref()) {
                (Some("Image"), _) => {
                    c = Category::Images;
                    images += 1;
                    let dim = |k: &str| o.get_dict(k).ok().flatten().and_then(|v| v.as_int().ok()).unwrap_or(0);
                    let filter = o.get_dict("Filter").ok().flatten().map(|f| f.to_string()).unwrap_or_default();
                    what = format!("Image {}x{} {}", dim("Width"), dim("Height"), filter.trim_start_matches('/'));
                }
                (Some("Form"), _) => c = Category::ContentStreams,
                (_, Some("ObjStm" | "XRef")) => c = Category::Structure,
                (_, Some("Metadata")) => c = Category::Metadata,
                (_, Some("EmbeddedFile")) => {
                    c = Category::Other;
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

    largest.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    largest.truncate(16);
    Ok(Audit {
        total: file_size,
        categories: usage,
        images,
        fonts,
        largest,
    })
}
