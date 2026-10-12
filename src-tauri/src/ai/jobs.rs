//! The assistant's tools that take time or write files: compress, recognize text, export, convert to
//! PDF/A / PDF/UA / PDF/X, create a blank PDF, and compare.
//!
//! Same rules as `actions.rs` and `toolbox.rs`. A tool that makes a file writes a new one next to
//! the document under a name that is not taken (the original and every other file stay as they
//! are), reports its progress into the activity trail and ends early when the panel's Stop is
//! pressed (nothing half-written is left). A tool that changes the document (recognize text) goes
//! through the edit lane as one undo step under `EditSource::Assistant`.

use super::actions::{Touch, bool_arg, data_block, err, guard, landed, lane, pages_arg, plural, str_arg};
use super::toolbox::{engine, guard_content, unknown_keys};
use super::tools::{Class, Ctx, Lane, Output, Tool, UiAction, clip, int, page_list_label, pages_requested, tool};
use crate::convert::{self, CreateSource, ExportKind, ExportRequest, ImageFormat, Target};
use crate::engine::{DocId, DocSource, Engine};
use crate::tools::{BackendChoice, CompressRequest, OcrRequest, ToolError, make_backend, run_compress_new, run_ocr_as};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// More than this in one answer is a wall the user will not read.
const MAX_REPORT: usize = 6_000;

pub fn tools() -> Vec<Arc<Tool>> {
    vec![
        tool(
            "compare_documents",
            "Compares this document as it is now with the file it was opened from, and lists what changed (text, \
             formatting, images, pages, comments, form fields, metadata): the unsaved changes. Reads both; changes \
             nothing. To compare with a different open document use compare_with_open_document.",
            json!({ "type": "object", "additionalProperties": false, "properties": {
                "with": { "description": "only \"saved\" (the default)", "type": "string", "enum": ["saved"] },
            } }),
            Class::Read,
            |_| "Compare with the saved file".into(),
            compare_documents,
        ),
        tool(
            "compare_with_open_document",
            "Compares this document with ANOTHER open document and lists what differs (older = the other one). \
             document: the file name of the other open document, as the user said it. This reads the other \
             document's content, so the user is asked to approve it every time, in every mode.",
            json!({ "type": "object", "additionalProperties": false, "required": ["document"], "properties": {
                "document": { "type": "string", "description": "the file name of the other open document, e.g. invoice-v1.pdf" },
            } }),
            Class::Permanent,
            |a| format!("Compare with “{}”", clip(str_arg(a, "document").unwrap_or("another open document"), 60)),
            compare_with_open_document,
        )
        .checked(compare_other_check),
        tool(
            "compress_document",
            "Makes a smaller copy of the document next to it (\"<name> (compressed).pdf\"); the original is not changed. \
             preset: smallest, balanced (default) or high_quality. Works on the saved file: it fails when there are unsaved \
             changes, so the user saves first. Takes a while on big files; the activity trail shows the progress.",
            json!({ "type": "object", "additionalProperties": false, "properties": {
                "preset": { "type": "string", "enum": ["smallest", "balanced", "high_quality"] },
            } }),
            Class::Edit,
            |a| format!("Make a compressed copy ({})", preset_arg(a).map_or("balanced", preset_name)),
            compress_document,
        )
        .checked(compress_check),
        tool(
            "ocr_document",
            "Recognizes the text of scanned pages and adds it as an invisible layer, so the document becomes searchable and \
             selectable: one undo step. pages (1-based, default every page); pages that already have text are skipped unless \
             force is true. engine: windows (default) or tesseract; languages: BCP 47 tags (en-US, sv-SE) for the Windows \
             engine, Tesseract codes (eng, swe) for Tesseract. Takes a while; the activity trail shows the progress. \
             Afterwards the text tools read the new layer.",
            json!({ "type": "object", "additionalProperties": false, "properties": {
                "pages": { "type": "array", "items": { "type": "integer", "minimum": 1 }, "description": "1-based pages; omit for every page" },
                "languages": { "type": "array", "items": { "type": "string" }, "maxItems": 5 },
                "force": { "type": "boolean", "description": "read pages that already have text and replace an earlier layer of ours" },
                "engine": { "type": "string", "enum": ["windows", "tesseract"] },
            } }),
            Class::Edit,
            |a| format!("Recognize text on {}", pages_words(a)),
            ocr_document,
        )
        .checked(|c, _| guard_content(c)),
        tool(
            "export_document",
            "Exports the document as it is now into a new file or folder next to it; nothing is overwritten. format: text \
             (.txt; layout=true keeps columns), html, word (.docx), images (one image per page; image_format png|jpeg|tiff, \
             dpi 36-1200 default 150, grayscale) or extract_images (the pictures drawn on the pages, into a folder). pages \
             (1-based) default every page.",
            json!({ "type": "object", "additionalProperties": false, "required": ["format"], "properties": {
                "format": { "type": "string", "enum": ["text", "html", "word", "images", "extract_images"] },
                "pages": { "type": "array", "items": { "type": "integer", "minimum": 1 } },
                "layout": { "type": "boolean", "description": "text: keep the columns and indentation" },
                "image_format": { "type": "string", "enum": ["png", "jpeg", "tiff"] },
                "dpi": { "type": "integer", "minimum": 36, "maximum": 1200 },
                "grayscale": { "type": "boolean" },
            } }),
            Class::Edit,
            |a| format!("Export as {} to a new file", str_arg(a, "format").unwrap_or("a file").replace('_', " ")),
            export_document,
        )
        .checked(export_check),
        tool(
            "convert_to_standard",
            "Makes a converted COPY of the document that meets pdfa-1b, pdfa-2b, pdfa-3b, pdfa-2u, pdfua-1 or pdfx-4 as far \
             as the safe fixes go, next to the original (\"<name> (PDF-A).pdf\" style), and checks the copy again. The \
             original is not changed. Run check_preflight first to see what would be fixed. lang (BCP 47) is needed when \
             check_preflight says the document has no language. Say plainly what was fixed and what still fails.",
            json!({ "type": "object", "additionalProperties": false, "required": ["profile"], "properties": {
                "profile": { "type": "string", "enum": ["pdfa-1b", "pdfa-2b", "pdfa-3b", "pdfa-2u", "pdfua-1", "pdfx-4"] },
                "lang": { "type": "string", "description": "document language, e.g. en-US or sv-SE" },
            } }),
            Class::Edit,
            |a| format!("Convert a copy to {}", profile_label(a)),
            convert_to_standard,
        )
        .checked(convert_check),
        tool(
            "create_blank_pdf",
            "Creates a new blank PDF (1-500 pages, page_size letter or a4) as a separate unsaved document the user can open. \
             It does not change this document.",
            json!({ "type": "object", "additionalProperties": false, "properties": {
                "pages": { "type": "integer", "minimum": 1, "maximum": 500, "description": "default 1" },
                "page_size": { "type": "string", "enum": ["letter", "a4"] },
            } }),
            Class::Edit,
            |a| format!("Create a blank PDF ({})", plural(int(a, "pages").unwrap_or(1).clamp(1, 500) as usize, "page")),
            create_blank_pdf,
        ),
    ]
}

// ---- shared -----------------------------------------------------------------------------------

/// A tool-job error code as a sentence the model can act on.
fn job_err(e: String) -> String {
    match e.as_str() {
        "cancelled" => "Stopped by the user. Nothing was written.".into(),
        "stale" => "The document changed while this ran. Retry.".into(),
        "needs-password" | "bad-password" => "The document is password protected and Inkwell does not have its password.".into(),
        "tesseract-not-found" => {
            "Tesseract is not installed or not set up. Use the Windows engine, or tell the user to set Tesseract up in the OCR tool.".into()
        }
        "language-unavailable" => "That recognition language is not installed. Use another, or tell the user to install it.".into(),
        "output-exists" => "A file with the name this would use already exists. Nothing was written; try again.".into(),
        _ => e,
    }
}

fn doc_path(l: &Lane) -> Result<PathBuf, String> {
    Ok(PathBuf::from(l.eng.doc_entry(l.doc).map_err(err)?.path()))
}

/// The folder and stem of the document, for naming what is made from it.
fn folder_and_stem(l: &Lane) -> Result<(PathBuf, String), String> {
    let path = doc_path(l)?;
    let dir = path.parent().filter(|d| d.is_dir()).ok_or("The document's folder is not available.")?.to_path_buf();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    Ok((dir, convert::safe_stem(&stem)))
}

/// `<folder of the document>/<stem><suffix>.<ext>`, numbered `(2)`, `(3)` when that is taken: never a file that exists.
fn sibling(l: &Lane, suffix: &str, ext: &str) -> Result<PathBuf, String> {
    let (dir, stem) = folder_and_stem(l)?;
    let dotted = format!(".{ext}");
    let unique = convert::unique_stem(&dir, &format!("{stem}{suffix}"), std::slice::from_ref(&dotted));
    Ok(dir.join(format!("{unique}{dotted}")))
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A file the assistant made, as the "Open" offer under its answer.
fn offered(c: &Ctx, path: &Path) -> Option<UiAction> {
    let p = path.to_string_lossy().into_owned();
    let token = c.host.offer_file(&p)?;
    Some(UiAction::File { name: file_name(path), path: p, token })
}

/// Jobs that read the file on disk need the document saved first: the assistant cannot save.
fn saved(l: &Lane, what: &str) -> Result<(), String> {
    if l.eng.edit_state(l.doc).map_err(err)?.dirty {
        return Err(format!(
            "{what} works on the saved file, and this document has unsaved changes. Ask the user to save it (Ctrl+S), then call this again."
        ));
    }
    Ok(())
}

fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_048_576.0 {
        format!("{:.1} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

fn pages_words(a: &Value) -> String {
    let p = pages_requested(a);
    if p.is_empty() { "every page".into() } else { page_list_label(&p) }
}

/// `1,3,5` (1-based) for the jobs that take a range string; none for every page.
fn range_of(pages: &[u32]) -> Option<String> {
    (!pages.is_empty()).then(|| pages.iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(","))
}

// ---- open documents and comparing ------------------------------------------------------------

fn name_of(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// The bytes a document shows now (unsaved changes included).
fn bytes_of(eng: &Engine, doc: DocId) -> Result<Vec<u8>, String> {
    match eng.doc_source(doc).map_err(err)?.1 {
        DocSource::Path(p) => std::fs::read(&p).map_err(|e| format!("Could not read the file: {e}")),
        DocSource::Bytes(b, n) => Ok(b[..n].to_vec()),
    }
}

/// What `compare_documents` and `compare_with_open_document` do once the other side is known: `old` is the
/// bytes of the older document and `old_name` how the answer calls it.
fn compare_bytes(c: &Ctx, l: &Lane, old: Vec<u8>, old_name: String, old_pw: Option<&str>) -> Result<Output, String> {
    use crate::diff::{ChangeKind, CompareOptions};
    let new = bytes_of(l.eng, l.doc)?;
    let opts = CompareOptions {
        password_a: old_pw.map(|p| p.to_string()),
        password_b: l.eng.password(l.doc).map_err(err)?.map(|p| p.to_string()),
        include_volatile_metadata: false,
    };
    let mut last = (u32::MAX, u32::MAX);
    let cmp = crate::diff::compare_with(&old, &new, &opts, c.cancel, &mut |p| {
        if (p.done, p.total) != last {
            last = (p.done, p.total);
            (c.progress)(format!("Comparing: {} of {}", p.done, p.total.max(1)));
        }
    })
    .map_err(|e| match e {
        crate::diff::DiffError::Cancelled => job_err("cancelled".into()),
        e => e.to_string(),
    })?;
    if cmp.identical || cmp.changes.is_empty() {
        return Ok(Output::text(format!("No difference between {old_name} and this document.")));
    }
    let mut kinds: std::collections::BTreeMap<ChangeKind, usize> = std::collections::BTreeMap::new();
    for ch in &cmp.changes {
        *kinds.entry(ch.kind).or_default() += 1;
    }
    let summary: Vec<String> = kinds
        .iter()
        .map(|(k, n)| format!("{n} {}", serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()))
        .collect();
    let mut s = format!(
        "{} from {old_name} ({}) to this document ({}): {}.\n",
        plural(cmp.changes.len(), "change"),
        plural(cmp.pages.a_pages as usize, "page"),
        plural(cmp.pages.b_pages as usize, "page"),
        summary.join(", ")
    );
    let mut body = String::new();
    for (i, ch) in cmp.changes.iter().enumerate() {
        if i >= 60 || body.len() > MAX_REPORT {
            let _ = writeln!(body, "… and {} more", cmp.changes.len() - i);
            break;
        }
        let _ = writeln!(body, "- {}", clip(&ch.label, 240));
    }
    s.push_str(&data_block(&body));
    Ok(Output::text(s))
}

fn compare_documents(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = engine(c)?;
    unknown_keys(a, &["with"], "compare_documents")?;
    match a.get("with") {
        None | Some(Value::Null) => {}
        Some(v) if v.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("saved")) => {}
        Some(_) => {
            return Err("compare_documents only compares with the saved file. To compare with another open document use compare_with_open_document.".into());
        }
    }
    if !l.eng.edit_state(l.doc).map_err(err)?.dirty {
        return Err("There are no unsaved changes: this document is the saved file. To compare with another open document use compare_with_open_document.".into());
    }
    let path = doc_path(&l)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("Could not read the saved file: {e}"))?;
    let name = format!("the saved {}", name_of(&path.to_string_lossy()));
    let pw = l.eng.password(l.doc).map_err(err)?;
    compare_bytes(c, &l, bytes, name, pw.as_ref().map(|p| p.as_str()))
}

/// The one other open document called `name` (file name, any case). Several of that name, or none, are errors that
/// do not list what else is open: the model gets to know a document's name only from the user.
fn other_document(l: &Lane, name: &str) -> Result<(DocId, String), String> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return Err("\"document\" is required: the file name of the other open document.".into());
    }
    let matches: Vec<(DocId, String)> = l
        .eng
        .open_docs()
        .iter()
        .filter(|d| d.id != l.doc)
        .map(|d| (d.id, name_of(&d.path())))
        .filter(|(_, n)| n.to_lowercase() == want)
        .collect();
    match matches.len() {
        0 => Err(format!("No other open document is called “{}”. Ask the user which document to compare with.", clip(name, 80))),
        1 => Ok(matches.into_iter().next().expect("one match")),
        _ => Err(format!("More than one open document is called “{}”. Ask the user to close the others, or to say which one.", clip(name, 80))),
    }
}

fn compare_other_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    unknown_keys(a, &["document"], "compare_with_open_document")?;
    let name = str_arg(a, "document").ok_or("\"document\" is required: the file name of the other open document.")?;
    let Some(l) = c.host.lane() else { return Ok(None) };
    let (_, found) = other_document(&l, name)?;
    Ok(Some(format!("Reads the content of the open document “{found}” and sends what differs from this one to the assistant")))
}

fn compare_with_open_document(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = engine(c)?;
    unknown_keys(a, &["document"], "compare_with_open_document")?;
    let name = str_arg(a, "document").ok_or("\"document\" is required: the file name of the other open document.")?;
    let (id, found) = other_document(&l, name)?;
    let bytes = bytes_of(l.eng, id)?;
    let pw = l.eng.password(id).map_err(err)?;
    compare_bytes(c, &l, bytes, found, pw.as_ref().map(|p| p.as_str()))
}

// ---- compress ------------------------------------------------------------------------------------

fn preset_arg(a: &Value) -> Result<crate::compress::Preset, String> {
    use crate::compress::Preset;
    match str_arg(a, "preset").unwrap_or("balanced") {
        "smallest" => Ok(Preset::Smallest),
        "balanced" => Ok(Preset::Balanced),
        "high_quality" | "high-quality" | "highQuality" => Ok(Preset::HighQuality),
        o => Err(format!("Unknown preset \"{o}\". Use smallest, balanced or high_quality.")),
    }
}

fn preset_name(p: crate::compress::Preset) -> &'static str {
    use crate::compress::Preset;
    match p {
        Preset::Smallest => "smallest",
        Preset::Balanced | Preset::Custom => "balanced",
        Preset::HighQuality => "high quality",
    }
}

fn compress_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    unknown_keys(a, &["preset"], "compress_document")?;
    preset_arg(a)?;
    let Some(l) = c.host.lane() else { return Ok(None) };
    saved(&l, "Compressing")?;
    let dest = sibling(&l, " (compressed)", "pdf")?;
    Ok(Some(format!("Saves “{}” next to the document; the original is not changed", file_name(&dest))))
}

fn compress_document(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = engine(c)?;
    unknown_keys(a, &["preset"], "compress_document")?;
    let preset = preset_arg(a)?;
    saved(&l, "Compressing")?;
    let dest = sibling(&l, " (compressed)", "pdf")?;
    let req = CompressRequest { doc: l.doc, preset, custom: None, save_to: None };
    let mut last = -1i32;
    // a name nobody confirmed: a file that appears there meanwhile is never replaced
    let r = run_compress_new(l.eng, &req, dest.clone(), c.cancel, &mut |p| {
        let pct = (p.fraction * 100.0) as i32;
        if pct != last {
            last = pct;
            (c.progress)(format!("Compressing: {pct}%"));
        }
    })
    .map_err(|e| job_err(e.0))?;
    let mut s = if r.kept_original {
        format!(
            "Nothing in the document could be made smaller, so “{}” is a plain copy of the original ({}). Tell the user.",
            file_name(&r.output),
            size_text(r.size_before)
        )
    } else {
        format!(
            "Made “{}”: {} → {} ({:.0}% smaller, {}). The original is unchanged.",
            file_name(&r.output),
            size_text(r.size_before),
            size_text(r.size_after),
            r.saved_percent,
            plural(r.pages as usize, "page")
        )
    };
    for w in r.warnings.iter().take(3) {
        let _ = write!(s, " Note: {}", clip(w, 200));
    }
    Ok(Output { text: s, ui: offered(c, &r.output), ..Default::default() })
}

// ---- recognize text ----------------------------------------------------------------------------

fn ocr_document(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Content, &[])?;
    unknown_keys(a, &["pages", "from", "to", "languages", "force", "engine"], "ocr_document")?;
    let count = l.eng.doc_entry(l.doc).map_err(err)?.page_count();
    let pages = pages_arg(a, count)?;
    let languages: Vec<String> = match a.get("languages") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(v)) => {
            let mut out = Vec::new();
            for x in v.iter().take(5) {
                let s = x.as_str().map(str::trim).filter(|s| !s.is_empty() && s.len() <= 20 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
                out.push(s.ok_or("\"languages\" are short codes like en-US or eng.")?.to_string());
            }
            out
        }
        Some(_) => return Err("\"languages\" must be a list of codes like en-US.".into()),
    };
    let backend = match str_arg(a, "engine").unwrap_or("windows") {
        "windows" => BackendChoice::Windows,
        "tesseract" => BackendChoice::Tesseract,
        o => return Err(format!("Unknown engine \"{o}\". Use windows or tesseract.")),
    };
    let req = OcrRequest { doc: l.doc, pages: range_of(&pages), languages, force: bool_arg(a, "force", false), backend, style: Default::default(), dpi: None, font: Default::default(), save_to: None };
    let tesseract = c.host.tesseract();
    let rec = make_backend(backend, tesseract.as_deref()).map_err(|e| job_err(e.0))?;
    let out = run_ocr_as(
        l.eng,
        &req,
        None,
        rec.as_ref(),
        &|p| (c.progress)(format!("Reading page {} of {}", p.done.max(1), p.total.max(1))),
        c.cancel,
        l.source.clone(),
    )
    .map_err(|e| job_err(e.0))?;
    let r = &out.report;
    if r.pages_recognized == 0 {
        let mut why = String::from("No text was added.");
        if r.pages_skipped_text > 0 {
            let _ = write!(why, " {} already have text (force=true reads them again).", plural(r.pages_skipped_text as usize, "page"));
        }
        if r.pages_no_text > 0 {
            let _ = write!(why, " {} had nothing readable.", plural(r.pages_no_text as usize, "page"));
        }
        return Err(why);
    }
    let state = out.state.ok_or("The text was recognized but the document did not change.")?;
    let mut said = format!("Recognized text on {} ({} words, {} engine).", plural(r.pages_recognized as usize, "page"), r.words, r.backend);
    if r.pages_skipped_text > 0 {
        let _ = write!(said, " {} already had text and were skipped.", plural(r.pages_skipped_text as usize, "page"));
    }
    if r.pages_no_text > 0 {
        let _ = write!(said, " {} had nothing readable.", plural(r.pages_no_text as usize, "page"));
    }
    if r.pages_failed > 0 {
        let _ = write!(said, " {} could not be read.", plural(r.pages_failed as usize, "page"));
    }
    for w in r.warnings.iter().take(2) {
        let _ = write!(said, " {}", clip(w, 200));
    }
    Ok(landed(&l, state, said))
}

// ---- export -------------------------------------------------------------------------------------

fn export_kind(a: &Value) -> Result<ExportKind, String> {
    Ok(match str_arg(a, "format") {
        Some("text") => ExportKind::Text { layout: bool_arg(a, "layout", false) },
        Some("html") => ExportKind::Html { xhtml: false },
        Some("word") => ExportKind::Word,
        Some("extract_images") => ExportKind::ExtractImages { min_side: 32 },
        Some("images") => {
            let format = match str_arg(a, "image_format").unwrap_or("png") {
                "png" => ImageFormat::Png,
                "jpeg" | "jpg" => ImageFormat::Jpeg,
                "tiff" | "tif" => ImageFormat::Tiff,
                o => return Err(format!("Unknown image_format \"{o}\". Use png, jpeg or tiff.")),
            };
            let dpi = int(a, "dpi").unwrap_or(150);
            if !(36..=1200).contains(&dpi) {
                return Err("\"dpi\" must be from 36 to 1200.".into());
            }
            ExportKind::Images { format, dpi: dpi as u32, gray: bool_arg(a, "grayscale", false), quality: 90 }
        }
        Some(o) => return Err(format!("Unknown format \"{o}\". Use text, html, word, images or extract_images.")),
        None => return Err("\"format\" is required: text, html, word, images or extract_images.".into()),
    })
}

/// Where an export goes: a new file, or for the formats that write many files a new folder, next to the document.
fn export_target(l: &Lane, kind: &ExportKind) -> Result<(Target, PathBuf), String> {
    if kind.is_folder() {
        let (dir, stem) = folder_and_stem(l)?;
        let name = if matches!(kind, ExportKind::Images { .. }) { " (images)" } else { " (pictures)" };
        let unique = convert::unique_stem(&dir, &format!("{stem}{name}"), &[String::new()]);
        let folder = dir.join(unique);
        Ok((Target::Folder(folder.clone()), folder))
    } else {
        let file = sibling(l, "", kind.extension())?;
        Ok((Target::File(file.clone()), file))
    }
}

fn export_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    unknown_keys(a, &["format", "pages", "from", "to", "layout", "image_format", "dpi", "grayscale"], "export_document")?;
    let kind = export_kind(a)?;
    let Some(l) = c.host.lane() else { return Ok(None) };
    let (_, dest) = export_target(&l, &kind)?;
    let what = if kind.is_folder() { "folder" } else { "file" };
    Ok(Some(format!("Writes the {what} “{}” next to the document; nothing is overwritten", file_name(&dest))))
}

fn export_document(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = engine(c)?;
    unknown_keys(a, &["format", "pages", "from", "to", "layout", "image_format", "dpi", "grayscale"], "export_document")?;
    let kind = export_kind(a)?;
    let count = l.eng.doc_entry(l.doc).map_err(err)?.page_count();
    let pages = pages_arg(a, count)?;
    let (target, dest) = export_target(&l, &kind)?;
    let made_folder = if let Target::Folder(d) = &target {
        std::fs::create_dir_all(d).map_err(|e| format!("Could not make the folder: {e}"))?;
        true
    } else {
        false
    };
    let req = ExportRequest { doc: l.doc, kind: kind.clone(), pages: range_of(&pages), save_to: String::new() };
    let mut last = u32::MAX;
    let r = convert::export_new(l.eng, &req, target, c.cancel, &mut |p| {
        if p.done != last {
            last = p.done;
            (c.progress)(format!("Exporting: {} of {}", p.done, p.total.max(1)));
        }
    })
    .map_err(|e| {
        if made_folder {
            // only when nothing was put in it
            let _ = std::fs::remove_dir(&dest);
        }
        job_err(ToolError::from(e).0)
    })?;
    let mut s = if r.files.len() == 1 {
        format!("Exported {} to “{}” ({}).", plural(r.pages as usize, "page"), name_of(&r.files[0]), size_text(r.bytes))
    } else {
        format!("Exported {} into the folder {} ({}, {}).", plural(r.pages as usize, "page"), r.folder, plural(r.files.len(), "file"), size_text(r.bytes))
    };
    for w in r.warnings.iter().take(3) {
        let _ = write!(s, " Note: {}", clip(w, 200));
    }
    let ui = if r.files.len() == 1 && !kind.is_folder() { offered(c, Path::new(&r.files[0])) } else { None };
    Ok(Output { text: s, ui, ..Default::default() })
}

// ---- convert to a standard -------------------------------------------------------------------------------

fn profile_label(a: &Value) -> &'static str {
    super::toolbox::profile_arg(a).map_or("a standard", |p| p.label())
}

/// Everything `convert_to_standard` can refuse, and what it would write, worked out before the user is asked: the
/// preflight check runs here (and again when the tool runs, on whatever the document is by then).
struct ConvertJob {
    profile: crate::preflight::Profile,
    convert: crate::preflight::Convert,
    dest: PathBuf,
}

fn convert_job(c: &Ctx, l: &Lane, a: &Value) -> Result<ConvertJob, String> {
    use crate::preflight::{Convert, check_doc};
    unknown_keys(a, &["profile", "lang"], "convert_to_standard")?;
    let profile = super::toolbox::profile_arg(a)?;
    let lang = super::toolbox::text_arg(a, "lang", 35)?.filter(|t| !t.is_empty());
    // The copy is written without encryption: a document its owner keeps from being copied out of is refused (as
    // export and extract do), and a protected one is not made into an open one on the assistant's say-so.
    l.eng.ensure_extractable(l.doc).map_err(err)?;
    if l.eng.security_info(l.doc).map_err(err)?.encrypted {
        return Err("This document is password protected, and a converted copy is written without its protection, so the \
                    assistant does not make one. Tell the user they can convert it themselves in the Preflight tool \
                    (as a new file)."
            .into());
    }
    let mut last = u32::MAX;
    let before = check_doc(l.eng, l.doc, profile, &mut |p| {
        if p.done != last {
            last = p.done;
            (c.progress)(format!("Checking page {} of {}", p.done.max(1), p.pages));
        }
    }, c.cancel)
    .map_err(|e| job_err(err(e)))?;
    if before.conforms {
        return Err(format!("The document already meets {} as far as the checks go. Nothing to convert.", profile.label()));
    }
    if before.plan.needs_lang && lang.is_none() {
        return Err("The document has no language and the conversion has to set one: pass \"lang\" (for example en-US or sv-SE); ask the user if it is not clear.".into());
    }
    let groups: Vec<_> = before.plan.groups.iter().filter(|g| g.issues > 0 && g.default_on && g.disabled.is_none()).map(|g| g.id).collect();
    if groups.is_empty() {
        let why = before.plan.remaining.first().map(|r| format!(" {}.", r.text)).unwrap_or_default();
        return Err(format!("No safe fix applies to what fails, so a conversion would not change the result.{why} Tell the user."));
    }
    let suffix = format!(" ({})", profile.label().replace('/', "-"));
    let dest = sibling(l, &suffix, "pdf")?;
    Ok(ConvertJob { profile, convert: Convert { groups, lang }, dest })
}

fn convert_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    let Some(l) = c.host.lane() else { return Ok(None) };
    let job = convert_job(c, &l, a)?;
    Ok(Some(format!(
        "Saves the converted copy “{}” next to the document; the original is not changed",
        file_name(&job.dest)
    )))
}

fn convert_to_standard(c: &Ctx, a: &Value) -> Result<Output, String> {
    use crate::preflight::convert_to_new_file;
    let l = engine(c)?;
    let ConvertJob { profile, convert, dest } = convert_job(c, &l, a)?;
    let mut last = u32::MAX;
    let done = convert_to_new_file(l.eng, l.doc, profile, &convert, &dest.to_string_lossy(), &mut |p| {
        if p.done != last {
            last = p.done;
            (c.progress)(format!("Converting: page {} of {}", p.done.max(1), p.pages));
        }
    }, c.cancel)
    .map_err(|e| job_err(err(e)))?;
    let mut s = format!("Made “{}”, a converted copy for {}. The original is unchanged.", file_name(&dest), profile.label());
    let failed = done.report.failed();
    if done.report.conforms {
        s.push_str(" The copy passes every rule the checker can test; some rules need a person's check.");
    } else {
        let _ = write!(s, " {} still fail in the copy.", plural(failed, "rule"));
    }
    if !done.fixed.is_empty() {
        let fixed: Vec<String> = done.fixed.iter().take(12).map(|f| format!("- {}", clip(f, 160))).collect();
        s.push('\n');
        s.push_str(&fixed.join("\n"));
    }
    Ok(Output { text: s, ui: offered(c, &dest), ..Default::default() })
}

// ---- create ---------------------------------------------------------------------------------------

fn create_blank_pdf(c: &Ctx, a: &Value) -> Result<Output, String> {
    unknown_keys(a, &["pages", "page_size"], "create_blank_pdf")?;
    let pages = int(a, "pages").unwrap_or(1);
    if !(1..=500).contains(&pages) {
        return Err("\"pages\" must be from 1 to 500.".into());
    }
    let size = match str_arg(a, "page_size").unwrap_or("letter") {
        "letter" => "letter",
        "a4" => "a4",
        o => return Err(format!("Unknown page_size \"{o}\". Use letter or a4.")),
    };
    let req: convert::CreateRequest = serde_json::from_value(json!({
        "source": { "kind": "blank", "pages": pages },
        "pageSize": size,
        "margin": 0.0,
    }))
    .map_err(|e| e.to_string())?;
    let out = convert::create(&CreateSource::Blank(pages as u32), &req, c.cancel, &mut |_| {})
        .map_err(|e| job_err(ToolError::from(e).0))?;
    let path = PathBuf::from(&out.path);
    Ok(Output {
        text: format!(
            "Created a new blank PDF of {} as an unsaved document. The user opens it from the button under your answer.",
            plural(out.pages as usize, "page")
        ),
        ui: offered(c, &path),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests;
