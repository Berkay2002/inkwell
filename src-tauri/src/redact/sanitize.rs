//! Sanitize: removes the leak channels next to the page content (Acrobat's "Sanitize and remove
//! hidden information", plus what Acrobat misses: catalog/page /AF associated files, orphan
//! popups). Independent of the per-category switches, anything that quotes a redacted string
//! (annotation, form field, docinfo entry, XMP packet, bookmark) is always removed.

use super::{RedactError, annots_of, needle_regex, scan_strings, subtype};
use mupdf::pdf::{PdfDocument, PdfObject, PdfRedactImageMethod, PdfRedactLineArtMethod, PdfRedactOptions, PdfRedactTextMethod};
use mupdf::text_page::{TextCharFlags, TextPageFlags};
use mupdf::{Buffer, Outline};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

type Result<T> = std::result::Result<T, RedactError>;

/// What to strip. All on by default (Acrobat's sanitize checkbox is on by default too).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sanitize {
    /// Docinfo, XMP (catalog and page /Metadata), /PieceInfo.
    pub metadata: bool,
    /// Content in optional-content layers that are off, then /OCProperties.
    pub hidden_layers: bool,
    /// Invisible text (render mode 3 etc.) anywhere. Note: removes OCR text layers of scans too.
    pub hidden_text: bool,
    /// JavaScript, OpenAction, additional actions, annotation actions, links, XFA.
    pub scripts_actions: bool,
    /// Embedded files: the EmbeddedFiles name tree, catalog and page /AF, file attachment annotations.
    pub embedded_files: bool,
    /// Comments and markups (every annotation except form widgets and links), incl. popups.
    pub annotations: bool,
    /// Form fields whose widget touches a mark (used by `apply`).
    pub form_fields_under_marks: bool,
    pub bookmarks: bool,
}

impl Default for Sanitize {
    fn default() -> Self {
        Self {
            metadata: true,
            hidden_layers: true,
            hidden_text: true,
            scripts_actions: true,
            embedded_files: true,
            annotations: true,
            form_fields_under_marks: true,
            bookmarks: true,
        }
    }
}

impl Sanitize {
    pub const NONE: Sanitize = Sanitize {
        metadata: false,
        hidden_layers: false,
        hidden_text: false,
        scripts_actions: false,
        embedded_files: false,
        annotations: false,
        form_fields_under_marks: false,
        bookmarks: false,
    };
}

/// Items removed per category.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizeReport {
    pub metadata: u32,
    pub hidden_layer_blocks: u32,
    pub hidden_text_pages: u32,
    pub scripts_actions: u32,
    pub embedded_files: u32,
    pub annotations: u32,
    pub form_fields: u32,
    pub bookmarks: u32,
}

fn del(o: &mut PdfObject, key: &str) -> Result<u32> {
    Ok(if o.get_dict(key)?.is_some() {
        o.dict_delete(key)?;
        1
    } else {
        0
    })
}

fn num(o: &PdfObject) -> Result<i32> {
    Ok(if o.is_indirect()? { o.as_indirect()? } else { 0 })
}

fn quotes(res: &[Regex], s: &str) -> bool {
    res.iter().any(|r| r.is_match(s))
}

fn pages(doc: &PdfDocument) -> Result<Vec<PdfObject>> {
    (0..doc.page_count()?).map(|p| Ok(doc.find_page(p)?)).collect()
}

/// Sanitizes `doc`. `needles` = redacted strings (empty when sanitizing without a redaction).
pub fn sanitize(doc: &mut PdfDocument, o: &Sanitize, needles: &[String]) -> Result<SanitizeReport> {
    let res: Vec<Regex> = needles.iter().map(|n| Regex::new(&needle_regex(n))).collect::<std::result::Result<_, _>>()?;
    let mut r = SanitizeReport::default();
    let mut cat = doc.catalog()?;

    if o.hidden_layers {
        r.hidden_layer_blocks = strip_hidden_layers(doc)?;
    }
    if o.hidden_text {
        r.hidden_text_pages = strip_invisible_text(doc)?;
    }

    if o.scripts_actions {
        r.scripts_actions += del(&mut cat, "OpenAction")? + del(&mut cat, "AA")?;
        if let Some(mut names) = cat.get_dict("Names")? {
            r.scripts_actions += del(&mut names, "JavaScript")?;
        }
        if let Some(mut af) = cat.get_dict("AcroForm")? {
            r.scripts_actions += del(&mut af, "XFA")?;
        }
    }
    if o.embedded_files {
        r.embedded_files += doc.embedded_files()?.len() as u32;
        if let Some(mut names) = cat.get_dict("Names")? {
            del(&mut names, "EmbeddedFiles")?;
        }
        if let Some(af) = cat.get_dict("AF")? {
            r.embedded_files += af.len()? as u32;
            cat.dict_delete("AF")?;
        }
        del(&mut cat, "Collection")?;
    }
    if let Some(names) = cat.get_dict("Names")?
        && names.dict_len()? == 0
    {
        cat.dict_delete("Names")?;
    }
    if o.metadata {
        r.metadata += del(&mut cat, "Metadata")? + del(&mut cat, "PieceInfo")?;
        r.metadata += del(&mut doc.trailer()?, "Info")?;
    }

    for mut page in pages(doc)? {
        if o.scripts_actions {
            r.scripts_actions += del(&mut page, "AA")?;
        }
        if o.embedded_files {
            r.embedded_files += del(&mut page, "AF")?;
        }
        if o.metadata {
            r.metadata += del(&mut page, "Metadata")? + del(&mut page, "PieceInfo")?;
        }
        let mut quoted = Vec::new();
        let mut links = 0;
        let n = drop_annots(&mut page, |a, st| {
            let quoting = {
                let mut q = false;
                scan_strings(a, 0, &mut |s| q |= quotes(&res, s))?;
                q
            };
            if st == b"Widget" {
                if quoting {
                    quoted.push(a.clone());
                }
                return Ok(quoting);
            }
            if st == b"Link" && (quoting || o.scripts_actions) {
                links += 1;
                return Ok(true);
            }
            Ok(quoting || (o.annotations && st != b"Link") || (o.embedded_files && st == b"FileAttachment"))
        })?;
        r.annotations += n - links;
        r.scripts_actions += links;
        for w in quoted {
            remove_field(doc, &w)?;
            r.form_fields += 1;
        }
        if o.scripts_actions {
            for mut a in annots_of(&page)? {
                r.scripts_actions += del(&mut a, "A")? + del(&mut a, "AA")?;
            }
        }
    }

    if o.bookmarks {
        r.bookmarks = count_outlines(&doc.outlines()?);
        doc.delete_outlines()?;
        del(&mut cat, "Outlines")?;
        if cat.get_dict("PageMode")?.and_then(|m| m.as_name().ok()).as_deref() == Some(b"UseOutlines") {
            cat.dict_delete("PageMode")?;
        }
    } else if !res.is_empty() {
        let toc = doc.outlines()?;
        let kept = filter_outlines(&toc, &res);
        let removed = count_outlines(&toc) - count_outlines(&kept);
        if removed > 0 {
            doc.set_outlines(&kept)?;
            r.bookmarks += removed;
        }
    }

    // Quoting metadata goes even when metadata is kept.
    if !res.is_empty() {
        if let Some(mut info) = doc.trailer()?.get_dict("Info")? {
            let mut keys = Vec::new();
            for i in 0..info.dict_len()? {
                if let (Some(k), Some(v)) = (info.get_dict_key(i as i32)?, info.get_dict_val(i as i32)?)
                    && v.is_string()?
                    && quotes(&res, &v.as_string()?)
                {
                    keys.push(String::from_utf8_lossy(&k.as_name()?).into_owned());
                }
            }
            for k in keys {
                r.metadata += del(&mut info, &k)?;
            }
        }
        if let Some(xmp) = cat.get_dict("Metadata")?
            && quotes(&res, &String::from_utf8_lossy(&xmp.read_stream()?))
        {
            r.metadata += del(&mut cat, "Metadata")?;
        }
    }
    Ok(r)
}

fn count_outlines(t: &[Outline]) -> u32 {
    t.iter().map(|o| 1 + count_outlines(&o.down)).sum()
}

/// Drops bookmarks whose title quotes a redacted string; their children move up a level.
fn filter_outlines(t: &[Outline], res: &[Regex]) -> Vec<Outline> {
    let mut out = Vec::new();
    for o in t {
        let down = filter_outlines(&o.down, res);
        if quotes(res, &o.title) {
            out.extend(down);
        } else {
            out.push(Outline { title: o.title.clone(), uri: o.uri.clone(), dest: o.dest, down });
        }
    }
    out
}

/// Removes annotations for which `drop(annot, subtype)` says so from the page's /Annots, plus the
/// popups that belong to them. Returns how many non-popup annotations went.
pub(super) fn drop_annots(
    page: &mut PdfObject,
    mut drop: impl FnMut(&PdfObject, &[u8]) -> Result<bool>,
) -> Result<u32> {
    let Some(mut arr) = page.get_dict("Annots")? else { return Ok(0) };
    let mut gone = Vec::new();
    let mut gone_nums = HashSet::new();
    let mut popups = Vec::new();
    for i in 0..arr.len()? as i32 {
        let Some(a) = arr.get_array(i)? else { continue };
        if !a.is_dict()? {
            continue;
        }
        let st = subtype(&a)?;
        if st == b"Popup" {
            popups.push((i, a));
        } else if drop(&a, &st)? {
            gone.push(i);
            gone_nums.insert(num(&a)?);
            if let Some(p) = a.get_dict("Popup")? {
                gone_nums.insert(num(&p)?);
            }
        }
    }
    let count = gone.len() as u32;
    for (i, p) in popups {
        let parent = p.get_dict("Parent")?.map(|p| num(&p)).transpose()?;
        let orphan = parent.is_none_or(|n| gone_nums.contains(&n));
        if gone_nums.contains(&num(&p)?) || orphan || drop(&p, b"Popup")? {
            gone.push(i);
        }
    }
    gone.sort_unstable();
    for i in gone.into_iter().rev() {
        arr.array_delete(i)?;
    }
    Ok(count)
}

/// Removes the form field a widget belongs to: the widget from its parent's /Kids, and parents
/// that end up empty, up to /AcroForm /Fields.
pub(super) fn remove_field(doc: &PdfDocument, widget: &PdfObject) -> Result<()> {
    let mut node = widget.clone();
    loop {
        let n = num(&node)?;
        let (mut list, parent) = match node.get_dict("Parent")? {
            Some(p) => (p.get_dict("Kids")?, Some(p)),
            None => (doc.catalog()?.get_dict("AcroForm")?.map(|f| f.get_dict("Fields")).transpose()?.flatten(), None),
        };
        if let Some(list) = list.as_mut() {
            for i in (0..list.len()? as i32).rev() {
                if list.get_array(i)?.map(|k| num(&k)).transpose()? == Some(n) {
                    list.array_delete(i)?;
                }
            }
        }
        match parent {
            Some(p) if list.map_or(Ok(0), |l| l.len())? == 0 => node = p,
            _ => return Ok(()),
        }
    }
}

pub(super) fn is_image(o: &PdfObject) -> Result<bool> {
    Ok(o.get_dict("Subtype")?.map(|s| s.as_name()).transpose()?.as_deref() == Some(b"Image"))
}

/// Removes invisible text page by page with a page-sized redaction that only targets invisible text.
fn strip_invisible_text(doc: &mut PdfDocument) -> Result<u32> {
    let mut n = 0;
    for p in 0..doc.page_count()? {
        let mut page = doc.load_pdf_page(p)?;
        let invisible = page
            .to_text_page(TextPageFlags::empty())?
            .blocks()
            .flat_map(|b| b.lines().flat_map(|l| l.chars().map(|c| c.flags()).collect::<Vec<_>>()).collect::<Vec<_>>())
            .any(|f| !f.intersects(TextCharFlags::FILLED | TextCharFlags::STROKED));
        if !invisible {
            continue;
        }
        let bounds = page.bounds()?;
        let annot = page.add_redact_annotation(bounds)?;
        annot.apply_redaction_with_options(PdfRedactOptions {
            black_boxes: false,
            image_method: PdfRedactImageMethod::None,
            line_art: PdfRedactLineArtMethod::None,
            text: PdfRedactTextMethod::RemoveInvisible,
        })?;
        n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------------
// Hidden optional content

fn ref_nums(o: Option<PdfObject>) -> Result<HashSet<i32>> {
    let mut s = HashSet::new();
    if let Some(o) = o {
        if o.is_array()? {
            for i in 0..o.len()? as i32 {
                if let Some(x) = o.get_array(i)? {
                    s.insert(num(&x)?);
                }
            }
        } else {
            s.insert(num(&o)?);
        }
    }
    Ok(s)
}

/// Object numbers of the OCGs that are off in the default configuration (None = no layers).
fn off_ocgs(doc: &PdfDocument) -> Result<Option<HashSet<i32>>> {
    let Some(ocp) = doc.catalog()?.get_dict("OCProperties")? else { return Ok(None) };
    let all = ref_nums(ocp.get_dict("OCGs")?)?;
    let Some(d) = ocp.get_dict("D")? else { return Ok(Some(HashSet::new())) };
    let base_off = d.get_dict("BaseState")?.map(|b| b.as_name()).transpose()?.as_deref() == Some(b"OFF");
    Ok(Some(if base_off {
        let on = ref_nums(d.get_dict("ON")?)?;
        all.difference(&on).copied().collect()
    } else {
        ref_nums(d.get_dict("OFF")?)?
    }))
}

/// Whether an /OC value (OCG or OCMD reference) is hidden.
fn oc_hidden(oc: &PdfObject, off: &HashSet<i32>) -> Result<bool> {
    if oc.get_dict("Type")?.map(|t| t.as_name()).transpose()?.as_deref() != Some(b"OCMD") {
        return Ok(off.contains(&num(oc)?));
    }
    let states: Vec<bool> = ref_nums(oc.get_dict("OCGs")?)?.iter().map(|n| !off.contains(n)).collect();
    if states.is_empty() {
        return Ok(false);
    }
    let policy = oc.get_dict("P")?.map(|p| p.as_name()).transpose()?.unwrap_or_else(|| b"AnyOn".to_vec());
    let visible = match policy.as_slice() {
        b"AllOn" => states.iter().all(|&s| s),
        b"AnyOff" => states.iter().any(|&s| !s),
        b"AllOff" => states.iter().all(|&s| !s),
        _ => states.iter().any(|&s| s),
    };
    Ok(!visible)
}

/// Names in `res`'s `/Properties` and `/XObject` that are hidden, plus the visible form XObjects.
type Names = HashSet<Vec<u8>>;

fn hidden_names(res: &Option<PdfObject>, off: &HashSet<i32>) -> Result<(Names, Names, Vec<PdfObject>)> {
    let (mut props, mut xobjs, mut forms) = (HashSet::new(), HashSet::new(), Vec::new());
    let Some(res) = res else { return Ok((props, xobjs, forms)) };
    if let Some(p) = res.get_dict("Properties")? {
        for i in 0..p.dict_len()? as i32 {
            if let (Some(k), Some(v)) = (p.get_dict_key(i)?, p.get_dict_val(i)?)
                && oc_hidden(&v, off)?
            {
                props.insert(k.as_name()?);
            }
        }
    }
    if let Some(x) = res.get_dict("XObject")? {
        for i in 0..x.dict_len()? as i32 {
            let (Some(k), Some(v)) = (x.get_dict_key(i)?, x.get_dict_val(i)?) else { continue };
            if let Some(oc) = v.get_dict("OC")?
                && oc_hidden(&oc, off)?
            {
                xobjs.insert(k.as_name()?);
            } else if v.get_dict("Subtype")?.map(|s| s.as_name()).transpose()?.as_deref() == Some(b"Form") {
                forms.push(v);
            }
        }
    }
    Ok((props, xobjs, forms))
}

/// Deletes content in hidden layers from every page and form XObject, hidden annotations, then
/// /OCProperties. Returns how many blocks went.
fn strip_hidden_layers(doc: &mut PdfDocument) -> Result<u32> {
    let Some(off) = off_ocgs(doc)? else { return Ok(0) };
    let mut count = 0;
    if !off.is_empty() {
        let mut seen = HashSet::new();
        for mut page in pages(doc)? {
            let res = page.get_dict_inheritable("Resources")?;
            let (props, xobjs, forms) = hidden_names(&res, &off)?;
            if let Some(c) = page.get_dict("Contents")? {
                let streams: Vec<PdfObject> = if c.is_array()? {
                    (0..c.len()? as i32).filter_map(|i| c.get_array(i).transpose()).collect::<std::result::Result<_, _>>()?
                } else {
                    vec![c]
                };
                let mut data = Vec::new();
                for s in &streams {
                    data.extend(s.read_stream()?);
                    data.push(b'\n');
                }
                let (out, n) = strip_hidden(&data, &props, &xobjs);
                if n > 0 {
                    let mut first = streams[0].clone();
                    first.write_stream_buffer(&Buffer::from_bytes(&out)?)?;
                    page.dict_put("Contents", first)?;
                    count += n;
                }
            }
            let mut stack: Vec<(PdfObject, Option<PdfObject>)> = forms.into_iter().map(|f| (f, res.clone())).collect();
            while let Some((mut form, parent_res)) = stack.pop() {
                if !seen.insert(num(&form)?) {
                    continue;
                }
                let res = form.get_dict("Resources")?.or(parent_res);
                let (props, xobjs, forms) = hidden_names(&res, &off)?;
                let (out, n) = strip_hidden(&form.read_stream()?, &props, &xobjs);
                if n > 0 {
                    form.write_stream_buffer(&Buffer::from_bytes(&out)?)?;
                    count += n;
                }
                stack.extend(forms.into_iter().map(|f| (f, res.clone())));
            }
            count += drop_annots(&mut page, |a, _| Ok(a.get_dict("OC")?.map(|oc| oc_hidden(&oc, &off)).transpose()? == Some(true)))?;
        }
    }
    doc.catalog()?.dict_delete("OCProperties")?;
    Ok(count)
}

#[derive(Debug, PartialEq)]
enum Tok {
    Name(Vec<u8>),
    Op(Vec<u8>),
    Other,
}

/// Minimal content-stream lexer: enough to find operators and their operands (strings, hex
/// strings, dicts, arrays, comments and inline image data are skipped correctly).
struct Lexer<'a> {
    s: &'a [u8],
    pos: usize,
}

fn is_ws(b: u8) -> bool {
    b"\0\t\n\x0c\r ".contains(&b)
}

fn is_delim(b: u8) -> bool {
    b"()<>[]{}/%".contains(&b)
}

impl Iterator for Lexer<'_> {
    type Item = (usize, usize, Tok);

    fn next(&mut self) -> Option<Self::Item> {
        let s = self.s;
        loop {
            while self.pos < s.len() && is_ws(s[self.pos]) {
                self.pos += 1;
            }
            if self.pos < s.len() && s[self.pos] == b'%' {
                while self.pos < s.len() && s[self.pos] != b'\n' && s[self.pos] != b'\r' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
        let start = self.pos;
        let b = *s.get(start)?;
        let next = s.get(start + 1).copied();
        let tok = match b {
            b'(' => {
                let mut depth = 0;
                while self.pos < s.len() {
                    match s[self.pos] {
                        b'\\' => self.pos += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                self.pos += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                Tok::Other
            }
            b'<' | b'>' if next == Some(b) => {
                self.pos += 2;
                Tok::Other
            }
            b'<' => {
                while self.pos < s.len() && s[self.pos] != b'>' {
                    self.pos += 1;
                }
                self.pos += 1;
                Tok::Other
            }
            b'/' => {
                self.pos += 1;
                while self.pos < s.len() && !is_ws(s[self.pos]) && !is_delim(s[self.pos]) {
                    self.pos += 1;
                }
                let raw = &s[start + 1..self.pos];
                let mut name = Vec::with_capacity(raw.len());
                let mut i = 0;
                while i < raw.len() {
                    if raw[i] == b'#'
                        && let Some(v) = raw.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())
                    {
                        name.push(v);
                        i += 3;
                    } else {
                        name.push(raw[i]);
                        i += 1;
                    }
                }
                Tok::Name(name)
            }
            _ if is_delim(b) => {
                self.pos += 1;
                Tok::Other
            }
            _ => {
                while self.pos < s.len() && !is_ws(s[self.pos]) && !is_delim(s[self.pos]) {
                    self.pos += 1;
                }
                let w = &s[start..self.pos];
                if w[0].is_ascii_digit() || b"+-.".contains(&w[0]) || [&b"true"[..], b"false", b"null"].contains(&w) {
                    Tok::Other
                } else {
                    if w == b"ID" {
                        // Inline image data runs to the first whitespace-delimited "EI".
                        let mut i = self.pos + 1;
                        while i + 2 <= s.len() {
                            if &s[i..i + 2] == b"EI" && is_ws(s[i - 1]) && s.get(i + 2).is_none_or(|&c| is_ws(c)) {
                                break;
                            }
                            i += 1;
                        }
                        self.pos = (i + 2).min(s.len());
                    }
                    Tok::Op(w.to_vec())
                }
            }
        };
        Some((start, self.pos.min(s.len()), tok))
    }
}

/// Every name a content stream uses (operands only, so `/Name` keys of inline dicts count too).
pub(super) fn names_in(src: &[u8]) -> HashSet<Vec<u8>> {
    Lexer { s: src, pos: 0 }.filter_map(|(_, _, t)| if let Tok::Name(n) = t { Some(n) } else { None }).collect()
}

/// Cuts `/OC /name BDC ... EMC` blocks whose layer is hidden and `/name Do` of hidden XObjects.
fn strip_hidden(src: &[u8], props: &HashSet<Vec<u8>>, xobjs: &HashSet<Vec<u8>>) -> (Vec<u8>, u32) {
    let mut out = Vec::with_capacity(src.len());
    let (mut copied, mut count) = (0, 0);
    let mut operands: Vec<(usize, Tok)> = Vec::new();
    let mut dropping: Option<(usize, u32)> = None;
    for (start, end, tok) in (Lexer { s: src, pos: 0 }) {
        let Tok::Op(op) = tok else {
            operands.push((start, tok));
            continue;
        };
        let first = operands.first().map_or(start, |o| o.0);
        match (dropping, op.as_slice()) {
            (None, b"BDC")
                if matches!(operands.as_slice(), [(_, Tok::Name(oc)), (_, Tok::Name(n))] if oc == b"OC" && props.contains(n)) =>
            {
                dropping = Some((first, 1));
            }
            (Some((s, d)), b"BDC" | b"BMC") => dropping = Some((s, d + 1)),
            (Some((s, 1)), b"EMC") => {
                out.extend_from_slice(&src[copied..s]);
                copied = end;
                count += 1;
                dropping = None;
            }
            (Some((s, d)), b"EMC") => dropping = Some((s, d - 1)),
            (None, b"Do") if matches!(operands.last(), Some((_, Tok::Name(n))) if xobjs.contains(n)) => {
                out.extend_from_slice(&src[copied..first]);
                copied = end;
                count += 1;
            }
            _ => {}
        }
        operands.clear();
    }
    if let Some((s, _)) = dropping {
        out.extend_from_slice(&src[copied..s]);
        copied = src.len();
        count += 1;
    }
    out.extend_from_slice(&src[copied..]);
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_hidden_blocks() {
        let props: HashSet<Vec<u8>> = [b"OC1".to_vec()].into();
        let xobjs: HashSet<Vec<u8>> = [b"Im1".to_vec()].into();
        let src = b"q (a\\) EMC (b)) Tj /OC /OC1 BDC BT (SECRET) Tj /Span <</ActualText (x)>> BDC (y) Tj EMC ET EMC \
BI /W 1 /H 1 ID \x00EMC\xff EI /Im1 Do /OC /OC2 BDC (kept) Tj EMC % EMC comment\n/#4Fther Do Q";
        let (out, n) = strip_hidden(src, &props, &xobjs);
        assert_eq!(n, 2);
        assert_eq!(
            out,
            b"q (a\\) EMC (b)) Tj  \
BI /W 1 /H 1 ID \x00EMC\xff EI  /OC /OC2 BDC (kept) Tj EMC % EMC comment\n/#4Fther Do Q"
        );
        // An unterminated hidden block is cut to the end.
        assert_eq!(strip_hidden(b"a /OC /OC1 BDC (x) Tj", &props, &xobjs), (b"a ".to_vec(), 1));
    }
}
