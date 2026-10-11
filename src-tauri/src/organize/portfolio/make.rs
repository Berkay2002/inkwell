//! Creating a portfolio from files and extracting a portfolio's files to a folder.

use super::{fill_ci, resolved_dict, size_text};
use crate::engine::edit::{Readback, rewrite_opts, write_atomic, write_doc};
use crate::engine::Engine;
use crate::organize::attach::{self, AttachKind, MAX_ATTACH, NewFile, Texts, clean_name, entry_ids, stream_of, tree, unique};
use crate::organize::combine::Face;
use crate::organize::{Result, bad, put_int, put_name, put_str};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Font, Size};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Longest file name written by Extract, in characters; and in UTF-16 units and UTF-8 bytes, which
/// no file system takes more than 255 of (a little room is left for what a clean-up may put in front).
const MAX_NAME: usize = 200;
const MAX_UNITS: usize = 250;
/// Most files, and most bytes, a new portfolio takes.
pub const CREATE_MAX_FILES: usize = 500;
pub const CREATE_MAX_BYTES: u64 = 2 << 30;
/// Most decoded bytes Extract all writes in one go.
const EXTRACT_MAX_BYTES: u64 = 4 << 30;

// ---- create -------------------------------------------------------------------------------------

/// A file to put in a new portfolio: the name it is shown by and where its bytes are.
pub struct Source {
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub path: String,
    pub files: u32,
}

const COVER: (f32, f32) = (595.0, 842.0);
const MARGIN: f32 = 56.0;
const LEADING: f32 = 18.0;
const HEAD: f32 = 120.0;
const SIZE: f32 = 11.0;

/// Cover pages: what is inside, for readers that ignore /Collection. Returns how many pages it made.
pub(super) fn write_cover(doc: &mut PdfDocument, rows: &[(String, u64)], total: u64) -> Result<u32> {
    let (w, h) = COVER;
    let face = Face { font: Font::new("Helvetica")? };
    let font_obj = doc.add_font(&face.font)?;
    let first = (((h - 2.0 * MARGIN - HEAD) / LEADING).floor() as usize).max(1);
    let next = (((h - 2.0 * MARGIN) / LEADING).floor() as usize).max(1);
    let mut pages: Vec<&[(String, u64)]> = Vec::new();
    let (a, rest) = rows.split_at(rows.len().min(first));
    pages.push(a);
    pages.extend(rest.chunks(next));
    let count = pages.len() as u32;
    for (n, chunk) in pages.into_iter().enumerate() {
        let mut c = String::new();
        let mut y = h - MARGIN;
        if n == 0 {
            c.push_str(&format!("BT /F1 26 Tf {MARGIN} {:.2} Td <{}> Tj ET\n", y - 26.0, face.hex("PDF Portfolio")));
            let line = format!("{} {}, {}", rows.len(), if rows.len() == 1 { "file" } else { "files" }, size_text(total));
            c.push_str(&format!("BT /F1 13 Tf {MARGIN} {:.2} Td <{}> Tj ET\n", y - 52.0, face.hex(&line)));
            let note = "Open this file in a PDF reader that supports PDF Portfolios to browse the files inside.";
            c.push_str(&format!("BT /F1 {SIZE} Tf 0.4 g {MARGIN} {:.2} Td <{}> Tj ET\n", y - 74.0, face.hex(&face.fit(note, SIZE, w - 2.0 * MARGIN))));
            y -= HEAD;
        }
        c.push_str("0 g\n");
        for (name, size) in chunk {
            y -= LEADING;
            let size_s = size_text(*size);
            let sw = face.width(&size_s, SIZE);
            let shown = face.fit(name, SIZE, w - 2.0 * MARGIN - sw - 24.0);
            c.push_str(&format!("BT /F1 {SIZE} Tf {MARGIN} {y:.2} Td <{}> Tj ET\n", face.hex(&shown)));
            c.push_str(&format!("BT /F1 {SIZE} Tf {:.2} {y:.2} Td <{}> Tj ET\n", w - MARGIN - sw, face.hex(&size_s)));
        }
        let mut page = doc.new_page_at(n as i32, Size::new(w, h))?;
        let mut fonts = doc.new_dict()?;
        fonts.dict_put("F1", font_obj.clone())?;
        let mut res = doc.new_dict()?;
        res.dict_put("Font", fonts)?;
        page.object().dict_put("Resources", res)?;
        page.insert_contents(doc, c.as_bytes(), true)?;
    }
    Ok(count)
}

/// The schema every Inkwell portfolio gets: (key, shown name, subtype).
const SCHEMA: [(&str, &str, &str); 4] = [("Name", "Name", "F"), ("Description", "Description", "Desc"), ("Size", "Size", "Size"), ("Modified", "Modified", "ModDate")];

fn write_collection(doc: &mut PdfDocument, initial: Option<&str>) -> Result<()> {
    let mut schema = doc.new_dict()?;
    schema.dict_put("Type", PdfObject::new_name("CollectionSchema")?)?;
    for (i, (key, name, sub)) in SCHEMA.iter().enumerate() {
        let mut f = doc.new_dict()?;
        f.dict_put("Type", PdfObject::new_name("CollectionField")?)?;
        put_name(&mut f, "Subtype", sub)?;
        put_str(&mut f, "N", name)?;
        put_int(&mut f, "O", i as i32)?;
        f.dict_put("V", PdfObject::new_bool(true))?;
        schema.dict_put(*key, f)?;
    }
    let mut sort = doc.new_dict()?;
    sort.dict_put("Type", PdfObject::new_name("CollectionSort")?)?;
    put_name(&mut sort, "S", SCHEMA[0].0)?;
    sort.dict_put("A", PdfObject::new_bool(true))?;
    let mut coll = doc.new_dict()?;
    coll.dict_put("Type", PdfObject::new_name("Collection")?)?;
    coll.dict_put("Schema", schema)?;
    put_name(&mut coll, "View", "D")?;
    coll.dict_put("Sort", sort)?;
    if let Some(d) = initial {
        put_str(&mut coll, "D", d)?;
    }
    let coll = doc.add_object(&coll)?;
    doc.catalog()?.dict_put("Collection", coll)?;
    Ok(())
}

/// A new portfolio at `out` from the files `sources`: a cover page listing them, every file embedded
/// with its size and time, the collection (Name, Description, Size, Modified; sorted by name; the
/// first PDF opens first). The files are only read.
pub fn create(eng: &Engine, sources: &[Source], out: &str) -> Result<Created> {
    if sources.is_empty() {
        return Err(bad("choose at least one file"));
    }
    if sources.len() > CREATE_MAX_FILES {
        return Err(bad(format!("a portfolio takes up to {CREATE_MAX_FILES} files here, {} were chosen", sources.len())));
    }
    eng.check_not_open(out, None)?;
    // Sizes first: nothing is read until the whole set is known to fit.
    // An existing target by its real spelling (a case-insensitive disk names "A.pdf" and "a.pdf" one file).
    let out_at = {
        let o = Path::new(out);
        std::fs::canonicalize(o).ok().or_else(|| o.parent().and_then(|p| std::fs::canonicalize(p).ok()).zip(o.file_name()).map(|(p, n)| p.join(n)))
    };
    let mut sizes = Vec::with_capacity(sources.len());
    let mut total = 0u64;
    for s in sources {
        let meta = std::fs::metadata(&s.path).map_err(|e| bad(format!("{}: {e}", s.name)))?;
        if !meta.is_file() {
            return Err(bad(format!("{} is not a file", s.name)));
        }
        if meta.len() > MAX_ATTACH {
            return Err(bad(format!("{} is over {} MB, too large for a portfolio", s.name, MAX_ATTACH >> 20)));
        }
        if out_at.is_some() && std::fs::canonicalize(&s.path).ok() == out_at {
            return Err(bad(format!("{} is the file being written; choose another name", s.name)));
        }
        total += meta.len();
        sizes.push(meta.len());
    }
    if total > CREATE_MAX_BYTES {
        return Err(bad(format!("the files add up to {} MB, over the {} MB a portfolio takes here", total >> 20, CREATE_MAX_BYTES >> 20)));
    }

    // The names the files will have (made unique the way attaching does), for the cover.
    let mut taken: HashSet<String> = HashSet::new();
    let names: Vec<String> = sources
        .iter()
        .map(|s| {
            let n = unique(&taken, &clean_name(&s.name));
            taken.insert(n.to_lowercase());
            n
        })
        .collect();
    let rows: Vec<(String, u64)> = names.iter().cloned().zip(sizes.iter().copied()).collect();
    let mut doc = PdfDocument::new();
    let cover_pages = write_cover(&mut doc, &rows, total)?;

    let mut keys = Vec::with_capacity(sources.len());
    let mut read_total = 0u64;
    for s in sources {
        // Read at most one byte past the limit: a file that grew since it was measured is refused, not slurped.
        let data = (|| {
            use std::io::Read;
            let mut data = Vec::new();
            std::fs::File::open(&s.path)?.take(MAX_ATTACH + 1).read_to_end(&mut data)?;
            std::io::Result::Ok(data)
        })()
        .map_err(|e| bad(format!("{}: {e}", s.name)))?;
        if data.len() as u64 > MAX_ATTACH {
            return Err(bad(format!("{} is over {} MB, too large for a portfolio", s.name, MAX_ATTACH >> 20)));
        }
        // What was measured can have grown: the bytes actually read count against the total too.
        read_total += data.len() as u64;
        if read_total > CREATE_MAX_BYTES {
            return Err(bad(format!("the files add up to over {} MB, more than a portfolio takes here", CREATE_MAX_BYTES >> 20)));
        }
        let modified = std::fs::metadata(&s.path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        let key = attach::add_one(&mut doc, &NewFile { name: s.name.clone(), data, description: String::new(), modified })?;
        keys.push(key);
    }
    let initial = keys.iter().find(|k| k.to_lowercase().ends_with(".pdf")).cloned();
    write_collection(&mut doc, initial.as_deref())?;
    fill_ci(&mut doc, &keys)?;
    for e in tree(&doc)? {
        if let Some(mut spec) = resolved_dict(Some(e.spec)) {
            put_name(&mut spec, "AFRelationship", "Unspecified")?;
        }
    }

    let bytes = write_doc(&doc, rewrite_opts(), 1 << 20)?;
    drop(doc);
    write_atomic(Path::new(out), &bytes, cover_pages, Readback::default())?;
    Ok(Created { path: out.to_string(), files: keys.len() as u32 })
}

// ---- extract ------------------------------------------------------------------------------------

/// `stem` cut so that stem, `suffix` and `ext` together are within [`MAX_NAME`] characters and
/// [`MAX_UNITS`] UTF-16 units and UTF-8 bytes; the three joined.
fn fit(stem: &str, suffix: &str, ext: &str) -> String {
    let (mut chars, mut u16s, mut bytes) = (0, 0, 0);
    for c in suffix.chars().chain(ext.chars()) {
        chars += 1;
        u16s += c.len_utf16();
        bytes += c.len_utf8();
    }
    let mut cut = String::new();
    for c in stem.chars() {
        if chars + 1 > MAX_NAME || u16s + c.len_utf16() > MAX_UNITS || bytes + c.len_utf8() > MAX_UNITS {
            break;
        }
        chars += 1;
        u16s += c.len_utf16();
        bytes += c.len_utf8();
        cut.push(c);
    }
    format!("{}{suffix}{ext}", if cut.len() < stem.len() { cut.trim_end() } else { &cut })
}

/// The stem and extension of `name` (an extension of up to 20 characters counts as one).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name[i..].chars().count() <= 20 => name.split_at(i),
        _ => (name, ""),
    }
}

/// A name for a file taken out of a document: from [`clean_name`], and short enough for any file
/// system (its extension kept).
pub fn extract_name(name: &str) -> String {
    let name = clean_name(name);
    let (stem, ext) = split_ext(&name);
    clean_name(&fit(stem, "", ext))
}

/// Writes `bytes` as a new file in `dir` named `name`, or `name (2)` and so on when that exists: a file
/// is never replaced. `next` remembers, for each name, the number to try first, so a run of files with
/// one name does not start over every time. Returns the name it got.
fn write_new(dir: &Path, name: &str, bytes: &[u8], next: &mut HashMap<String, u32>) -> std::io::Result<String> {
    use std::io::Write;
    let (stem, ext) = split_ext(name);
    let start = next.get(&name.to_lowercase()).copied().unwrap_or(1);
    for n in start..start + 10_000 {
        // The number goes in before the cut, so a name at the length limit still gets a different name.
        let candidate = if n == 1 { fit(stem, "", ext) } else { fit(stem, &format!(" ({n})"), ext) };
        let path = dir.join(&candidate);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let r = f.write_all(bytes).and_then(|()| f.sync_all());
                if let Err(e) = r {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                next.insert(name.to_lowercase(), n + 1);
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files of that name"))
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Extracted {
    /// The names written (after clash and sanitising), in the folder.
    pub written: Vec<String>,
    /// One line for each file left out, with the reason.
    pub skipped: Vec<String>,
}

/// Lines of `skipped` kept; the rest are counted.
const MAX_SKIPPED: usize = 200;

/// Writes every file of the portfolio into `dir` (a folder that exists), flat, never replacing a file.
/// The document is only read. One file's bytes are held at a time; `written` hears of each file as
/// soon as it is on disk. A file that cannot be written is left out and the rest go on.
pub fn extract_all(doc: &PdfDocument, dir: &Path, written: &mut dyn FnMut(&Path)) -> Result<Extracted> {
    if !dir.is_dir() {
        return Err(bad("the folder to extract into is not there"));
    }
    let mut out = Extracted::default();
    let mut left_out = 0usize;
    let mut skip = |out: &mut Extracted, line: String| {
        if out.skipped.len() < MAX_SKIPPED {
            out.skipped.push(line);
        } else {
            left_out += 1;
        }
    };
    // Decoded bytes in all, including those of a file that turned out too big: that is the work done.
    let mut total = 0u64;
    let mut streams: HashSet<i32> = HashSet::new();
    let mut texts = Texts::default();
    let mut next: HashMap<String, u32> = HashMap::new();
    let entries = tree(doc)?;
    for (e, id) in entries.iter().zip(entry_ids(&entries)) {
        let meta = attach::info_with(&mut texts, &e.spec, super::split_prefix(&e.key).1, id, AttachKind::File, None, None);
        let name = extract_name(&meta.name);
        let Some(stream) = stream_of(&e.spec) else {
            skip(&mut out, format!("{name}: it links to a file outside the document"));
            continue;
        };
        if meta.size > MAX_ATTACH {
            skip(&mut out, format!("{name}: over {} MB", MAX_ATTACH >> 20));
            continue;
        }
        // One stream is decoded once, whatever number of names point at it.
        if stream.is_indirect().unwrap_or(false) && stream.as_indirect().is_ok_and(|n| !streams.insert(n)) {
            skip(&mut out, format!("{name}: the same data as a file before it"));
            continue;
        }
        let room = EXTRACT_MAX_BYTES.saturating_sub(total);
        if room == 0 {
            skip(&mut out, format!("{name}: the folder would pass {} GB", EXTRACT_MAX_BYTES >> 30));
            continue;
        }
        let cap = room.min(MAX_ATTACH);
        let bytes = match stream.read_stream_capped(cap as usize) {
            Ok(b) => b,
            Err(err) if err.is_stream_cap() => {
                total += cap;
                if cap == MAX_ATTACH {
                    skip(&mut out, format!("{name}: over {} MB once unpacked", MAX_ATTACH >> 20));
                } else {
                    skip(&mut out, format!("{name}: the folder would pass {} GB", EXTRACT_MAX_BYTES >> 30));
                }
                continue;
            }
            Err(err) => {
                // It may have inflated to just under the cap before failing: that work is spent.
                total += cap;
                skip(&mut out, format!("{name}: {err}"));
                continue;
            }
        };
        total += bytes.len() as u64;
        match write_new(dir, &name, &bytes, &mut next) {
            Ok(file) => {
                written(&dir.join(&file));
                out.written.push(file);
            }
            Err(err) => skip(&mut out, format!("{name}: {err}")),
        }
    }
    if left_out > 0 {
        out.skipped.push(format!("and {left_out} more files left out"));
    }
    Ok(out)
}

#[cfg(test)]
pub(super) fn write_new_for_test(dir: &Path, name: &str, bytes: &[u8], next: &mut HashMap<String, u32>) -> String {
    write_new(dir, name, bytes, next).unwrap()
}
