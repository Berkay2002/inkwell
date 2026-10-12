//! Edit existing text in place (Acrobat "Edit text & images" for text): list a page's text
//! blocks, and replace a block's text, change its size, colour and alignment, move it or resize it.
//!
//! Every change is one `Engine::apply_removing` step on the authoritative document: one undo step,
//! kept by Save, recorded in history. Coordinates in and out are the viewer's page space (points,
//! origin at the top left of the page as shown, `/Rotate` applied: the space of `page_text`).
//! See docs/text-edit.md.

pub mod app;
pub(crate) mod bundled;
pub(crate) mod content;
pub(crate) mod encodings;
mod font;
mod layout;
pub mod replace;
pub(crate) mod text;

pub use text::{Align, Fallback, Issue, Rise, RunChange, TextChange};
pub(crate) use content::Matrix;

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use mupdf::pdf::PdfDocument;
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, EngineError>;

/// One block of text on a page.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextBlockDto {
    /// Pass to [`edit`] with the `rev` of the list it came from.
    pub index: u32,
    pub text: String,
    /// `[x0, y0, x1, y1]` in page space.
    pub rect: [f32; 4],
    /// Each line's box, in page space.
    pub lines: Vec<[f32; 4]>,
    /// `/BaseFont` (a subset keeps its `ABCDEF+` prefix).
    pub font: String,
    /// Points.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: [f32; 3],
    pub align: Align,
    /// Distance between lines as a multiple of the font size; none for a single line.
    pub line_spacing: Option<f32>,
    /// The text as runs of one font, size and colour (pass their indexes as `source` in
    /// `BlockChange::runs`).
    pub runs: Vec<RunDto>,
    pub editable: bool,
    /// Why it can't be edited.
    pub issue: Option<Issue>,
    /// The reason as a sentence for the UI.
    pub reason: Option<String>,
}

/// One run of a block.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunDto {
    pub index: u32,
    pub text: String,
    /// `/BaseFont`.
    pub font: String,
    /// The font as the font picker names it (`doc:<object>` or `res:<name>`).
    pub font_id: String,
    pub size: f32,
    pub color: [f32; 3],
    pub bold: bool,
    pub italic: bool,
    /// Drawn with one of our own tagged rectangles under or through it (other producers' lines
    /// are not guessed at).
    pub underline: bool,
    pub strike: bool,
    /// By the sign of its `Ts`, from any file.
    pub rise: Option<Rise>,
}

/// An area that shows text that can't be edited here (outside any block).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AreaDto {
    pub rect: [f32; 4],
    pub issue: Issue,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextBlocks {
    /// The generation the blocks belong to.
    pub rev: u32,
    pub blocks: Vec<TextBlockDto>,
    pub areas: Vec<AreaDto>,
    /// Why the document's text can't be changed (a certified document); none when it can.
    pub locked: Option<String>,
    /// The document is signed: a change is saved after the signature.
    pub signed: bool,
}

/// A change to one block, in page space.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BlockChange {
    /// New text; `\n` starts a new line, the rest is reflowed to the block's width.
    pub text: Option<String>,
    /// Font size in points.
    pub size: Option<f32>,
    /// RGB 0..1.
    pub color: Option<[f32; 3]>,
    pub align: Option<Align>,
    /// Move by `[dx, dy]` (page space, y down).
    pub offset: Option<[f32; 2]>,
    /// New width of the block (page space): the text reflows to it.
    pub width: Option<f32>,
    /// Distance between lines as a multiple of the font size (0.5 to 5).
    pub line_spacing: Option<f32>,
    /// Bold or italic for every run: a font of the same family and style in the document, else
    /// the standard face of that style.
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// For every run without its own: underline, strikeout, superscript or subscript (`null`
    /// clears it; missing keeps it).
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    #[serde(default, deserialize_with = "text::clearable", skip_serializing_if = "Option::is_none")]
    pub rise: Option<Option<Rise>>,
    /// The new content as runs, each with its own font, size, colour, bold and italic (replaces
    /// `text`; see `RunChange`).
    pub runs: Option<Vec<RunChange>>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextEdited {
    pub state: EditState,
    /// Set when the block's font could not show the text and another one was used (the first of
    /// `fallbacks`).
    pub fallback: Option<Fallback>,
    pub fallbacks: Vec<Fallback>,
}

/// User space → page space, and the page's origin there.
pub(crate) fn page_space(doc: &PdfDocument, page: u32) -> Result<Matrix> {
    let p = doc.find_page(page as i32)?;
    let m = p.page_ctm()?;
    let b = doc.load_page(page as i32)?.bounds()?;
    let m = Matrix([m.a, m.b, m.c, m.d, m.e, m.f].map(f64::from));
    Ok(m.then(&Matrix::translate(-f64::from(b.x0), -f64::from(b.y0))))
}

fn rect32(m: &Matrix, r: [f64; 4]) -> [f32; 4] {
    m.bbox(r).map(|v| v as f32)
}

/// The text blocks of `page` (0-based) as the document is now.
pub fn blocks(eng: &Engine, doc: DocId, page: u32) -> Result<TextBlocks> {
    let entry = eng.doc_entry(doc)?;
    eng.read_custom(doc, |d| {
        let rev = entry.generation();
        let pc = text::read_page(d, page)?;
        let m = page_space(d, page)?;
        let blocks = text::group_blocks(&pc.lines)
            .into_iter()
            .enumerate()
            .map(|(i, b)| TextBlockDto {
                index: i as u32,
                text: b.text.clone(),
                rect: rect32(&m, b.rect),
                lines: b.lines.iter().map(|&l| rect32(&m, pc.lines[l].rect)).collect(),
                font: b.base_font.clone(),
                size: b.size as f32,
                bold: b.bold,
                italic: b.italic,
                color: b.color.map(|v| v as f32),
                align: b.align,
                line_spacing: line_spacing(&b, &pc.lines, &m),
                runs: b
                    .runs
                    .iter()
                    .enumerate()
                    .map(|(k, r)| RunDto {
                        index: k as u32,
                        text: r.text.clone(),
                        font: r.base_font.clone(),
                        font_id: pc.font_id(&r.font),
                        size: r.size as f32,
                        color: r.color.map(|v| v as f32),
                        bold: r.bold,
                        italic: r.italic,
                        underline: r.underline,
                        strike: r.strike,
                        rise: r.rise_kind(),
                    })
                    .collect(),
                editable: b.issue.is_none(),
                issue: b.issue,
                reason: b.issue.map(|i| text::issue_text(i).to_string()),
            })
            .collect();
        let areas = pc
            .areas
            .iter()
            .map(|a| AreaDto { rect: rect32(&m, a.rect), issue: a.issue, reason: text::issue_text(a.issue).into() })
            .collect();
        let locked = match crate::outline::refuse_if_certified(d, "Editing its text") {
            Err(EngineError::Invalid(m)) => Some(m),
            _ => None,
        };
        Ok(TextBlocks { rev, blocks, areas, locked, signed: crate::outline::signed(d) })
    })
}

/// The gap between the block's first two baselines over its font size, as shown on the page.
fn line_spacing(b: &text::Block, lines: &[text::Line], m: &Matrix) -> Option<f32> {
    let (first, second) = (b.lines.first()?, b.lines.get(1)?);
    let gap = (m.bbox(lines[*second].rect)[3] - m.bbox(lines[*first].rect)[3]).abs();
    (b.size > 0.0 && gap > 0.0).then(|| (gap / b.size) as f32)
}

/// Whether `runs` set the block's own text again (only their styles differ): a formatting change.
fn keeps_text(d: &PdfDocument, page: u32, index: u32, runs: &[RunChange]) -> bool {
    let Ok(pc) = text::read_page(d, page) else { return false };
    let Some(b) = text::group_blocks(&pc.lines).into_iter().nth(index as usize) else { return false };
    let new: Option<String> = runs
        .iter()
        .map(|r| r.text.clone().or_else(|| r.source.and_then(|i| b.runs.get(i)).map(|s| s.text.clone())))
        .collect();
    new.is_some_and(|t| t == b.runs.iter().map(|r| r.text.as_str()).collect::<String>())
}

fn label(c: &BlockChange, page: u32, formatting_only: bool) -> String {
    let what = if c.text.is_some() || (c.runs.is_some() && !formatting_only) {
        "Edit text"
    } else if c.offset.is_some()
        && c.size.is_none()
        && c.color.is_none()
        && c.align.is_none()
        && c.width.is_none()
        && c.line_spacing.is_none()
        && c.bold.is_none()
        && c.italic.is_none()
        && c.underline.is_none()
        && c.strike.is_none()
        && c.rise.is_none()
    {
        "Move text"
    } else if c.width.is_some()
        && c.size.is_none()
        && c.color.is_none()
        && c.align.is_none()
        && c.line_spacing.is_none()
        && c.bold.is_none()
        && c.italic.is_none()
        && c.underline.is_none()
        && c.strike.is_none()
        && c.rise.is_none()
    {
        "Resize text box"
    } else {
        "Format text"
    };
    format!("{what} on p. {}", page + 1)
}

/// Applies `change` to block `index` of `page`, listed at generation `rev`: one undo step.
/// `Stale` when the document has changed since that list.
pub fn edit(eng: &Engine, doc: DocId, rev: u32, page: u32, index: u32, change: &BlockChange, source: EditSource) -> Result<TextEdited> {
    let entry = eng.doc_entry(doc)?;
    if *change == BlockChange::default() {
        return Err(EngineError::Invalid("nothing to change".into()));
    }
    let mut fallbacks = Vec::new();
    let formatting_only = match &change.runs {
        Some(runs) => eng.read_custom(doc, |d| Ok(keeps_text(d, page, index, runs))).unwrap_or(false),
        None => false,
    };
    let state = eng.apply_removing(doc, label(change, page, formatting_only), source, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        crate::outline::refuse_if_certified(d, "Editing its text")?;
        // Page space → user space for the move and the width.
        let back = page_space(d, page)?.invert().ok_or_else(|| EngineError::Invalid("the page has no area".into()))?;
        let offset = change.offset.map(|[x, y]| {
            let (ux, uy) = back.apply_vec(f64::from(x), f64::from(y));
            [ux, uy]
        });
        let width = change.width.map(|w| {
            let (ux, uy) = back.apply_vec(f64::from(w), 0.0);
            (ux * ux + uy * uy).sqrt()
        });
        let c = TextChange {
            text: change.text.clone(),
            size: change.size.map(f64::from),
            color: change.color.map(|c| c.map(f64::from)),
            align: change.align,
            offset,
            width,
            line_spacing: change.line_spacing.map(f64::from),
            bold: change.bold,
            italic: change.italic,
            underline: change.underline,
            strike: change.strike,
            rise: change.rise,
            runs: change.runs.clone(),
        };
        fallbacks = text::rewrite_block(d, page, index as usize, &c)?;
        Ok(vec![page])
    })?;
    Ok(TextEdited { state, fallback: fallbacks.first().cloned(), fallbacks })
}

/// New text to put on a page.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewTextBox {
    pub text: String,
    /// Points.
    pub size: f32,
    /// RGB 0..1.
    pub color: [f32; 3],
    /// Top left of the box in page space.
    pub at: [f32; 2],
    /// Wrap to this width (page space); none keeps the lines as typed.
    pub width: Option<f32>,
}

/// Adds a text box to `page` (0-based), listed at generation `rev`: one undo step.
pub fn add(eng: &Engine, doc: DocId, rev: u32, page: u32, new: &NewTextBox, source: EditSource) -> Result<EditState> {
    let entry = eng.doc_entry(doc)?;
    eng.apply_custom(doc, format!("Add text on p. {}", page + 1), source, false, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        crate::outline::refuse_if_certified(d, "Adding text")?;
        let back = page_space(d, page)?.invert().ok_or_else(|| EngineError::Invalid("the page has no area".into()))?;
        let n = text::NewText {
            text: &new.text,
            size: f64::from(new.size),
            color: new.color.map(f64::from),
            at: new.at.map(f64::from),
            width: new.width.map(f64::from),
            back,
        };
        text::add_text(d, page, &n)?;
        Ok(vec![page])
    })
}

/// A font the UI can offer for new or restyled text.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FontInfo {
    /// Pass as `RunChange::font`.
    pub id: String,
    /// What to show: the font's name without its subset tag.
    pub name: String,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// `document`, `standard` or `bundled`.
    pub kind: &'static str,
    /// The font program is in the PDF (document fonts; bundled faces are embedded as subsets).
    pub embedded: bool,
    /// Only the glyphs the document used are in it: typed text may fall back to another font.
    pub subset: bool,
    /// New text can be written in it (not Type 3, not a predefined CJK encoding, with a Unicode map).
    pub usable: bool,
    /// Pages (0-based) whose resources name it (document fonts).
    pub pages: Vec<u32>,
}

/// The fonts the font picker offers: the document's own (in page order), the standard 14 faces
/// and the bundled faces.
pub fn fonts(eng: &Engine, doc: DocId) -> Result<Vec<FontInfo>> {
    eng.read_custom(doc, |d| {
        let mut out: Vec<FontInfo> = Vec::new();
        let n = d.page_count()?.max(0);
        for p in 0..n {
            let page = d.find_page(p)?;
            let Some(fonts) = page.get_dict_inheritable("Resources")?.and_then(|r| r.get_dict("Font").ok().flatten()) else { continue };
            for i in 0..fonts.dict_len()? as i32 {
                let (Some(k), Some(v)) = (fonts.get_dict_key(i)?, fonts.get_dict_val(i)?) else { continue };
                // A font object is one entry for all pages; a direct dict is named on its page only.
                let id = match v.is_indirect()? {
                    true => format!("doc:{}", v.as_indirect()?),
                    false => format!("res:{}", String::from_utf8_lossy(&k.as_name()?)),
                };
                if let Some(f) = out.iter_mut().find(|f| f.id == id && (id.starts_with("doc:") || f.pages == [p as u32])) {
                    if !f.pages.contains(&(p as u32)) {
                        f.pages.push(p as u32);
                    }
                    continue;
                }
                let Some(dict) = v.resolve()?.filter(|o| o.is_dict().unwrap_or(false)) else { continue };
                let m = font::Metrics::summary(&dict);
                let name = match m.base_font.split_once('+') {
                    Some((t, rest)) if t.len() == 6 => rest.to_string(),
                    _ => m.base_font.clone(),
                };
                out.push(FontInfo {
                    id,
                    family: layout::family_key(&m.base_font),
                    name,
                    bold: m.bold,
                    italic: m.italic,
                    kind: "document",
                    embedded: m.embedded,
                    subset: m.subset,
                    usable: m.usable,
                    pages: vec![p as u32],
                });
            }
        }
        for row in font::STD14 {
            for (k, name) in row.iter().enumerate() {
                out.push(FontInfo {
                    id: format!("std:{name}"),
                    name: (*name).to_string(),
                    family: name.split('-').next().unwrap_or(name).to_string(),
                    bold: k % 2 == 1,
                    italic: k >= 2,
                    kind: "standard",
                    embedded: false,
                    subset: false,
                    usable: true,
                    pages: Vec::new(),
                });
            }
        }
        for f in &bundled::FACES {
            out.push(FontInfo {
                id: format!("bundled:{}", f.id),
                name: f.ps_name.to_string(),
                family: f.family.to_string(),
                bold: f.bold,
                italic: f.italic,
                kind: "bundled",
                embedded: true,
                subset: true,
                usable: true,
                pages: Vec::new(),
            });
        }
        Ok(out)
    })
}

#[cfg(test)]
mod tests;
