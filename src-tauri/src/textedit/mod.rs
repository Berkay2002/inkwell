//! Edit existing text in place (Acrobat "Edit text & images" for text): list a page's text
//! blocks, and replace a block's text, change its size, colour and alignment, move it or resize it.
//!
//! Every change is one `Engine::apply_custom` step on the authoritative document: one undo step,
//! kept by Save, recorded in history. Coordinates in and out are the viewer's page space (points,
//! origin at the top left of the page as shown, `/Rotate` applied: the space of `page_text`).
//! See docs/text-edit.md.

pub mod app;
mod content;
mod encodings;
mod font;
mod text;

pub use text::{Align, Fallback, Issue, TextChange};

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use content::Matrix;
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
    pub editable: bool,
    /// Why it can't be edited.
    pub issue: Option<Issue>,
    /// The reason as a sentence for the UI.
    pub reason: Option<String>,
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
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextEdited {
    pub state: EditState,
    /// Set when the block's font could not show the text and another one was used.
    pub fallback: Option<Fallback>,
}

/// User space → page space, and the page's origin there.
fn page_space(doc: &PdfDocument, page: u32) -> Result<Matrix> {
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
        Ok(TextBlocks { rev, blocks, areas })
    })
}

fn label(c: &BlockChange, page: u32) -> String {
    let what = if c.text.is_some() {
        "Edit text"
    } else if c.offset.is_some() && c.size.is_none() && c.color.is_none() && c.align.is_none() && c.width.is_none() {
        "Move text"
    } else if c.width.is_some() && c.size.is_none() && c.color.is_none() && c.align.is_none() {
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
    let mut fallback = None;
    let state = eng.apply_custom(doc, label(change, page), source, false, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
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
        };
        fallback = text::rewrite_block(d, page, index as usize, &c)?;
        Ok(vec![page])
    })?;
    Ok(TextEdited { state, fallback })
}

#[cfg(test)]
mod tests;
