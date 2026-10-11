//! The checks added toward the Matterhorn Protocol (PDF/UA-1) failure conditions that software can
//! decide: content that is neither tagged nor an artifact, artifacts and tags nested in each other,
//! the role map, XMP identification, the accessibility permission, table structure, annotations in
//! the tree, embedded fonts, XFA, optional content configurations and language values. Which failure
//! conditions each rule covers, and which are left to a person, is the table in docs/a11y.md.

use super::content::{self, Op, Operand};
use super::tree::{Tree, is_standard, is_standard_17, name, text};
use super::{Finding, Result, dict, res_lookup};
use mupdf::pdf::PdfObject;
use std::collections::HashSet;

// ---- marked content on a page --------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mark {
    /// `BDC` with an MCID: real content, tied to the tree.
    Tagged,
    /// `/Artifact BMC` or `/Artifact ... BDC`.
    Artifact,
    /// Anything else (optional content, spans without an MCID).
    Other,
}

/// What one page paints, and how it is marked.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Marking {
    /// Painted outside tagged content and artifacts, by kind.
    pub text: u32,
    pub paths: u32,
    pub images: u32,
    pub shadings: u32,
    /// `Artifact` inside tagged content.
    pub artifact_in_tagged: u32,
    /// Tagged content inside an `Artifact`.
    pub tagged_in_artifact: u32,
}

impl Marking {
    pub fn untagged(&self) -> u32 {
        self.text + self.paths + self.images + self.shadings
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for (n, what) in [(self.text, "text"), (self.paths, "path"), (self.images, "image"), (self.shadings, "shading")] {
            if n > 0 {
                parts.push(format!("{n} {what}{}", if n == 1 || what == "text" { "" } else { "s" }));
            }
        }
        parts.join(", ")
    }
}

/// Follows one top-level operator of a page's content.
pub fn track(op: &Op, res: Option<&PdfObject>, stack: &mut Vec<Mark>, m: &mut Marking) -> Result<()> {
    match op.name.as_slice() {
        b"BMC" | b"BDC" => {
            let tag = op.operands.first().and_then(Operand::name);
            let has_mcid = match op.operands.get(1) {
                Some(d @ Operand::Dict(_)) => d.get(b"MCID").is_some(),
                Some(Operand::Name(n)) => match res_lookup(res, "Properties", n)? {
                    Some(p) => p.is_dict()? && p.get_dict("MCID")?.is_some(),
                    None => false,
                },
                _ => false,
            };
            let mark = if tag == Some(b"Artifact") {
                Mark::Artifact
            } else if has_mcid {
                Mark::Tagged
            } else {
                Mark::Other
            };
            if mark == Mark::Artifact && stack.contains(&Mark::Tagged) {
                m.artifact_in_tagged += 1;
            }
            if mark == Mark::Tagged && stack.contains(&Mark::Artifact) {
                m.tagged_in_artifact += 1;
            }
            stack.push(mark);
        }
        b"EMC" => {
            stack.pop();
        }
        _ => {
            let covered = stack.iter().any(|k| matches!(k, Mark::Tagged | Mark::Artifact));
            if covered {
                return Ok(());
            }
            if content::shows(op) {
                m.text += 1;
            } else if content::paints_path(op) {
                m.paths += 1;
            } else if op.is(b"sh") {
                m.shadings += 1;
            } else if op.is(b"BI") {
                m.images += 1;
            } else if op.is(b"Do")
                && let Some(n) = op.operands.first().and_then(Operand::name)
                && let Some(x) = res_lookup(res, "XObject", n)?
            {
                // A form's own content is checked where the form is read (see `scan_stream`).
                if name(&x, "Subtype")?.as_deref() == Some("Image") {
                    m.images += 1;
                }
            }
        }
    }
    Ok(())
}

// ---- the role map --------------------------------------------------------------------------------

/// Types that are not standard and map to nothing standard, circular maps, and standard types that are remapped.
pub fn role_map(t: &Tree) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for i in t.preorder() {
        let e = &t.elems[i];
        if !seen.insert(e.raw.clone()) {
            continue;
        }
        let what = if is_standard_17(&e.raw) {
            t.role_map.get(&e.raw).filter(|to| **to != e.raw).map(|to| format!("the standard type {} is remapped to {to}; standard types may not be remapped", e.raw))
        } else {
            let mut cur = e.raw.clone();
            let mut visited = HashSet::new();
            loop {
                if is_standard_17(&cur) {
                    break None;
                }
                if !visited.insert(cur.clone()) {
                    break Some(format!("the type {} is mapped in a circle", e.raw));
                }
                match t.role_map.get(&cur) {
                    Some(next) => cur = next.clone(),
                    None if is_standard(&e.raw) => {
                        break Some(format!("the type {} is from PDF 2.0; PDF/UA-1 needs a role map entry from it to a PDF 1.7 type", e.raw));
                    }
                    None => break Some(format!("the type {} is not mapped to a standard type", e.raw)),
                }
            }
        };
        if let Some(w) = what {
            out.push(Finding {
                page: e.page,
                object: Some(e.raw.clone()),
                xref: (e.xref != 0).then_some(e.xref),
                rect: None,
                message: format!("Role map: {w}"),
            });
        }
    }
    out
}

// ---- tables --------------------------------------------------------------------------------------

fn span(e: &super::tree::Elem, key: &str) -> usize {
    e.attrs.get(key).and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| *n >= 1).unwrap_or(1)
}

/// Tables whose parts are in the wrong place, whose rows don't line up, or whose cells name header
/// cells that don't exist.
pub fn table_structure(t: &Tree, loc: &super::locate::Locator) -> Vec<Finding> {
    let mut out = Vec::new();
    let ids: HashSet<&str> = t.elems.iter().filter_map(|e| e.id.as_deref()).collect();
    let find = |i: usize, why: &str| super::elem_finding(t, loc, i, why);
    for i in t.preorder() {
        let e = &t.elems[i];
        match e.ty.as_str() {
            "Table" => {
                let stray = e.children.iter().filter(|&&c| !matches!(t.elems[c].ty.as_str(), "TR" | "THead" | "TBody" | "TFoot" | "Caption" | "NonStruct" | "Artifact")).count();
                if stray > 0 {
                    out.push(find(i, &format!("{stray} kid{} that are not rows (TR), THead, TBody, TFoot or Caption", if stray == 1 { "" } else { "s" })));
                }
                let rows = t.table_rows(i);
                if rows.len() > 1 {
                    // Column count per row, with cells that span several rows counted in the rows they cover.
                    let mut carry: Vec<(usize, usize)> = Vec::new();
                    let mut widths = Vec::new();
                    for &r in &rows {
                        let cells: Vec<usize> = t.elems[r].children.iter().copied().filter(|&c| matches!(t.elems[c].ty.as_str(), "TH" | "TD")).collect();
                        let mut w: usize = cells.iter().map(|&c| span(&t.elems[c], "ColSpan")).sum();
                        w += carry.iter().filter(|(left, _)| *left > 0).map(|(_, cols)| cols).sum::<usize>();
                        carry.iter_mut().for_each(|(left, _)| *left = left.saturating_sub(1));
                        carry.retain(|(left, _)| *left > 0);
                        for &c in &cells {
                            let rs = span(&t.elems[c], "RowSpan");
                            if rs > 1 {
                                carry.push((rs - 1, span(&t.elems[c], "ColSpan")));
                            }
                        }
                        widths.push(w);
                    }
                    if widths.iter().any(|w| *w != widths[0]) {
                        let (lo, hi) = (widths.iter().min().copied().unwrap_or(0), widths.iter().max().copied().unwrap_or(0));
                        out.push(find(i, &format!("rows have different numbers of columns ({lo} to {hi}); a table must be regular")));
                    }
                }
            }
            "THead" | "TBody" | "TFoot" => {
                if !t.parent_is(i, &["Table"]) {
                    out.push(find(i, "not inside a Table"));
                }
                if e.children.iter().any(|&c| !matches!(t.elems[c].ty.as_str(), "TR" | "NonStruct" | "Artifact")) {
                    out.push(find(i, "holds something other than rows (TR)"));
                }
            }
            "TR" => {
                if !t.parent_is(i, &["Table", "THead", "TBody", "TFoot"]) {
                    out.push(find(i, "a row outside a Table, THead, TBody or TFoot"));
                }
                if e.children.iter().any(|&c| !matches!(t.elems[c].ty.as_str(), "TH" | "TD" | "NonStruct" | "Artifact")) {
                    out.push(find(i, "holds something other than cells (TH or TD)"));
                }
            }
            "TH" | "TD" if !t.parent_is(i, &["TR"]) => out.push(find(i, "a cell outside a row (TR)")),
            _ => {}
        }
        for h in &e.headers {
            if !ids.contains(h.as_str()) {
                out.push(find(i, &format!("lists the header \"{h}\", which no element has as its ID")));
            }
        }
    }
    out
}

/// Header cells (TH) without a `Scope`, in tables whose cells don't name their headers with `/Headers`.
pub fn table_scope(t: &Tree, loc: &super::locate::Locator) -> Vec<Finding> {
    let table_of = |i: usize| t.ancestors(i).find(|&a| t.is(a, "Table"));
    let named: HashSet<usize> = (0..t.elems.len()).filter(|&i| !t.elems[i].headers.is_empty()).filter_map(table_of).collect();
    let mut out = Vec::new();
    for i in t.preorder() {
        if t.is(i, "TH") && !t.elems[i].attrs.contains_key("Scope") && !table_of(i).is_some_and(|tb| named.contains(&tb)) {
            out.push(super::elem_finding(t, loc, i, "header cell without a Scope (Row, Column or Both)"));
        }
    }
    out
}

// ---- document-level checks -----------------------------------------------------------------------

/// The XMP packet as text, if the catalog has one.
pub fn xmp(cat: &PdfObject) -> Result<Option<String>> {
    Ok(match cat.get_dict("Metadata")? {
        Some(m) if m.is_stream()? => Some(String::from_utf8_lossy(&m.read_stream().unwrap_or_default()).into_owned()),
        _ => None,
    })
}

/// The XMP packet for a fix: it must be UTF-8 (a UTF-16 packet would be misread as text).
pub fn xmp_strict(cat: &PdfObject) -> Result<Option<String>> {
    Ok(match cat.get_dict("Metadata")? {
        Some(m) if m.is_stream()? => {
            let bytes = m.read_stream_capped(4 << 20).map_err(|_| super::bad("the XMP metadata is too large to edit"))?;
            Some(String::from_utf8(bytes).map_err(|_| super::bad("the XMP metadata isn't UTF-8, so it isn't edited here; fix it in the source application"))?)
        }
        _ => None,
    })
}

pub fn has_dc_title(x: &str) -> bool {
    let Some(at) = x.find("<dc:title") else { return false };
    let rest = &x[at..];
    let end = rest.find("</dc:title>").unwrap_or(rest.len());
    // Something between the tags other than markup, after the opening tag itself.
    let mut in_tag = true;
    let mut text = false;
    for c in rest[..end].chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() => text = true,
            _ => {}
        }
    }
    text
}

/// The value of `pdfuaid:part`, as an element (`<pdfuaid:part>1</pdfuaid:part>`) or an attribute.
fn pdfua_part(x: &str) -> Option<String> {
    if let Some(at) = x.find("<pdfuaid:part>") {
        let rest = &x[at + "<pdfuaid:part>".len()..];
        return Some(rest[..rest.find('<').unwrap_or(rest.len())].trim().to_owned());
    }
    let at = x.find("pdfuaid:part=")? + "pdfuaid:part=".len();
    let rest = &x[at..];
    let quote = rest.chars().next().filter(|q| matches!(q, '"' | '\''))?;
    let body = &rest[1..];
    Some(body[..body.find(quote)?].trim().to_owned())
}

/// The file identifies itself as PDF/UA-1 (part exactly `1`, not `10`).
pub fn has_pdfua_id(x: &str) -> bool {
    pdfua_part(x).as_deref() == Some("1")
}

/// The identification and title a PDF/UA file carries in its XMP metadata.
pub fn metadata(cat: &PdfObject) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    match xmp(cat)? {
        None => out.push(Finding::doc("The document has no XMP metadata stream")),
        Some(x) => {
            if !has_pdfua_id(&x) {
                out.push(Finding::doc("The XMP metadata doesn't identify the file as PDF/UA (pdfuaid:part 1)"));
            }
            if !has_dc_title(&x) {
                out.push(Finding::doc("The XMP metadata has no dc:title"));
            }
        }
    }
    Ok(out)
}

/// The document is encrypted and its permissions don't allow extracting text for accessibility (bit 10 of /P).
pub fn permissions(doc: &mupdf::pdf::PdfDocument) -> Result<Vec<Finding>> {
    let Some(enc) = dict(&doc.trailer()?, "Encrypt")? else { return Ok(Vec::new()) };
    let p = enc.get_dict("P")?.and_then(|v| v.as_int().ok());
    Ok(match p {
        Some(p) if p & 0x200 == 0 => vec![Finding::doc("Encryption doesn't allow text extraction for accessibility (permission bit 10 is off)")],
        _ => Vec::new(),
    })
}

/// A form is described by dynamic XFA (`/XFA` with `/NeedsRendering true`): its pages are drawn when
/// the file is opened, so it can't be tagged. Static XFA has real pages and is not flagged.
pub fn xfa(cat: &PdfObject) -> Result<Vec<Finding>> {
    let Some(form) = dict(cat, "AcroForm")? else { return Ok(Vec::new()) };
    if form.get_dict("XFA")?.is_none() {
        return Ok(Vec::new());
    }
    let dynamic = super::is_true(cat.get_dict("NeedsRendering")?)? || super::is_true(form.get_dict("NeedsRendering")?)?;
    Ok(if dynamic { vec![Finding::doc("The form is a dynamic XFA form (/XFA with /NeedsRendering true), which can't be made accessible")] } else { Vec::new() })
}

/// Optional content configurations need a name, and none may set /AS (automatic state).
pub fn optional_content(cat: &PdfObject) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    let Some(oc) = dict(cat, "OCProperties")? else { return Ok(out) };
    let mut configs: Vec<(String, PdfObject)> = Vec::new();
    if let Some(d) = dict(&oc, "D")? {
        configs.push(("Default configuration".into(), d));
    }
    if let Some(a) = oc.get_dict("Configs")?.filter(|a| a.is_array().unwrap_or(false)) {
        for (n, c) in a.array_iter()?.enumerate() {
            let c = c?;
            if c.is_dict()? {
                configs.push((format!("Configuration {}", n + 1), c));
            }
        }
    }
    for (label, c) in configs {
        if text(&c, "Name")?.is_none() {
            out.push(Finding::doc(format!("{label} of the optional content has no /Name")));
        }
        if c.get_dict("AS")?.is_some() {
            out.push(Finding::doc(format!("{label} of the optional content sets /AS (automatic state), which PDF/UA forbids")));
        }
    }
    Ok(out)
}

/// A font program is embedded (Type3 glyphs are drawn in the file, so they count).
pub fn font_embedded(f: &PdfObject) -> Result<bool> {
    let sub = name(f, "Subtype")?;
    if sub.as_deref() == Some("Type3") {
        return Ok(true);
    }
    let source = if sub.as_deref() == Some("Type0") {
        match f.get_dict("DescendantFonts")?.filter(|d| d.is_array().unwrap_or(false)).and_then(|d| d.get_array(0).ok().flatten()) {
            Some(d) => d,
            None => return Ok(false),
        }
    } else {
        f.clone()
    };
    Ok(match dict(&source, "FontDescriptor")? {
        Some(fd) => ["FontFile", "FontFile2", "FontFile3"].iter().any(|k| fd.get_dict(*k).ok().flatten().is_some()),
        None => false,
    })
}

// ---- XMP for the fix ------------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn description(title: Option<&str>, pdfua: bool) -> String {
    let mut d = String::from("<rdf:Description rdf:about=\"\"");
    if title.is_some() {
        d.push_str(" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"");
    }
    if pdfua {
        d.push_str(" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"");
    }
    d.push('>');
    if let Some(t) = title {
        d.push_str(&format!("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>", xml_escape(t)));
    }
    if pdfua {
        d.push_str("<pdfuaid:part>1</pdfuaid:part>");
    }
    d.push_str("</rdf:Description>");
    d
}

/// An XMP packet with a title and the PDF/UA identification.
pub fn new_xmp(title: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{}</rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>",
        description(Some(title), true)
    )
}

/// An existing packet with what it lacks added as another description block. Refuses a packet that
/// names another PDF/UA part, has an empty `dc:title` to replace, or has no `</rdf:RDF>` to extend.
pub fn amend_xmp(x: &str, title: &str) -> Result<String> {
    let (need_id, need_title) = (!has_pdfua_id(x), !has_dc_title(x));
    if !need_id && !need_title {
        return Ok(x.to_owned());
    }
    if need_id && x.contains("pdfuaid:part") {
        return Err(super::bad("the XMP metadata names a PDF/UA part other than 1, which this won't change"));
    }
    if need_title && x.contains("<dc:title") {
        return Err(super::bad("the XMP metadata has an empty dc:title; fix it in the source application"));
    }
    let at = x.rfind("</rdf:RDF>").ok_or_else(|| super::bad("the XMP metadata has no rdf:RDF element to extend"))?;
    let block = description(need_title.then_some(title), need_id);
    Ok(format!("{}{}{}", &x[..at], block, &x[at..]))
}
