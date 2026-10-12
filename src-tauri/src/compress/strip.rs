//! Optional extras that can be dropped on request. All off by default.

use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};

use super::Error;
use crate::outline::Discarded;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Strip {
    /// Document information dictionary and XMP packet.
    pub metadata: bool,
    /// Embedded page thumbnails.
    pub thumbnails: bool,
    /// File attachments (names tree, associated files, attachment annotations).
    pub embedded_files: bool,
    /// Document, page and annotation JavaScript.
    pub javascript: bool,
    /// The bookmark tree.
    pub bookmarks: bool,
    /// Bookmarks whose destination does not exist (page gone, name not defined); the working ones under
    /// them move up into their place.
    pub invalid_bookmarks: bool,
    /// Submit-form, reset-form and import-data actions (fields, widgets, links).
    pub form_actions: bool,
    /// `/Alternates` of image XObjects (print or web variants of an image).
    pub alternate_images: bool,
    /// `/PieceInfo`: what other applications left in the catalog, the pages and the forms.
    pub private_data: bool,
    /// Every link annotation.
    pub links: bool,
    /// Link annotations that do nothing or lead nowhere (missing page, unknown named destination).
    pub invalid_links: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StripReport {
    pub metadata: bool,
    pub thumbnails: u32,
    pub embedded_files: u32,
    pub javascript: u32,
    pub bookmarks: bool,
    pub invalid_bookmarks: u32,
    pub form_actions: u32,
    pub alternate_images: u32,
    pub private_data: u32,
    pub links: u32,
    pub invalid_links: u32,
    /// Invalid links were asked for but the named destinations could not be read, so none were judged.
    pub links_unchecked: bool,
    /// Invalid bookmarks were asked for but the outline was left as it is, for this reason.
    pub bookmarks_unchecked: Option<String>,
}

impl StripReport {
    /// Something the user asked to drop was actually there and is gone.
    pub fn any(&self) -> bool {
        self.metadata
            || self.bookmarks
            || self.thumbnails
                + self.invalid_bookmarks
                + self.embedded_files
                + self.javascript
                + self.form_actions
                + self.alternate_images
                + self.private_data
                + self.links
                + self.invalid_links
                > 0
    }
}

fn is_name(o: &PdfObject, key: &str, want: &[u8]) -> bool {
    o.get_dict(key)
        .ok()
        .flatten()
        .is_some_and(|v| v.as_name().is_ok_and(|n| n == want))
}

fn is_js_action(a: &PdfObject) -> bool {
    a.is_dict().unwrap_or(false) && is_name(a, "S", b"JavaScript")
}

/// Takes out of `o` the actions that satisfy `pred`: `/A` when it does, and the entries of `/AA` that do
/// (`/AA` itself once nothing is left in it). Actions of other kinds stay. Returns how many went.
fn drop_actions(o: &mut PdfObject, pred: fn(&PdfObject) -> bool) -> Result<u32, Error> {
    let mut n = 0;
    if o.get_dict("A")?.is_some_and(|a| pred(&a)) {
        n += remove(o, "A") as u32;
    }
    n += drop_aa(o, pred)?;
    Ok(n)
}

fn drop_aa(o: &mut PdfObject, pred: fn(&PdfObject) -> bool) -> Result<u32, Error> {
    let Some(mut aa) = o.get_dict("AA")?.filter(|a| a.is_dict().unwrap_or(false)) else { return Ok(0) };
    let mut keys = Vec::new();
    for i in 0..aa.dict_len()? as i32 {
        let (Some(k), Some(v)) = (aa.get_dict_key(i)?, aa.get_dict_val(i)?) else { continue };
        if pred(&v) {
            if let Ok(k) = k.as_name() {
                keys.push(String::from_utf8_lossy(&k).into_owned());
            }
        }
    }
    for k in &keys {
        aa.dict_delete(k.as_str())?;
    }
    if !keys.is_empty() && aa.dict_len()? == 0 {
        o.dict_delete("AA")?;
    }
    Ok(keys.len() as u32)
}

/// Drops `key` from `o` when present; true if it was.
fn remove(o: &mut PdfObject, key: &str) -> bool {
    let had = o.get_dict(key).ok().flatten().is_some();
    had && o.dict_delete(key).is_ok()
}

pub(crate) fn strip(doc: &mut PdfDocument, s: &Strip, pages: usize) -> Result<StripReport, Error> {
    let mut r = StripReport::default();
    if *s == Strip::default() {
        return Ok(r);
    }
    let mut catalog = doc.catalog()?;

    if s.metadata {
        r.metadata = remove(&mut catalog, "Metadata");
        r.metadata |= remove(&mut doc.trailer()?, "Info");
        // Application-private data (Photoshop, InDesign ...) travels in PieceInfo; pages carry their own XMP.
        r.metadata |= remove(&mut catalog, "PieceInfo");
    }
    if s.bookmarks {
        r.bookmarks = remove(&mut catalog, "Outlines");
    }
    if let Some(mut names) = catalog.get_dict("Names")?.filter(|n| n.is_dict().unwrap_or(false)) {
        if s.embedded_files {
            r.embedded_files += remove(&mut names, "EmbeddedFiles") as u32;
        }
        if s.javascript {
            r.javascript += remove(&mut names, "JavaScript") as u32;
        }
    }
    if s.embedded_files {
        r.embedded_files += remove(&mut catalog, "AF") as u32;
    }
    if s.javascript {
        r.javascript += drop_aa(&mut catalog, is_js_action)?;
        if catalog.get_dict("OpenAction")?.is_some_and(|a| is_js_action(&a)) {
            r.javascript += remove(&mut catalog, "OpenAction") as u32;
        }
    }

    // A name tree that cannot be read is not an empty one: every named link would look broken.
    let judge_bookmarks = s.invalid_bookmarks && !s.bookmarks;
    let named = if s.invalid_links || judge_bookmarks { Dests::load(doc)? } else { None };
    r.links_unchecked = s.invalid_links && named.is_none();
    if judge_bookmarks {
        match &named {
            None => r.bookmarks_unchecked = Some("the named destinations could not be read".into()),
            Some(d) => match crate::outline::discard_where(doc, &|item| !bookmark_works(doc, d, item, pages))
                .map_err(|e| Error::Refused(e.to_string()))?
            {
                Discarded::Removed(n) => r.invalid_bookmarks = n,
                Discarded::Left(why) => r.bookmarks_unchecked = Some(why),
            },
        }
    }
    if s.metadata || s.thumbnails || s.javascript || s.embedded_files || s.links || s.invalid_links || s.private_data {
        for i in 0..pages {
            let mut page = doc.find_page(i as i32)?;
            if s.metadata {
                r.metadata |= remove(&mut page, "Metadata");
                r.metadata |= remove(&mut page, "PieceInfo");
            }
            if s.embedded_files {
                r.embedded_files += remove(&mut page, "AF") as u32;
            }
            if s.thumbnails {
                r.thumbnails += remove(&mut page, "Thumb") as u32;
            }
            if s.javascript {
                r.javascript += drop_aa(&mut page, is_js_action)?;
            }
            if s.private_data {
                r.private_data += remove(&mut page, "PieceInfo") as u32;
            }
            let Some(mut annots) = page.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) else {
                continue;
            };
            let mut drop_at = Vec::new();
            for (k, a) in annots.array_iter()?.enumerate() {
                let Ok(mut a) = a else { continue };
                if s.embedded_files && is_name(&a, "Subtype", b"FileAttachment") {
                    drop_at.push(k);
                    r.embedded_files += 1;
                    continue;
                }
                if is_name(&a, "Subtype", b"Link") {
                    if s.links {
                        drop_at.push(k);
                        r.links += 1;
                        continue;
                    }
                    if s.invalid_links && let Some(d) = &named && !link_works(doc, d, &a) {
                        drop_at.push(k);
                        r.invalid_links += 1;
                        continue;
                    }
                }
                if s.javascript {
                    r.javascript += drop_actions(&mut a, is_js_action)?;
                }
            }
            for k in drop_at.into_iter().rev() {
                annots.array_delete(k as i32)?;
            }
        }
    }
    if s.form_actions || s.alternate_images || s.private_data {
        sweep(doc, s, &mut r)?;
    }
    Ok(r)
}

/// What MuPDF's image pass is about to lose: image XObjects that carry `/Alternates` and form XObjects
/// that carry `/PieceInfo` (the pass writes new objects for the images and forms it rewrites). Counted
/// after the strip step, so what the user asked to drop is not reported again.
pub(crate) fn rewrite_losses(doc: &PdfDocument) -> Result<(u32, u32), Error> {
    let (mut alternates, mut private) = (0u32, 0u32);
    for x in 1..doc.xref_len()? {
        let Ok(o) = doc.new_indirect(x as i32, 0) else { continue };
        if !matches!(o.resolve(), Ok(Some(v)) if v.is_dict().unwrap_or(false)) || !o.is_stream().unwrap_or(false) {
            continue;
        }
        if is_name(&o, "Subtype", b"Image") && o.get_dict("Alternates").ok().flatten().is_some() {
            alternates += 1;
        }
        if is_name(&o, "Subtype", b"Form") && o.get_dict("PieceInfo").ok().flatten().is_some() {
            private += 1;
        }
    }
    Ok((alternates, private))
}

/// The three action types that talk to a form.
fn is_form_action(a: &PdfObject) -> bool {
    a.is_dict().unwrap_or(false)
        && [b"SubmitForm".as_slice(), b"ResetForm", b"ImportData"].iter().any(|n| is_name(a, "S", n))
}

/// Everything that is found by looking at every object rather than by walking from the catalog:
/// actions on fields and widgets (which hang off the field tree), image alternates, private data on
/// form XObjects.
fn sweep(doc: &PdfDocument, s: &Strip, r: &mut StripReport) -> Result<(), Error> {
    let n = doc.xref_len()?;
    for x in 1..n {
        let Ok(mut o) = doc.new_indirect(x as i32, 0) else { continue };
        if !matches!(o.resolve(), Ok(Some(v)) if v.is_dict().unwrap_or(false)) {
            continue;
        }
        if s.form_actions {
            r.form_actions += drop_actions(&mut o, is_form_action)?;
        }
        if s.alternate_images && o.is_stream().unwrap_or(false) && is_name(&o, "Subtype", b"Image") {
            r.alternate_images += remove(&mut o, "Alternates") as u32;
        }
        if s.private_data {
            r.private_data += remove(&mut o, "PieceInfo") as u32;
        }
    }
    Ok(())
}

/// Named destinations of the document: the catalog's `/Dests` dictionary (PDF 1.1) and the `/Names`
/// tree. Loaded once per run.
struct Dests {
    old: Option<PdfObject>,
    tree: Option<PdfObject>,
}

impl Dests {
    /// None when the `/Names` tree is there but cannot be read.
    fn load(doc: &PdfDocument) -> Result<Option<Self>, Error> {
        let catalog = doc.catalog()?;
        let old = catalog.get_dict("Dests")?.filter(|d| d.is_dict().unwrap_or(false));
        let has_tree = catalog.get_dict("Names")?.and_then(|n| n.get_dict("Dests").ok().flatten()).is_some();
        let tree = if has_tree {
            match doc.load_name_tree(PdfObject::new_name("Dests")?) {
                Ok(t) => Some(t),
                Err(_) => return Ok(None),
            }
        } else {
            None
        };
        Ok(Some(Self { old, tree }))
    }

    /// The destination a name or string stands for, if the document defines it.
    fn lookup(&self, key: &PdfObject) -> Option<PdfObject> {
        let name = if key.is_name().ok()? {
            String::from_utf8(key.as_name().ok()?).ok()?
        } else {
            key.as_string().ok()?
        };
        [&self.tree, &self.old].into_iter().flatten().find_map(|d| d.get_dict(name.as_str()).ok().flatten())
    }
}

/// An explicit destination array's page must be one of the document's pages. `pages`: when an integer
/// stands for the page (a remote destination's form, which some viewers follow here too), it has to be
/// below this; none leaves integers alone.
fn dest_array_ok(doc: &PdfDocument, arr: &PdfObject, pages: Option<i32>) -> bool {
    let Ok(Some(first)) = arr.get_array(0) else { return false };
    if first.is_int().unwrap_or(false) {
        return pages.is_none_or(|n| first.as_int().is_ok_and(|p| (0..n).contains(&p)));
    }
    let is_ref = first.is_indirect().unwrap_or(false);
    is_ref && matches!(doc.lookup_page_number(&first), Ok(n) if n >= 0)
}

/// A destination value: a name or string looked up, an array, or a dictionary holding `/D`.
fn dest_ok(doc: &PdfDocument, d: &Dests, v: &PdfObject, pages: Option<i32>) -> bool {
    if v.is_array().unwrap_or(false) {
        return dest_array_ok(doc, v, pages);
    }
    if v.is_name().unwrap_or(false) || v.is_string().unwrap_or(false) {
        let Some(target) = d.lookup(v) else { return false };
        return match target.get_dict("D").ok().flatten() {
            Some(inner) if target.is_dict().unwrap_or(false) => {
                inner.is_array().unwrap_or(false) && dest_array_ok(doc, &inner, pages)
            }
            _ => target.is_array().unwrap_or(false) && dest_array_ok(doc, &target, pages),
        };
    }
    false
}

/// A link works when its action or destination leads somewhere that exists. Actions that leave the
/// document (URI, remote go-to, launch, named, JavaScript ...) are not judged here.
fn link_works(doc: &PdfDocument, d: &Dests, link: &PdfObject) -> bool {
    if let Ok(Some(dest)) = link.get_dict("Dest") {
        return dest_ok(doc, d, &dest, None);
    }
    match link.get_dict("A") {
        Ok(Some(a)) if a.is_dict().unwrap_or(false) => {
            if is_name(&a, "S", b"GoTo") {
                a.get_dict("D").ok().flatten().is_some_and(|dest| dest_ok(doc, d, &dest, None))
            } else {
                // Any other action type is a decision of its own; a dictionary without /S does nothing.
                a.get_dict("S").ok().flatten().is_some()
            }
        }
        _ => false,
    }
}

/// A bookmark works unless its `/Dest`, or the `/D` of a GoTo action, does not resolve to a page of this document
/// (`pages` pages). An item with no destination is a heading and stays, and so does one with any other action
/// (URI, remote go-to, launch, named, JavaScript ...): those are not judged here.
fn bookmark_works(doc: &PdfDocument, d: &Dests, item: &PdfObject, pages: usize) -> bool {
    let pages = Some(pages as i32);
    if let Ok(Some(dest)) = item.get_dict("Dest") && !dest.is_null().unwrap_or(false) {
        return dest_ok(doc, d, &dest, pages);
    }
    match item.get_dict("A") {
        Ok(Some(a)) if a.is_dict().unwrap_or(false) && is_name(&a, "S", b"GoTo") => {
            a.get_dict("D").ok().flatten().is_some_and(|dest| dest_ok(doc, d, &dest, pages))
        }
        _ => true,
    }
}
