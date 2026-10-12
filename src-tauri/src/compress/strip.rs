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
    let named = if s.invalid_links || judge_bookmarks { Some(Dests::load(doc)?).filter(|d| !d.tree_unreadable()) } else { None };
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
/// tree. Loaded once per run. MuPDF (`pdf_lookup_dest`) reads `/Dests` alone whenever the catalog has one and the
/// tree only when it has not: `lookup` follows that, `lookup_any` finds a name in either.
pub(crate) struct Dests {
    /// The catalog's `/Dests`, whatever it is (a value that is not a dictionary finds nothing, as in MuPDF).
    old: Option<PdfObject>,
    tree: Option<PdfObject>,
    /// The `/Names` tree is there but could not be read.
    tree_unreadable: bool,
    /// The `/Dests` entries whose key is not UTF-8, by the key's bytes (a name object cannot be built from them).
    raw: std::collections::HashMap<Vec<u8>, PdfObject>,
}

impl Dests {
    pub(crate) fn load(doc: &PdfDocument) -> Result<Self, Error> {
        let catalog = doc.catalog()?;
        // MuPDF's `if (dests)`: a reference to a missing object is still there, and hides the tree
        let old = catalog.get_dict("Dests")?;
        let has_tree = catalog.get_dict("Names")?.and_then(|n| n.get_dict("Dests").ok().flatten()).is_some();
        let (tree, tree_unreadable) = if has_tree {
            match doc.load_name_tree(PdfObject::new_name("Dests")?) {
                Ok(t) => (Some(t), false),
                Err(_) => (None, true),
            }
        } else {
            (None, false)
        };
        let mut raw = std::collections::HashMap::new();
        if let Some(d) = &old {
            for i in 0..d.dict_len().unwrap_or(0) as i32 {
                if let (Ok(Some(k)), Ok(Some(v))) = (d.get_dict_key(i), d.get_dict_val(i))
                    && let Ok(name) = k.as_name()
                    && std::str::from_utf8(&name).is_err()
                {
                    raw.insert(name, v);
                }
            }
        }
        Ok(Self { old, tree, tree_unreadable, raw })
    }

    /// The `/Names` tree is there but could not be read.
    pub(crate) fn tree_unreadable(&self) -> bool {
        self.tree_unreadable
    }

    #[cfg(test)]
    pub(crate) fn with_unreadable_tree() -> Self {
        Self { old: None, tree: None, tree_unreadable: true, raw: Default::default() }
    }

    /// A name goes in as the object itself: its bytes need not be UTF-8 (`/caf#E9`) and MuPDF compares bytes.
    /// Resolved first: a dictionary lookup by a reference finds nothing.
    fn key(key: &PdfObject) -> PdfObject {
        key.resolve().ok().flatten().unwrap_or_else(|| key.share())
    }

    /// The destination a name or string stands for, MuPDF's way (`pdf_lookup_dest`): in `/Dests` alone when the
    /// catalog has one, else in the `/Names` tree. `Some(None)`: the document does not define it. `None`: the key
    /// could not be put in a form to look up (not UTF-8, a NUL, a lossy decode) or the tree could not be read,
    /// so it is not judged. `first`: the key is the link's own destination, which MuPDF's viewer turns into a URI and
    /// back (see `viewer_key`); a value found in `/Dests` that is itself a name or string (a later hop) is looked up
    /// as it is (`resolve_dest_rec`).
    pub(crate) fn lookup(&self, key: &PdfObject, first: bool) -> Option<Option<PdfObject>> {
        let key = Self::key(key);
        if self.old.is_some() { self.in_old(&key, false, first) } else { self.in_tree(&key) }
    }

    /// Like `lookup`, but a name that either source defines is found (the optimizer only keeps more: a viewer
    /// other than MuPDF may read the tree when there is a `/Dests` too).
    pub(crate) fn lookup_any(&self, key: &PdfObject) -> Option<Option<PdfObject>> {
        let key = Self::key(key);
        match (self.in_old(&key, true, true), self.in_tree(&key)) {
            (Some(Some(v)), _) | (_, Some(Some(v))) => Some(Some(v)),
            (Some(None), Some(None)) => Some(None),
            _ => None,
        }
    }

    /// In the `/Dests` dictionary. On the first hop a name or a string goes by the key MuPDF's viewer forms from its
    /// text (`pdf_resolve_link_dest`: `pdf_new_text_string`, see `viewer_key`) and by that alone: a key spelled with
    /// the string's own bytes is not looked at, and a name that is not UTF-8 is not judged. A later hop goes by the
    /// name itself or the string's own bytes as a C string (`pdf_dict_get`, `pdf_dict_gets`). `any` (the optimizer,
    /// which only keeps more) tries all of those, and the text as a UTF-8 name and as PDFDocEncoding bytes.
    fn in_old(&self, key: &PdfObject, any: bool, first: bool) -> Option<Option<PdfObject>> {
        let Some(old) = &self.old else { return Some(None) };
        if key.is_name().ok()? {
            let by_name = old.get_dict(key.share()).ok().flatten();
            if !first && !any {
                return Some(by_name);
            }
            // the viewer turns a link's own name into a string of its text (pdf_parse_link_dest, then
            // pdf_new_text_string): a name that is not UTF-8 has no faithful text, so it is not judged there
            let text = String::from_utf8(key.as_name().ok()?).ok();
            if !any {
                return self.by_bytes(old, &viewer_key(&text?));
            }
            let viewed = text.and_then(|t| self.by_bytes(old, &viewer_key(&t)).flatten());
            return Some(by_name.or(viewed));
        }
        let bytes = key.as_bytes().ok()?;
        let cut = &bytes[..bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len())];
        if !any && !first {
            return self.by_bytes(old, cut);
        }
        if any {
            // a NUL makes the key unbuildable here, so it is not judged
            if let Some(v) = self.by_bytes(old, &bytes)? {
                return Some(Some(v));
            }
            if let Some(v) = self.by_bytes(old, cut)? {
                return Some(Some(v));
            }
        }
        // no faithful text: the viewer's key is not known, so it is not judged; `any` leaves that to the tree
        let Some(text) = mupdf_text(&bytes) else { return if any { Some(None) } else { None } };
        let viewer = viewer_key(&text);
        if !any {
            return self.by_bytes(old, &viewer);
        }
        let mut forms = vec![viewer, text.as_bytes().to_vec()];
        if let Some(doc) = text.chars().map(pdf_doc_byte).collect::<Option<Vec<u8>>>() {
            forms.push(doc);
        }
        // a form that cannot be a name (a NUL) finds nothing; the tree still judges the text
        Some(forms.iter().find_map(|f| self.by_bytes(old, f).flatten()))
    }

    /// A `/Dests` key by its bytes: a UTF-8 one is a name object, any other is in the index built at load. `None`:
    /// the bytes cannot be a name (a NUL).
    fn by_bytes(&self, old: &PdfObject, bytes: &[u8]) -> Option<Option<PdfObject>> {
        match std::str::from_utf8(bytes) {
            Ok(k) => old.get_dict(k).ok(),
            Err(_) => Some(self.raw.get(bytes).map(|v| v.share())),
        }
    }

    /// In the `/Names` tree: MuPDF keys it by a string's decoded text (UTF-16, UTF-8 with or without a mark, else
    /// PDFDocEncoding). A stored `/name` is not looked up as a name: MuPDF's viewer turns it into a "#nameddest="
    /// link and searches the tree by the name's text (`pdf_resolve_link_dest`), so it is found by its own bytes.
    // shortcut: an empty-string key in the tree is not matched as MuPDF's `pdf_lookup_dest` would, and the tree is
    // searched whole (MuPDF skips direct kids and takes the first duplicate); upgrade if a case shows up.
    fn in_tree(&self, key: &PdfObject) -> Option<Option<PdfObject>> {
        if self.tree_unreadable {
            return None;
        }
        let text = if key.is_name().ok()? {
            // MuPDF's viewer URI-encodes the bytes and reads them back as UTF-8 with U+FFFD for the bad ones: lossy,
            // so a name that is not UTF-8 is not judged
            String::from_utf8(key.as_name().ok()?).ok()?
        } else {
            mupdf_text(&key.as_bytes().ok()?)?
        };
        // a key with no faithful text is not judged, tree or no tree
        let name = PdfObject::new_name(&text).ok()?;
        Some(self.tree.as_ref().and_then(|t| t.get_dict(name).ok().flatten()))
    }
}

/// The `/Dests` key MuPDF's viewer looks a string destination up by: the decoded text through `pdf_new_text_string`
/// (ASCII as it is, any other text as UTF-16BE with a byte order mark, never PDFDocEncoding), read by `pdf_dict_gets`
/// as a C string, which ends at the first 0 byte (so "caf\u{e9}" looks up the key FE FF).
fn viewer_key(text: &str) -> Vec<u8> {
    let text = text.split('\0').next().unwrap_or_default();
    let mut key = if text.is_ascii() {
        text.as_bytes().to_vec()
    } else {
        [0xFE, 0xFF].into_iter().chain(text.encode_utf16().flat_map(u16::to_be_bytes)).collect()
    };
    key.truncate(key.iter().position(|&c| c == 0).unwrap_or(key.len()));
    key
}

/// MuPDF's `is_valid_utf8` (pdf-parse.c): looser than UTF-8 (overlong forms, surrogates and values past U+10FFFF
/// pass) and no byte 0x18..=0x1F, which PDFDocEncoding keeps.
fn mupdf_takes_as_utf8(b: &[u8]) -> bool {
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let follow = match c {
            0x18..=0x1F => return false,
            0..=0x7F => 0,
            0x80..=0xBF => return false,
            0xC0..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF4 => 3,
            _ => return false,
        };
        for _ in 0..follow {
            i += 1;
            if b.get(i).is_none_or(|n| n & 0xC0 != 0x80) {
                return false;
            }
        }
        i += 1;
    }
    true
}

/// The text MuPDF's `pdf_new_utf8_from_pdf_string` makes of a string, which keys its name tree. A string with no
/// byte order mark is taken as UTF-8 first and only then as PDFDocEncoding. None: no faithful text (a lone
/// surrogate, a form MuPDF reads as UTF-8 and Rust does not, a NUL or an escape MuPDF drops), so the string is
/// not judged.
fn mupdf_text(b: &[u8]) -> Option<String> {
    let marked = [&[0xFE, 0xFF][..], &[0xFF, 0xFE], &[0xEF, 0xBB, 0xBF]].iter().any(|m| b.starts_with(m));
    if !marked {
        if mupdf_takes_as_utf8(b) {
            return String::from_utf8(b.to_vec()).ok();
        }
        // PDFDocEncoding has nothing at 0x7F and 0xAD: MuPDF reads them as NUL and the C string ends there
        if b.iter().any(|&c| c == 0x7F || c == 0xAD) {
            return None;
        }
    }
    let text = crate::outline::decode_text(b);
    // U+001B starts a language escape, which MuPDF skips
    (!text.contains(['\u{FFFD}', '\u{1B}'])).then_some(text)
}

/// The PDFDocEncoding byte of a character that has one in the range both it and Latin-1 share (the printable ASCII
/// and U+00A1..=U+00FF but U+00AD; U+00A0 is the Euro sign there).
fn pdf_doc_byte(c: char) -> Option<u8> {
    match c as u32 {
        0x20..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF => Some(c as u8),
        _ => None,
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

/// A destination value: an array, a dictionary holding `/D`, or a name or string looked up in the document's
/// destinations. A few hops at most (a dictionary's `/D` can be a name), so a loop in a damaged file ends.
fn dest_ok(doc: &PdfDocument, d: &Dests, v: &PdfObject, pages: Option<i32>) -> bool {
    let mut cur = v.share();
    for _ in 0..4 {
        if cur.is_array().unwrap_or(false) {
            return dest_array_ok(doc, &cur, pages);
        }
        let next = if cur.is_dict().unwrap_or(false) {
            cur.get_dict("D").ok().flatten()
        } else if cur.is_name().unwrap_or(false) || cur.is_string().unwrap_or(false) {
            match d.lookup_any(&cur) {
                Some(found) => found,
                None => return true,
            }
        } else {
            None
        };
        let Some(next) = next else { return false };
        cur = next;
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
/// (URI, remote go-to, launch, named, JavaScript ...) or a GoTo with no `/D`: those are not judged here.
fn bookmark_works(doc: &PdfDocument, d: &Dests, item: &PdfObject, pages: usize) -> bool {
    let pages = Some(pages as i32);
    if let Ok(Some(dest)) = item.get_dict("Dest") && !dest.is_null().unwrap_or(false) {
        return dest_ok(doc, d, &dest, pages);
    }
    match item.get_dict("A") {
        Ok(Some(a)) if a.is_dict().unwrap_or(false) && is_name(&a, "S", b"GoTo") => {
            match a.get_dict("D").ok().flatten() {
                Some(dest) if !dest.is_null().unwrap_or(false) => dest_ok(doc, d, &dest, pages),
                _ => true,
            }
        }
        _ => true,
    }
}
