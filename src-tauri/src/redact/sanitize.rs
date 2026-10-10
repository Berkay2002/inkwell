//! Sanitize: removes the leak channels next to the page content (Acrobat's "Sanitize and remove
//! hidden information", plus what Acrobat misses: catalog/page /AF associated files, orphan
//! popups). Independent of the per-category switches, anything that quotes a redacted string
//! (annotation, form field, docinfo entry, XMP packet, bookmark) is always removed.

use super::{RedactError, annots_of, needle_regex, prune_resources, scan_strings, subtype, text_of};
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
    /// Other text quoting a redacted string: tagged-PDF alt text, page labels, URIs, named
    /// destinations, rich text and the like.
    pub quoting: u32,
}

pub(super) fn del(o: &mut PdfObject, key: &str) -> Result<u32> {
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
        // Oracle against lexer differentials: with /OCProperties gone every layer shows, so any
        // glyph MuPDF shows now and did not show before (same char at the same place) is hidden
        // content the stripping missed. Positions, not just counts: visible text cut by the fail-
        // closed rules must not cancel out revealed text with the same letters.
        let before = page_chars(doc)?;
        r.hidden_layer_blocks = strip_hidden_layers(doc)?;
        for (p, (b, a)) in before.iter().zip(page_chars(doc)?).enumerate() {
            if revealed(b, &a) {
                let place = format!("page {}: hidden layer content left after sanitize", p + 1);
                return Err(RedactError::Leaked(vec![super::Leak { needle: String::new(), place, critical: true }]));
            }
        }
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
                scan_strings(a, 0, &mut |s, name| q |= !name && quotes(&res, s))?;
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
    // After annotation removal: invisible text in an annotation the switches removed is gone.
    if o.hidden_text {
        r.hidden_text_pages = strip_invisible_text(doc)?;
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
                    && quotes(&res, &text_of(&v)?)
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
        // Old-style named destinations are name keys of the catalog /Dests dict.
        if let Some(mut dests) = cat.get_dict("Dests")? {
            for i in (0..dests.dict_len()? as i32).rev() {
                if let Some(k) = dests.get_dict_key(i)?
                    && quotes(&res, &String::from_utf8_lossy(&k.as_name()?))
                {
                    dests.dict_delete(k)?;
                    r.quoting += 1;
                }
            }
        }
    }
    let quoting = r.quoting;
    scrub_reachable(doc, o, &res, &mut r)?;
    if r.quoting > quoting {
        fix_name_trees(doc)?;
    }
    Ok(r)
}

/// Whether the page shows a glyph after stripping (same char at the same place) that it did not
/// show before.
pub(super) fn revealed(before: &Glyphs, after: &Glyphs) -> bool {
    after.iter().any(|(c, n)| before.get(c).is_none_or(|m| m < n))
}

/// After name-tree entries were removed: drops leaves left empty and recomputes /Limits, which
/// would otherwise still quote a removed key (and mislead lookups).
fn fix_name_trees(doc: &PdfDocument) -> Result<()> {
    let cat = doc.catalog()?;
    let mut roots = Vec::new();
    if let Some(names) = cat.get_dict("Names")? {
        for i in 0..names.dict_len()? as i32 {
            roots.extend(names.get_dict_val(i)?);
        }
    }
    if let Some(st) = cat.get_dict("StructTreeRoot")? {
        roots.extend(st.get_dict("IDTree")?);
    }
    for mut root in roots {
        fix_tree(&mut root, 0)?;
    }
    Ok(())
}

/// Fixes one name-tree node; returns its first and last key, None when it holds none.
fn fix_tree(node: &mut PdfObject, depth: u32) -> Result<Option<(PdfObject, PdfObject)>> {
    if depth > 32 {
        let place = "a name tree deeper than 32 levels (not sanitized)".to_string();
        return Err(RedactError::Leaked(vec![super::Leak { needle: String::new(), place, critical: true }]));
    }
    let mut bounds: Option<(PdfObject, PdfObject)> = None;
    if let Some(names) = node.get_dict("Names")?
        && names.is_array()?
        && names.len()? >= 2
    {
        let n = names.len()? as i32;
        bounds = names.get_array(0)?.zip(names.get_array((n - 2) & !1)?);
    }
    if let Some(mut kids) = node.get_dict("Kids")?
        && kids.is_array()?
    {
        for i in (0..kids.len()? as i32).rev() {
            let Some(mut kid) = kids.get_array(i)? else { continue };
            match fix_tree(&mut kid, depth + 1)? {
                None => kids.array_delete(i)?,
                Some((first, last)) => {
                    bounds = Some(match bounds {
                        None => (first, last),
                        // Kids run in key order, last kid first here.
                        Some((_, l)) => (first, l),
                    });
                }
            }
        }
    }
    if let Some(mut limits) = node.get_dict("Limits")? {
        match &bounds {
            Some((first, last)) if limits.is_array()? && limits.len()? == 2 => {
                limits.array_put(0, first.clone())?;
                limits.array_put(1, last.clone())?;
            }
            _ => node.dict_delete("Limits")?,
        }
    }
    Ok(bounds)
}

/// Text-bearing keys removed wherever their value quotes a redacted string: tagged-PDF alternate
/// text, titles and expansions, page label prefixes, URIs, rich text, field values and tooltips.
const QUOTE_KEYS: [&str; 17] =
    ["ActualText", "Alt", "E", "T", "TU", "TM", "URI", "RC", "RV", "DS", "P", "Contents", "V", "DV", "Title", "Subj", "NM"];

/// Walks every object reachable from the trailer: removes what the switches remove wherever it
/// sits (XMP and piece info on any object, additional actions and JavaScript on any dict, embedded
/// file streams of any file specification) and text quoting a redacted string (`QUOTE_KEYS`,
/// name-tree entries whose key quotes it).
fn scrub_reachable(doc: &PdfDocument, o: &Sanitize, res: &[Regex], r: &mut SanitizeReport) -> Result<()> {
    let mut refs = Vec::new();
    scrub(&mut doc.trailer()?, o, res, r, 0, &mut refs)?;
    let mut seen = HashSet::new();
    while let Some(n) = refs.pop() {
        if seen.insert(n) {
            scrub(&mut doc.new_indirect(n, 0)?, o, res, r, 0, &mut refs)?;
        }
    }
    Ok(())
}

/// Whether a string (or, with `streams`, a text stream such as rich text) quotes a redacted string.
fn quoting_value(v: &PdfObject, res: &[Regex], streams: bool) -> Result<bool> {
    Ok(if v.is_string()? {
        quotes(res, &text_of(v)?)
    } else if streams && v.is_stream()? {
        quotes(res, &String::from_utf8_lossy(&v.read_stream()?))
    } else {
        false
    })
}

fn scrub(o: &mut PdfObject, opts: &Sanitize, res: &[Regex], r: &mut SanitizeReport, depth: u32, refs: &mut Vec<i32>) -> Result<()> {
    if depth > 32 {
        let place = "an object nested deeper than 32 levels (not sanitized)".to_string();
        return Err(RedactError::Leaked(vec![super::Leak { needle: String::new(), place, critical: true }]));
    }
    let children = if o.is_dict()? {
        if opts.metadata {
            r.metadata += del(o, "Metadata")? + del(o, "PieceInfo")?;
        }
        if opts.scripts_actions {
            r.scripts_actions += del(o, "AA")? + del(o, "JS")?;
        }
        if opts.embedded_files {
            r.embedded_files += del(o, "EF")? + del(o, "RichMediaContent")?;
        }
        if !res.is_empty() {
            for key in QUOTE_KEYS {
                // Only rich text may be a stream: /Contents of a page is its content stream.
                if let Some(v) = o.get_dict(key)?
                    && quoting_value(&v, res, matches!(key, "RC" | "RV"))?
                {
                    r.quoting += 1;
                    // A field's /T and an outline's /Title are required: a placeholder instead.
                    if matches!(key, "T" | "Title") {
                        o.dict_put(key, PdfObject::new_string(&format!("redacted-{}", r.quoting))?)?;
                    } else {
                        o.dict_delete(key)?;
                    }
                }
            }
            if let Some(mut names) = o.get_dict("Names")?
                && names.is_array()?
            {
                for i in (0..names.len()? as i32 / 2).rev() {
                    if let Some(k) = names.get_array(2 * i)?
                        && quoting_value(&k, res, false)?
                    {
                        names.array_delete(2 * i + 1)?;
                        names.array_delete(2 * i)?;
                        r.quoting += 1;
                    }
                }
            }
        }
        (0..o.dict_len()? as i32).map(|i| o.get_dict_val(i)).collect::<std::result::Result<Vec<_>, _>>()?
    } else if o.is_array()? {
        (0..o.len()? as i32).map(|i| o.get_array(i)).collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        return Ok(());
    };
    for mut c in children.into_iter().flatten() {
        if c.is_indirect()? {
            refs.push(c.as_indirect()?);
        } else {
            scrub(&mut c, opts, res, r, depth + 1, refs)?;
        }
    }
    Ok(())
}

/// Per page: how often each char appears at each (rounded) position in the text MuPDF shows.
pub(super) type Glyphs = std::collections::HashMap<(char, i32, i32), u32>;

fn page_chars(doc: &PdfDocument) -> Result<Vec<Glyphs>> {
    let mut v = Vec::new();
    for p in 0..doc.page_count()? {
        let mut m = Glyphs::new();
        for c in super::index_page(&doc.load_page(p)?)?.chars.iter().filter(|c| !c.hidden && !c.c.is_whitespace()) {
            *m.entry((c.c, c.quad.ul.x.round() as i32, c.quad.ul.y.round() as i32)).or_insert(0) += 1;
        }
        v.push(m);
    }
    Ok(v)
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
    let mut seen = HashSet::new();
    loop {
        let n = num(&node)?;
        // A /Parent cycle would otherwise loop forever.
        if !seen.insert(n) {
            return Ok(());
        }
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

fn has_invisible_text(doc: &PdfDocument, p: i32) -> Result<bool> {
    Ok(doc
        .load_page(p)?
        .to_text_page(TextPageFlags::empty())?
        .blocks()
        .flat_map(|b| b.lines().flat_map(|l| l.chars().map(|c| c.flags()).collect::<Vec<_>>()).collect::<Vec<_>>())
        .any(|f| !f.intersects(TextCharFlags::FILLED | TextCharFlags::STROKED)))
}

/// Removes invisible text page by page with a page-sized redaction that only targets invisible
/// text. The page's annotations are set aside meanwhile: MuPDF drops every link and free-text
/// annotation a redaction touches, and this one touches all of them.
fn strip_invisible_text(doc: &mut PdfDocument) -> Result<u32> {
    let mut n = 0;
    for p in 0..doc.page_count()? {
        if !has_invisible_text(doc, p)? {
            continue;
        }
        let mut obj = doc.find_page(p)?;
        let annots = obj.get_dict("Annots")?;
        if annots.is_some() {
            obj.dict_delete("Annots")?;
        }
        {
            let mut page = doc.load_pdf_page(p)?;
            let bounds = page.bounds()?;
            let annot = page.add_redact_annotation(bounds)?;
            annot.apply_redaction_with_options(PdfRedactOptions {
                black_boxes: false,
                image_method: PdfRedactImageMethod::None,
                line_art: PdfRedactLineArtMethod::None,
                text: PdfRedactTextMethod::RemoveInvisible,
            })?;
        }
        match annots {
            Some(a) => obj.dict_put("Annots", a)?,
            None => obj.dict_delete("Annots")?,
        }
        // Invisible text MuPDF did not cut (e.g. in an annotation appearance) must not pass as removed.
        if has_invisible_text(doc, p)? {
            let place = format!("page {}: invisible text left after sanitize", p + 1);
            return Err(RedactError::Leaked(vec![super::Leak { needle: String::new(), place, critical: true }]));
        }
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
/// Fails closed: an OCG that some /AS auto-state or a /Usage /View /ViewState OFF could turn off
/// counts as off too.
fn off_ocgs(doc: &PdfDocument) -> Result<Option<HashSet<i32>>> {
    let Some(ocp) = doc.catalog()?.get_dict("OCProperties")? else { return Ok(None) };
    let all = ref_nums(ocp.get_dict("OCGs")?)?;
    let mut off = HashSet::new();
    if let Some(d) = ocp.get_dict("D")? {
        let base_off = d.get_dict("BaseState")?.map(|b| b.as_name()).transpose()?.as_deref() == Some(b"OFF");
        off = if base_off {
            let on = ref_nums(d.get_dict("ON")?)?;
            all.difference(&on).copied().collect()
        } else {
            ref_nums(d.get_dict("OFF")?)?
        };
        if let Some(r#as) = d.get_dict("AS")? {
            for i in 0..r#as.len()? as i32 {
                if let Some(a) = r#as.get_array(i)? {
                    off.extend(ref_nums(a.get_dict("OCGs")?)?);
                }
            }
        }
    }
    for &n in &all {
        let usage = doc.new_indirect(n, 0)?.get_dict("Usage")?;
        let view = usage.map(|u| u.get_dict("View")).transpose()?.flatten();
        if let Some(v) = view.map(|v| v.get_dict("ViewState")).transpose()?.flatten()
            && v.as_name()? == b"OFF"
        {
            off.insert(n);
        }
    }
    Ok(Some(off))
}

/// Whether an /OC value (OCG or OCMD reference) is hidden. Anything it can't evaluate counts as
/// hidden: sanitize must not keep layer content it isn't sure about.
fn oc_hidden(oc: &PdfObject, off: &HashSet<i32>) -> Result<bool> {
    match oc.get_dict("Type")?.map(|t| t.as_name()).transpose()?.as_deref() {
        Some(b"OCG") => return Ok(off.contains(&num(oc)?)),
        Some(b"OCMD") => {}
        _ => return Ok(true),
    }
    if let Some(ve) = oc.get_dict("VE")? {
        return Ok(!ve_visible(&ve, off, 0, &mut 10_000)?);
    }
    let states: Vec<bool> = ref_nums(oc.get_dict("OCGs")?)?.iter().map(|n| !off.contains(n)).collect();
    if states.is_empty() {
        return Ok(false);
    }
    let policy = oc.get_dict("P")?.map(|p| p.as_name()).transpose()?.unwrap_or_else(|| b"AnyOn".to_vec());
    let visible = match policy.as_slice() {
        b"AnyOn" => states.iter().any(|&s| s),
        b"AllOn" => states.iter().all(|&s| s),
        b"AnyOff" => states.iter().any(|&s| !s),
        b"AllOff" => states.iter().all(|&s| !s),
        _ => false,
    };
    Ok(!visible)
}

/// Evaluates an OCMD /VE visibility expression ([/And|/Or|/Not operand...], operands OCGs or
/// nested expressions). Malformed, too deep or too big = not visible. `budget` bounds the work:
/// shared indirect sub-expressions would otherwise be evaluated fanout^depth times.
fn ve_visible(ve: &PdfObject, off: &HashSet<i32>, depth: u32, budget: &mut u32) -> Result<bool> {
    if depth > 16 || *budget == 0 {
        *budget = 0;
        return Ok(false);
    }
    *budget -= 1;
    if !ve.is_array()? {
        return Ok(ve.is_indirect()? && !off.contains(&num(ve)?));
    }
    let Some(op) = ve.get_array(0)? else { return Ok(false) };
    let op = if op.is_name()? { op.as_name()? } else { return Ok(false) };
    let mut args = Vec::new();
    for i in 1..ve.len()? as i32 {
        if let Some(x) = ve.get_array(i)? {
            args.push(ve_visible(&x, off, depth + 1, budget)?);
        }
    }
    Ok(match (op.as_slice(), args.as_slice()) {
        (_, []) => false,
        (b"And", a) => a.iter().all(|&s| s),
        (b"Or", a) => a.iter().any(|&s| s),
        (b"Not", [s]) => !s,
        _ => false,
    })
}

type Names = HashSet<Vec<u8>>;
type Nested = Vec<(PdfObject, Option<PdfObject>)>;

/// For content drawn with `res`: the `/Properties` names that are visible, the `/XObject` names
/// that are hidden, and the nested content streams to visit next (form XObjects, tiling patterns,
/// Type3 glyph procedures) with the resources they fall back to.
fn layer_names(res: &Option<PdfObject>, off: &HashSet<i32>) -> Result<(Names, Names, Nested)> {
    let (mut shown, mut hidden, mut nested) = (HashSet::new(), HashSet::new(), Vec::new());
    let Some(res) = res else { return Ok((shown, hidden, nested)) };
    let name_of = |o: &PdfObject, k: &str| -> Result<Option<Vec<u8>>> { o.get_dict(k)?.map(|s| s.as_name()).transpose().map_err(Into::into) };
    let entries = |cat: &str| -> Result<Vec<(Vec<u8>, PdfObject)>> {
        let mut v = Vec::new();
        if let Some(d) = res.get_dict(cat)? {
            for i in 0..d.dict_len()? as i32 {
                if let (Some(k), Some(x)) = (d.get_dict_key(i)?, d.get_dict_val(i)?) {
                    v.push((k.as_name()?, x));
                }
            }
        }
        Ok(v)
    };
    for (k, v) in entries("Properties")? {
        if !oc_hidden(&v, off)? {
            shown.insert(k);
        }
    }
    for (k, v) in entries("XObject")? {
        if let Some(oc) = v.get_dict("OC")?
            && oc_hidden(&oc, off)?
        {
            hidden.insert(k);
        } else if name_of(&v, "Subtype")?.as_deref() == Some(b"Form") {
            nested.push((v, Some(res.clone())));
        }
    }
    // Soft-mask groups draw too: hidden content there is shown once /OCProperties is gone.
    for (_, gs) in entries("ExtGState")? {
        if let Some(sm) = gs.get_dict("SMask")?
            && sm.is_dict()?
            && let Some(g) = sm.get_dict("G")?
        {
            nested.push((g, Some(res.clone())));
        }
    }
    for (_, v) in entries("Pattern")? {
        if v.get_dict("PatternType")?.map(|t| t.as_int()).transpose()? == Some(1) {
            nested.push((v, Some(res.clone())));
        }
    }
    for (_, f) in entries("Font")? {
        if name_of(&f, "Subtype")?.as_deref() == Some(b"Type3")
            && let Some(procs) = f.get_dict("CharProcs")?
        {
            let fres = f.get_dict("Resources")?.or(Some(res.clone()));
            for i in 0..procs.dict_len()? as i32 {
                if let Some(p) = procs.get_dict_val(i)? {
                    nested.push((p, fres.clone()));
                }
            }
        }
    }
    Ok((shown, hidden, nested))
}

/// Deletes the hidden XObjects from `res`: they are hidden wherever they are drawn, and unused
/// they would still carry their content into the file.
fn drop_hidden_xobjects(res: &Option<PdfObject>, hidden: &Names) -> Result<()> {
    if hidden.is_empty() {
        return Ok(());
    }
    if let Some(mut x) = res.as_ref().map(|r| r.get_dict("XObject")).transpose()?.flatten() {
        for i in (0..x.dict_len()? as i32).rev() {
            if let Some(k) = x.get_dict_key(i)?
                && hidden.contains(&k.as_name()?)
            {
                x.dict_delete(k)?;
            }
        }
    }
    Ok(())
}

/// Deletes content in hidden layers from every page, form XObject, tiling pattern, Type3 glyph
/// and annotation appearance stream, hidden annotations, then /OCProperties. Returns how many
/// blocks went.
fn strip_hidden_layers(doc: &mut PdfDocument) -> Result<u32> {
    let Some(off) = off_ocgs(doc)? else { return Ok(0) };
    let mut count = 0;
    let mut seen = HashSet::new();
    for (p, mut page) in pages(doc)?.into_iter().enumerate() {
        let res = page.get_dict_inheritable("Resources")?;
        let (shown, hidden, mut stack) = layer_names(&res, &off)?;
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
            let (out, n) = strip_hidden(&data, &shown, &hidden);
            if n > 0 {
                let mut first = streams[0].clone();
                first.write_stream_buffer(&Buffer::from_bytes(&out)?)?;
                page.dict_put("Contents", first)?;
                prune_resources(doc, p as i32)?;
                count += n;
            }
        }
        // Hidden XObjects go even when nothing draws them: they would still carry their content.
        drop_hidden_xobjects(&res, &hidden)?;
        let mut widgets = Vec::new();
        count += drop_annots(&mut page, |a, st| {
            let gone = a.get_dict("OC")?.map(|oc| oc_hidden(&oc, &off)).transpose()? == Some(true);
            if gone && st == b"Widget" {
                widgets.push(a.clone());
            }
            Ok(gone)
        })?;
        // A hidden widget's field (and its value) must not stay in the form.
        for w in widgets {
            remove_field(doc, &w)?;
        }
        for a in annots_of(&page)? {
            let Some(ap) = a.get_dict("AP")? else { continue };
            for key in ["N", "R", "D"] {
                let Some(v) = ap.get_dict(key)? else { continue };
                if v.is_stream()? {
                    stack.push((v, None));
                } else if v.is_dict()? {
                    for i in 0..v.dict_len()? as i32 {
                        if let Some(s) = v.get_dict_val(i)? {
                            stack.push((s, None));
                        }
                    }
                }
            }
        }
        while let Some((mut form, parent_res)) = stack.pop() {
            if !form.is_stream()? || !seen.insert(num(&form)?) {
                continue;
            }
            let res = form.get_dict("Resources")?.or(parent_res);
            let (shown, hidden, nested) = layer_names(&res, &off)?;
            let (out, n) = strip_hidden(&form.read_stream()?, &shown, &hidden);
            if n > 0 {
                form.write_stream_buffer(&Buffer::from_bytes(&out)?)?;
                count += n;
            }
            drop_hidden_xobjects(&res, &hidden)?;
            stack.extend(nested);
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

/// Cuts `/OC ... BDC ... EMC` blocks unless the operand is a name in `shown` (so inline OC dicts
/// and unknown names go too) and `/name Do` of hidden XObjects.
fn strip_hidden(src: &[u8], shown: &HashSet<Vec<u8>>, xobjs: &HashSet<Vec<u8>>) -> (Vec<u8>, u32) {
    let mut out = Vec::with_capacity(src.len());
    let (mut copied, mut count) = (0, 0);
    let mut operands: Vec<(usize, Tok)> = Vec::new();
    let mut dropping: Option<(usize, u32)> = None;
    let mut nest = 0;
    for (start, end, tok) in (Lexer { s: src, pos: 0 }) {
        match &src[start..end] {
            b"<<" | b"[" => nest += 1,
            b">>" | b"]" if nest > 0 => nest -= 1,
            _ => {}
        }
        let Tok::Op(op) = tok else {
            operands.push((start, tok));
            continue;
        };
        // `R` inside an operand dict is a reference, not an operator. Any other operator ends a
        // dangling `[` or `<<`: viewers recover differently there, so the block is judged anyway.
        if nest > 0 && op == b"R" {
            operands.push((start, Tok::Other));
            continue;
        }
        nest = 0;
        let first = operands.first().map_or(start, |o| o.0);
        match (dropping, op.as_slice()) {
            (None, b"BDC")
                if operands.iter().any(|o| matches!(&o.1, Tok::Name(oc) if oc == b"OC"))
                    && !matches!(operands.as_slice(), [(_, Tok::Name(oc)), (_, Tok::Name(n))] if oc == b"OC" && shown.contains(n)) =>
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
        let props: HashSet<Vec<u8>> = [b"OC2".to_vec()].into();
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
        // Inline OC dicts and names missing from /Properties fail closed.
        assert_eq!(
            strip_hidden(b"a /OC <</Type/OCMD/OCGs 5 0 R>> BDC (x) Tj EMC /OC /Nope BDC (y) Tj EMC b", &props, &xobjs),
            (b"a   b".to_vec(), 2)
        );
        // A dangling `[` (or `<<`) before a hidden block or a hidden Do must not hide it from the cut.
        assert_eq!(strip_hidden(b"a [ /OC /OC1 BDC (x) Tj EMC b", &props, &xobjs), (b"a  b".to_vec(), 1));
        assert_eq!(strip_hidden(b"a << /OC /OC1 BDC (x) Tj EMC b", &props, &xobjs), (b"a  b".to_vec(), 1));
        assert_eq!(strip_hidden(b"a [ /Im1 Do b", &props, &xobjs), (b"a  b".to_vec(), 1));
        // TJ arrays and references in property dicts still work.
        assert_eq!(strip_hidden(b"[(a) -20 (b)] TJ /OC /OC2 BDC EMC", &props, &xobjs).1, 0);
    }
}
