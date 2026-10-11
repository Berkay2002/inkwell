//! Creating a portfolio from files and extracting a portfolio's files to a folder.

use super::{fill_ci, resolved_dict, size_text};
use crate::engine::edit::{Readback, rewrite_opts, write_atomic, write_doc};
use crate::engine::{Engine, EngineError};
use crate::organize::attach::{self, AttachKind, MAX_ATTACH, NewFile, clean_name, stream_of, tree, unique};
use crate::organize::combine::Face;
use crate::organize::{Result, bad, put_int, put_name, put_str};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Font, Size};
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

/// Longest file name written by Extract.
const MAX_NAME: usize = 200;
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
    for s in sources {
        let data = std::fs::read(&s.path).map_err(|e| bad(format!("{}: {e}", s.name)))?;
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

/// `name` cut to [`MAX_NAME`] characters, its extension kept.
fn capped(name: &str) -> String {
    if name.chars().count() <= MAX_NAME {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && name[i..].chars().count() <= 20 => name.split_at(i),
        _ => (name, ""),
    };
    let room = MAX_NAME - ext.chars().count();
    let cut: String = stem.chars().take(room).collect();
    format!("{}{ext}", cut.trim_end())
}

/// A name for a file taken out of a document: from [`clean_name`], and no longer than [`MAX_NAME`].
pub fn extract_name(name: &str) -> String {
    clean_name(&capped(&clean_name(name)))
}

/// Writes `bytes` as a new file in `dir` named `name`, or `name (2)` and so on when that exists: a file
/// is never replaced. Returns the name it got.
fn write_new(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<String> {
    use std::io::Write;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    for n in 1..10_000 {
        let candidate = if n == 1 {
            name.to_string()
        } else {
            // The number goes in before the cut, so a name at the length limit still gets a different name.
            let suffix = format!(" ({n})");
            let room = MAX_NAME.saturating_sub(ext.chars().count() + suffix.chars().count());
            let cut: String = stem.chars().take(room).collect();
            format!("{}{suffix}{ext}", cut.trim_end())
        };
        let path = dir.join(&candidate);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let r = f.write_all(bytes).and_then(|()| f.sync_all());
                if let Err(e) = r {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
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

/// Writes every file of the portfolio into `dir` (a folder that exists), flat, never replacing a file.
/// The document is only read. One file's bytes are held at a time.
pub fn extract_all(doc: &PdfDocument, dir: &Path) -> Result<Extracted> {
    if !dir.is_dir() {
        return Err(bad("the folder to extract into is not there"));
    }
    let mut out = Extracted::default();
    let mut total = 0u64;
    for e in tree(doc)? {
        let meta = attach::info(&e.spec, super::split_prefix(&e.key).1, format!("n:{}", e.key), AttachKind::File, None, None);
        let name = extract_name(&meta.name);
        let Some(stream) = stream_of(&e.spec) else {
            out.skipped.push(format!("{name}: it links to a file outside the document"));
            continue;
        };
        if meta.size > MAX_ATTACH {
            out.skipped.push(format!("{name}: over {} MB", MAX_ATTACH >> 20));
            continue;
        }
        let bytes = match stream.read_stream_capped(MAX_ATTACH as usize) {
            Ok(b) => b,
            Err(err) if err.is_stream_cap() => {
                out.skipped.push(format!("{name}: over {} MB once unpacked", MAX_ATTACH >> 20));
                continue;
            }
            Err(err) => {
                out.skipped.push(format!("{name}: {err}"));
                continue;
            }
        };
        if total + bytes.len() as u64 > EXTRACT_MAX_BYTES {
            out.skipped.push(format!("{name}: the folder would pass {} GB", EXTRACT_MAX_BYTES >> 30));
            continue;
        }
        match write_new(dir, &name, &bytes) {
            Ok(written) => {
                total += bytes.len() as u64;
                out.written.push(written);
            }
            Err(err) => return Err(EngineError::Io(format!("{name}: {err}"))),
        }
    }
    Ok(out)
}

