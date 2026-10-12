// Adapted from pdfcraft (MIT OR Apache-2.0), crates/preflight/src/lib.rs @ b8ddf71
//! The conversion: the fix groups the person ticked, applied in a fixed order, and the write of a
//! new file. Every group is idempotent (running it on a converted document finds nothing to do) and
//! reads only what the checker reads, so the report after a conversion shows what is left.

use super::check::{self, NAMED_OK, FORBIDDEN_ACTIONS, annotation_forbidden, dict_of, embedded_is_pdf, embedded_stream, filters};
use super::fonts::{self, FontPlan};
use super::xmp::{self, Ident};
use super::{Convert, Family, FixGroup, Profile, Result, Status, bad};
use crate::a11y::tree::{name, text, xref_of};
use crate::a11y::{self, Fix, Progress};
use crate::engine::EngineError;
use mupdf::Buffer;
use mupdf::pdf::{Encryption, PdfDocument, PdfObject, PdfWriteOptions};
use std::sync::atomic::{AtomicBool, Ordering};

/// What a conversion did.
pub(crate) struct Applied {
    /// Pages the conversion changed (empty when it only touched the document as a whole).
    pub pages: Vec<u32>,
    pub log: Vec<String>,
    /// Whether the metadata step wrote the profile's identification (the file passed the other rules).
    pub identified: bool,
}

/// The order groups run in: structure first, metadata last (it mirrors what the others set).
const ORDER: [FixGroup; 11] = [
    FixGroup::Structure,
    FixGroup::Actions,
    FixGroup::Annotations,
    FixGroup::Forms,
    FixGroup::Images,
    FixGroup::Files,
    FixGroup::Colour,
    FixGroup::Fonts,
    FixGroup::Appearance,
    FixGroup::Accessibility,
    FixGroup::Metadata,
];

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) { Err(bad("cancelled")) } else { Ok(()) }
}

fn pages_of(doc: &PdfDocument) -> Result<Vec<PdfObject>> {
    let n = doc.page_count()?.max(0);
    (0..n).map(|p| Ok(doc.find_page(p)?)).collect()
}

/// Whether `a` (or an action chained after it) is one the standards forbid.
fn action_forbidden(a: &PdfObject, depth: usize) -> Result<bool> {
    let mut cur = Some(a.share());
    for _ in 0..8 {
        let Some(x) = cur.take() else { break };
        if !x.is_dict()? {
            break;
        }
        let s = name(&x, "S")?.unwrap_or_default();
        let named_bad = s == "Named" && name(&x, "N")?.is_some_and(|n| !NAMED_OK.contains(&n.as_str()));
        if FORBIDDEN_ACTIONS.contains(&s.as_str()) || named_bad {
            return Ok(true);
        }
        cur = match x.get_dict("Next")? {
            Some(n) if n.is_array()? => {
                if depth < 4 {
                    for e in n.array_iter()?.flatten() {
                        if action_forbidden(&e, depth + 1)? {
                            return Ok(true);
                        }
                    }
                }
                None
            }
            Some(n) => Some(n),
            None => None,
        };
    }
    Ok(false)
}

// ---- the run -------------------------------------------------------------------------------------

/// A header the standards accept: what a new file gets when it is written (`write_new`).
pub(crate) const NEW_FILE_HEAD: &[u8] = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";

/// `head` is the first bytes of the file as it is now (a new file passes `NEW_FILE_HEAD`): the metadata step
/// checks the result before it identifies the file.
pub(crate) fn apply(doc: &mut PdfDocument, profile: Profile, c: &Convert, stem: &str, new_file: bool, head: &[u8], progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Applied> {
    if c.groups.is_empty() {
        return Err(bad("choose what to fix"));
    }
    if !new_file && doc.trailer()?.get_dict("Encrypt")?.is_some() {
        return Err(bad("This document is encrypted, and PDF/A allows no encryption: save the conversion as a new file"));
    }
    // An in-place conversion removes content, so its save is a full rewrite: it would void a certification and
    // break a signature. Converting into a new file leaves the open document as it is.
    if !new_file {
        if crate::outline::certified(doc)?.is_some() {
            return Err(bad("This document is certified, so converting it in place would void the certification: save the conversion as a new file"));
        }
        if crate::outline::signed(doc) {
            return Err(bad("This document is signed, and converting it in place would break its signature: save the conversion as a new file"));
        }
    }
    let want = |g: FixGroup| c.groups.contains(&g);
    let fam = profile.family();
    let mut log: Vec<String> = Vec::new();
    let mut pages_changed = false;
    let mut identified = false;

    let survey = if want(FixGroup::Fonts) || want(FixGroup::Images) || want(FixGroup::Appearance) || want(FixGroup::Colour) {
        Some(check::survey(doc, progress, cancel)?)
    } else {
        None
    };
    let steps: Vec<FixGroup> = ORDER.iter().copied().filter(|g| want(*g)).collect();
    for (i, g) in steps.iter().enumerate() {
        cancelled(cancel)?;
        progress(Progress { done: i as u32, pages: steps.len() as u32, stage: "convert" });
        match g {
            FixGroup::Structure => structure(doc, profile, fam, new_file, progress, cancel, &mut log)?,
            FixGroup::Actions => actions(doc, &mut log)?,
            FixGroup::Annotations => pages_changed |= annotations(doc, fam, &mut log)?,
            FixGroup::Forms => pages_changed |= forms(doc, &mut log)?,
            FixGroup::Images => pages_changed |= images(survey.as_ref().map(|s| &s.found.images[..]).unwrap_or(&[]), &mut log)?,
            FixGroup::Files => pages_changed |= files(doc, profile, &mut log)?,
            FixGroup::Colour => colour(doc, survey.as_ref().map(|s| s.device).unwrap_or([false; 3]), &mut log)?,
            FixGroup::Fonts => pages_changed |= font_group(doc, fam, survey.as_ref().map(|s| &s.found.fonts[..]).unwrap_or(&[]), &mut log)?,
            FixGroup::Appearance => pages_changed |= appearance(doc, fam, survey.as_ref(), &mut log)?,
            FixGroup::Accessibility => pages_changed |= accessibility(doc, c, stem, progress, cancel, &mut log)?,
            FixGroup::Metadata => identified = metadata(doc, profile, new_file, head, cancel, &mut log)?,
        }
    }
    progress(Progress { done: steps.len() as u32, pages: steps.len() as u32, stage: "convert" });
    let pages = if pages_changed { (0..doc.page_count()?.max(0) as u32).collect() } else { Vec::new() };
    Ok(Applied { pages, log, identified })
}

/// The bytes of a new file: no encryption, a classic cross-reference table, a header the standard
/// accepts.
/// Brings the XMP packet in line with the document information after a property changed (the
/// standards want the two to agree, and a reader that prefers XMP would otherwise show the old
/// value). A document without a packet is left without one. A packet that is not UTF-8 is refused,
/// so the properties are never left silently disagreeing. Returns whether the packet was rewritten.
pub(crate) fn sync_xmp_with_info(doc: &mut PdfDocument) -> Result<bool> {
    let cat = doc.catalog()?;
    let Some(mut meta) = cat.get_dict("Metadata")?.filter(|m| m.is_stream().unwrap_or(false)) else { return Ok(false) };
    let bytes = meta.read_stream().unwrap_or_default();
    let old = String::from_utf8(bytes).map_err(|_| bad("the XMP metadata isn't UTF-8, so the properties aren't changed here; fix it in the source application"))?;
    let info = doc.trailer()?.get_dict("Info")?.filter(|i| i.is_dict().unwrap_or(false));
    let props = check::info_props(info.as_ref());
    // A cleared property leaves nothing to mirror, so its old XMP value is taken out here.
    let mut base = old.clone();
    for (set, key) in [
        (&props.title, "dc:title"),
        (&props.author, "dc:creator"),
        (&props.subject, "dc:description"),
        (&props.keywords, "pdf:Keywords"),
    ] {
        if set.as_deref().is_none_or(str::is_empty) {
            xmp::remove_prop(&mut base, key);
        }
    }
    let new = xmp::merge(&base, &props, &[], &[]);
    if new == old {
        return Ok(false);
    }
    let buf = Buffer::from_bytes(new.as_bytes())?;
    // A stream with a filter becomes plain: other tools read the packet without decoding it.
    meta.write_stream_buffer(&buf)?;
    meta.dict_put("Type", PdfObject::new_name("Metadata")?)?;
    meta.dict_put("Subtype", PdfObject::new_name("XML")?)?;
    Ok(true)
}

pub(crate) fn write_new(doc: &PdfDocument, _profile: Profile) -> Result<Vec<u8>> {
    let mut o = PdfWriteOptions::default();
    o.set_garbage_level(3).set_compress(true).set_encryption(Encryption::None);
    let mut out = Vec::new();
    doc.write_to_with_options(&mut out, o)?;
    // `%PDF-2.0` and later headers are rewritten in place (same length) as PDF 1.7, which is what
    // the structures written here are.
    if out.starts_with(b"%PDF-") && out.len() > 8 {
        let ok = out[5] == b'1' && out[6] == b'.' && (b'0'..=b'7').contains(&out[7]);
        if !ok && out[6] == b'.' && out[7].is_ascii_digit() {
            out[5] = b'1';
            out[7] = b'7';
        }
    }
    Ok(out)
}

// ---- structure -----------------------------------------------------------------------------------

fn structure(doc: &mut PdfDocument, profile: Profile, fam: Family, new_file: bool, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool, log: &mut Vec<String>) -> Result<()> {
    let mut trailer = doc.trailer()?;
    let has_id = trailer.get_dict("ID")?.is_some_and(|id| id.is_array().unwrap_or(false) && id.len().unwrap_or(0) == 2);
    if !has_id {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|e| bad(format!("no random source: {e}")))?;
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let id = doc.new_object_from_str(&format!("[<{hex}><{hex}>]"))?;
        trailer.dict_put("ID", id)?;
        log.push("Added the file identifier (/ID)".into());
    }
    let cat = doc.catalog()?;
    // Automatic layer states (/AS) are not allowed in PDF/A-2 and -3 (PDF/A-1 has no layers at all).
    if fam == Family::A23 {
        if let Some(oc) = dict_of(&cat, "OCProperties") {
            let mut configs: Vec<PdfObject> = Vec::new();
            configs.extend(dict_of(&oc, "D"));
            if let Some(cs) = oc.get_dict("Configs")?.filter(|c| c.is_array().unwrap_or(false)) {
                configs.extend(cs.array_iter()?.flatten().filter(|c| c.is_dict().unwrap_or(false)));
            }
            let mut n = 0;
            for mut c in configs {
                if c.get_dict("AS")?.is_some() {
                    c.dict_delete("AS")?;
                    n += 1;
                }
            }
            if n > 0 {
                log.push("Removed the automatic layer states (/AS)".into());
            }
        }
    }
    // Trim and art boxes (PDF/X).
    if fam == Family::X {
        let mut n = 0;
        for mut page in pages_of(doc)? {
            if page.get_dict("TrimBox")?.is_none() && page.get_dict("ArtBox")?.is_none() {
                let src = page.get_dict_inheritable("CropBox")?.or(page.get_dict_inheritable("MediaBox")?).filter(|b| b.is_array().unwrap_or(false));
                if let Some(b) = src {
                    page.dict_put("TrimBox", b.copy_array()?)?;
                    n += 1;
                }
            }
        }
        if n > 0 {
            log.push(format!("Set the trim box of {}", plural(n, "page", "pages")));
        }
    }
    // LZW streams are decoded and stored plain (the writer compresses them again with Flate).
    if matches!(fam, Family::A1 | Family::A23) {
        let count = doc.count_objects()?;
        let step = (count / 50).max(1000);
        let mut n = 0;
        for i in 1..count as i32 {
            if i as u32 % step == 0 {
                cancelled(cancel)?;
                progress(Progress { done: i as u32, pages: count, stage: "objects" });
            }
            let mut o = doc.new_indirect(i, 0)?;
            if !o.is_stream().unwrap_or(false) {
                continue;
            }
            let fl = filters(&o);
            if !fl.iter().any(|f| f == "LZWDecode" || f == "LZW") {
                continue;
            }
            let plain = fl.iter().all(|f| matches!(f.as_str(), "LZWDecode" | "LZW" | "ASCII85Decode" | "A85" | "ASCIIHexDecode" | "AHx" | "FlateDecode" | "Fl"));
            if !plain {
                continue;
            }
            let data = o.read_stream()?;
            o.write_stream_buffer(&Buffer::from_bytes(&data)?)?;
            n += 1;
        }
        if n > 0 {
            log.push(format!("Re-encoded {} without LZW", plural(n, "stream", "streams")));
        }
    }
    if new_file {
        log.push("Wrote a new file without encryption and object streams, with a standard header".into());
    }
    let _ = profile;
    Ok(())
}

// ---- actions -------------------------------------------------------------------------------------

#[derive(Default)]
struct ActionCount {
    actions: usize,
    additional: usize,
}

fn clean_owner(o: &mut PdfObject, n: &mut ActionCount) -> Result<()> {
    if let Some(a) = o.get_dict("A")? {
        if a.is_dict()? && action_forbidden(&a, 0)? {
            o.dict_delete("A")?;
            n.actions += 1;
        }
    }
    if o.get_dict("AA")?.is_some() {
        o.dict_delete("AA")?;
        n.additional += 1;
    }
    Ok(())
}

fn actions(doc: &mut PdfDocument, log: &mut Vec<String>) -> Result<()> {
    let mut n = ActionCount::default();
    let mut cat = doc.catalog()?;
    if let Some(a) = cat.get_dict("OpenAction")?.filter(|a| a.is_dict().unwrap_or(false)) {
        if action_forbidden(&a, 0)? {
            cat.dict_delete("OpenAction")?;
            n.actions += 1;
        }
    }
    if cat.get_dict("AA")?.is_some() {
        cat.dict_delete("AA")?;
        n.additional += 1;
    }
    let mut js = false;
    if let Some(mut names) = dict_of(&cat, "Names") {
        if names.get_dict("JavaScript")?.is_some() {
            names.dict_delete("JavaScript")?;
            js = true;
        }
    }
    // Bookmarks.
    if let Some(root) = dict_of(&cat, "Outlines") {
        let mut stack = vec![root.get_dict("First")?];
        let mut seen = 0;
        while let Some(item) = stack.pop() {
            let Some(mut it) = item else { continue };
            seen += 1;
            if seen > 50_000 || !it.is_dict()? {
                break;
            }
            if let Some(a) = it.get_dict("A")?.filter(|a| a.is_dict().unwrap_or(false)) {
                if action_forbidden(&a, 0)? {
                    it.dict_delete("A")?;
                    n.actions += 1;
                }
            }
            stack.push(it.get_dict("Next")?);
            stack.push(it.get_dict("First")?);
        }
    }
    // Fields (and the widgets that are their own fields).
    if let Some(af) = dict_of(&cat, "AcroForm") {
        if let Some(fields) = af.get_dict("Fields")?.filter(|f| f.is_array().unwrap_or(false)) {
            let mut stack: Vec<PdfObject> = fields.array_iter()?.flatten().collect();
            let mut seen = 0;
            while let Some(mut f) = stack.pop() {
                seen += 1;
                if seen > 100_000 {
                    break;
                }
                if !f.is_dict()? {
                    continue;
                }
                clean_owner(&mut f, &mut n)?;
                if let Some(kids) = f.get_dict("Kids")?.filter(|k| k.is_array().unwrap_or(false)) {
                    stack.extend(kids.array_iter()?.flatten());
                }
            }
        }
    }
    // Pages and their annotations (popups carry no actions).
    for mut page in pages_of(doc)? {
        if page.get_dict("AA")?.is_some() {
            page.dict_delete("AA")?;
            n.additional += 1;
        }
        if let Some(an) = page.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) {
            for mut a in an.array_iter()?.flatten() {
                if a.is_dict()? {
                    clean_owner(&mut a, &mut n)?;
                }
            }
        }
    }
    if n.actions > 0 {
        log.push(format!("Removed {} (JavaScript, launch and other forbidden kinds)", plural(n.actions, "action", "actions")));
    }
    if n.additional > 0 {
        log.push(format!("Removed {}", plural(n.additional, "additional-actions entry", "additional-actions entries")));
    }
    if js {
        log.push("Removed the document-level JavaScript".into());
    }
    Ok(())
}

// ---- annotations ---------------------------------------------------------------------------------

fn annotations(doc: &mut PdfDocument, fam: Family, log: &mut Vec<String>) -> Result<bool> {
    let mut removed = 0usize;
    let mut flagged = 0usize;
    for page in pages_of(doc)? {
        let Some(mut arr) = page.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) else { continue };
        let mut gone: Vec<i32> = Vec::new();
        let n = arr.len()? as i32;
        for i in (0..n).rev() {
            let Some(mut a) = arr.get_array(i)? else { continue };
            if !a.is_dict()? {
                continue;
            }
            let sub = name(&a, "Subtype")?.unwrap_or_default();
            if sub == "Popup" {
                continue;
            }
            if annotation_forbidden(fam, &sub) {
                let x = xref_of(&a)?;
                if x != 0 {
                    gone.push(x);
                }
                arr.array_delete(i)?;
                removed += 1;
                continue;
            }
            let f = a.get_dict("F")?.and_then(|v| v.as_int().ok()).unwrap_or(0);
            let want = (f | 4) & !(1 | 2 | 32);
            if want != f {
                a.dict_put("F", PdfObject::new_int(want)?)?;
                flagged += 1;
            }
        }
        // A popup of a removed annotation has no parent left.
        if !gone.is_empty() {
            let n = arr.len()? as i32;
            for i in (0..n).rev() {
                let Some(a) = arr.get_array(i)? else { continue };
                if a.is_dict()? && name(&a, "Subtype")?.as_deref() == Some("Popup") {
                    if let Some(p) = a.get_dict("Parent")? {
                        if gone.contains(&xref_of(&p)?) {
                            arr.array_delete(i)?;
                        }
                    }
                }
            }
        }
    }
    if removed > 0 {
        log.push(format!("Removed {}", plural(removed, "annotation that the standard doesn't allow", "annotations that the standard doesn't allow")));
    }
    if flagged > 0 {
        log.push(format!("Set {} to print and not be hidden", plural(flagged, "annotation", "annotations")));
    }
    Ok(removed + flagged > 0)
}

// ---- forms ---------------------------------------------------------------------------------------

fn forms(doc: &mut PdfDocument, log: &mut Vec<String>) -> Result<bool> {
    let mut changed = false;
    let cat = doc.catalog()?;
    if let Some(af) = dict_of(&cat, "AcroForm") {
        let need = af.get_dict("NeedAppearances")?.is_some_and(|v| v.is_bool().unwrap_or(false) && v.as_bool().unwrap_or(false));
        if need {
            crate::forms::settle_need_appearances(doc, 0).map_err(|e| bad(format!("couldn't draw the form fields: {e}")))?;
            log.push("Drew the form field appearances into the file".into());
            changed = true;
        }
        let mut af = dict_of(&doc.catalog()?, "AcroForm").unwrap_or(af);
        if af.get_dict("XFA")?.is_some() {
            af.dict_delete("XFA")?;
            log.push("Removed the XFA form data".into());
        }
    }
    Ok(changed)
}

// ---- images --------------------------------------------------------------------------------------

fn images(imgs: &[PdfObject], log: &mut Vec<String>) -> Result<bool> {
    let mut n = 0;
    for img in imgs {
        let mut o = img.share();
        let mut touched = false;
        if o.get_dict("Interpolate")?.is_some_and(|v| v.is_bool().unwrap_or(false) && v.as_bool().unwrap_or(false)) {
            o.dict_delete("Interpolate")?;
            touched = true;
        }
        for k in ["Alternates", "OPI"] {
            if o.get_dict(k)?.is_some() {
                o.dict_delete(k)?;
                touched = true;
            }
        }
        n += touched as usize;
    }
    if n > 0 {
        log.push(format!("Cleaned {} (interpolation, alternates, OPI)", plural(n, "image", "images")));
    }
    Ok(n > 0)
}

// ---- embedded files ------------------------------------------------------------------------------

fn spec_removable(profile: Profile, fs: &PdfObject) -> bool {
    match profile {
        Profile::Pdfa1b => true,
        Profile::Pdfa2b | Profile::Pdfa2u => !embedded_is_pdf(fs),
        _ => false,
    }
}

/// Drops the removable filespecs of a name tree node and its kids. Returns how many.
fn prune_tree(node: &PdfObject, profile: Profile, depth: usize) -> Result<usize> {
    if depth > 16 || !node.is_dict()? {
        return Ok(0);
    }
    let mut n = 0;
    if let Some(mut names) = node.get_dict("Names")?.filter(|a| a.is_array().unwrap_or(false)) {
        let len = names.len()? as i32;
        let mut i = len - 2;
        while i >= 0 {
            if let Some(v) = names.get_array(i + 1)? {
                if v.is_dict()? && dict_of(&v, "EF").is_some() && spec_removable(profile, &v) {
                    names.array_delete(i + 1)?;
                    names.array_delete(i)?;
                    n += 1;
                }
            }
            i -= 2;
        }
    }
    if let Some(kids) = node.get_dict("Kids")?.filter(|a| a.is_array().unwrap_or(false)) {
        for k in kids.array_iter()?.flatten().take(5000) {
            n += prune_tree(&k, profile, depth + 1)?;
        }
    }
    Ok(n)
}

/// Gives every filespec of a name tree (and of FileAttachment annotations) an AFRelationship.
fn relate_tree(node: &PdfObject, depth: usize, n: &mut usize) -> Result<()> {
    if depth > 16 || !node.is_dict()? {
        return Ok(());
    }
    if let Some(names) = node.get_dict("Names")?.filter(|a| a.is_array().unwrap_or(false)) {
        let len = names.len()? as i32;
        let mut i = 1;
        while i < len {
            if let Some(mut v) = names.get_array(i)? {
                relate(&mut v, n)?;
            }
            i += 2;
        }
    }
    if let Some(kids) = node.get_dict("Kids")?.filter(|a| a.is_array().unwrap_or(false)) {
        for k in kids.array_iter()?.flatten().take(5000) {
            relate_tree(&k, depth + 1, n)?;
        }
    }
    Ok(())
}

fn relate(fs: &mut PdfObject, n: &mut usize) -> Result<()> {
    if fs.is_dict()? && dict_of(fs, "EF").is_some() && name(fs, "AFRelationship")?.is_none() {
        fs.dict_put("AFRelationship", PdfObject::new_name("Unspecified")?)?;
        *n += 1;
    }
    Ok(())
}

/// PDF/A-3: the media type of each embedded file (a MIME /Subtype on its stream, `application/octet-stream`
/// when it has none) and the file's place in the catalog's /AF array.
fn associate(doc: &mut PdfDocument, ef: &PdfObject, log: &mut Vec<String>) -> Result<()> {
    let mut specs = Vec::new();
    collect_specs(ef, 0, &mut specs)?;
    let mut cat = doc.catalog()?;
    let mut listed: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut af = cat.get_dict("AF")?.filter(|a| a.is_array().unwrap_or(false));
    if let Some(a) = &af {
        for e in a.array_iter()?.flatten() {
            let n = xref_of(&e)?;
            if n != 0 {
                listed.insert(n);
            }
        }
    }
    let (mut typed, mut added) = (0usize, 0usize);
    for fs in specs {
        if !fs.is_dict()? || dict_of(&fs, "EF").is_none() {
            continue;
        }
        if let Some(mut s) = embedded_stream(&fs) {
            if name(&s, "Subtype")?.is_none() {
                s.dict_put("Subtype", PdfObject::new_name("application/octet-stream")?)?;
                typed += 1;
            }
        }
        let x = xref_of(&fs)?;
        if x != 0 && !listed.contains(&x) {
            if af.is_none() {
                let arr = doc.new_array()?;
                cat.dict_put("AF", arr)?;
                af = cat.get_dict("AF")?;
            }
            if let Some(a) = af.as_mut() {
                a.array_push(fs.clone())?;
                listed.insert(x);
                added += 1;
            }
        }
    }
    if typed > 0 {
        log.push(format!("Gave {} the media type application/octet-stream (its real type is not known)", plural(typed, "embedded file", "embedded files")));
    }
    if added > 0 {
        log.push(format!("Listed {} in the document's /AF array", plural(added, "embedded file", "embedded files")));
    }
    Ok(())
}

/// The filespecs of a name tree node and its kids.
fn collect_specs(node: &PdfObject, depth: usize, out: &mut Vec<PdfObject>) -> Result<()> {
    if depth > 16 || !node.is_dict()? {
        return Ok(());
    }
    if let Some(names) = node.get_dict("Names")?.filter(|a| a.is_array().unwrap_or(false)) {
        let len = names.len()? as i32;
        let mut i = 1;
        while i < len {
            if let Some(v) = names.get_array(i)? {
                out.push(v);
            }
            i += 2;
        }
    }
    if let Some(kids) = node.get_dict("Kids")?.filter(|a| a.is_array().unwrap_or(false)) {
        for k in kids.array_iter()?.flatten().take(5000) {
            collect_specs(&k, depth + 1, out)?;
        }
    }
    Ok(())
}

fn files(doc: &mut PdfDocument, profile: Profile, log: &mut Vec<String>) -> Result<bool> {
    let cat = doc.catalog()?;
    let mut removed = 0usize;
    let mut related = 0usize;
    let ef = dict_of(&cat, "Names").and_then(|n| dict_of(&n, "EmbeddedFiles"));
    if profile == Profile::Pdfa3b {
        if let Some(ef) = &ef {
            relate_tree(ef, 0, &mut related)?;
            associate(doc, ef, log)?;
        }
    } else if let Some(ef) = &ef {
        removed += prune_tree(ef, profile, 0)?;
    }
    let mut pages = false;
    for page in pages_of(doc)? {
        let Some(mut arr) = page.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) else { continue };
        let n = arr.len()? as i32;
        for i in (0..n).rev() {
            let Some(a) = arr.get_array(i)? else { continue };
            if !a.is_dict()? || name(&a, "Subtype")?.as_deref() != Some("FileAttachment") {
                continue;
            }
            let Some(mut fs) = a.get_dict("FS")?.filter(|f| f.is_dict().unwrap_or(false)) else { continue };
            if dict_of(&fs, "EF").is_none() {
                continue;
            }
            if profile == Profile::Pdfa3b {
                relate(&mut fs, &mut related)?;
            } else if spec_removable(profile, &fs) {
                arr.array_delete(i)?;
                removed += 1;
                pages = true;
            }
        }
    }
    if removed > 0 {
        log.push(format!("Removed {} (their content is no longer in the file)", plural(removed, "embedded file", "embedded files")));
    }
    if related > 0 {
        log.push(format!("Marked {} with the relationship Unspecified", plural(related, "embedded file", "embedded files")));
    }
    Ok(pages)
}

// ---- colour --------------------------------------------------------------------------------------

fn colour(doc: &mut PdfDocument, device: [bool; 3], log: &mut Vec<String>) -> Result<()> {
    let mut cat = doc.catalog()?;
    let existing = cat.get_dict("OutputIntents")?.filter(|o| o.is_array().unwrap_or(false));
    // PDF/A allows one output profile in the file: the profile of an intent that is already there
    // (a PDF/X one, say) is shared, never a second one written next to it.
    let mut shared: Option<(PdfObject, i32)> = None;
    if let Some(arr) = &existing {
        for oi in arr.array_iter()?.flatten() {
            if !oi.is_dict()? {
                continue;
            }
            if name(&oi, "S")?.as_deref() == Some("GTS_PDFA1") {
                return Ok(());
            }
            if shared.is_none() {
                if let Some(p) = oi.get_dict("DestOutputProfile")?.filter(|p| p.is_stream().unwrap_or(false)) {
                    let n = p.get_dict("N")?.and_then(|v| v.as_int().ok()).unwrap_or(0);
                    shared = Some((p, n));
                }
            }
        }
    }
    if device[2] {
        log.push("Left the output intent alone: DeviceCMYK is used and needs a CMYK output condition you choose".into());
        return Ok(());
    }
    let (profile, reused) = match shared {
        Some((p, 3)) => (p, true),
        Some((_, n)) => {
            log.push(format!(
                "Left the output intent alone: the file's output intent has a {n}-component profile, and PDF/A allows one output profile only, so an sRGB one can't be added next to it"
            ));
            return Ok(());
        }
        None => {
            let icc = super::icc::srgb();
            let mut sd = doc.new_dict()?;
            sd.dict_put("N", PdfObject::new_int(3)?)?;
            sd.dict_put("Alternate", PdfObject::new_name("DeviceRGB")?)?;
            (doc.add_stream(&Buffer::from_bytes(&icc)?, Some(&sd), false)?, false)
        }
    };
    let mut oi = doc.new_object_from_str(
        "<</Type/OutputIntent/S/GTS_PDFA1/OutputConditionIdentifier(sRGB IEC61966-2.1)/RegistryName(http://www.color.org)/Info(sRGB IEC61966-2.1)>>",
    )?;
    oi.dict_put("DestOutputProfile", profile)?;
    let oi = doc.add_object(&oi)?;
    match existing {
        Some(mut arr) => arr.array_push(oi)?,
        None => {
            let mut arr = doc.new_array()?;
            arr.array_push(oi)?;
            cat.dict_put("OutputIntents", arr)?;
        }
    }
    log.push(if reused { "Added a PDF/A output intent that shares the file's existing output profile".into() } else { "Added an sRGB output intent".into() });
    Ok(())
}

// ---- fonts ---------------------------------------------------------------------------------------

fn font_group(doc: &mut PdfDocument, fam: Family, found: &[PdfObject], log: &mut Vec<String>) -> Result<bool> {
    let mut embedded = 0;
    let mut installed = 0;
    let mut cid = 0;
    // Fonts that came with a program of their own, and those the conversion embedded.
    let mut own = 0usize;
    let mut ours: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut no_subset = false;
    let mut a1_encoding = 0usize;
    for f in found {
        match fonts::classify(f)? {
            FontPlan::Embedded => {
                if name(f, "Subtype")?.as_deref() != Some("Type3") {
                    own += 1;
                }
                if fonts::set_identity_cid_to_gid(doc, f)? {
                    cid += 1;
                }
                if fam == Family::A1 && fonts::truetype_encoding_unfit(f)? {
                    a1_encoding += 1;
                }
            }
            plan @ FontPlan::Builtin { .. } => {
                log.push(fonts::embed(doc, f, &plan, fam)?.log);
                ours.insert(xref_of(f)?);
                embedded += 1;
            }
            plan @ FontPlan::Installed { .. } => {
                let e = fonts::embed(doc, f, &plan, fam)?;
                log.push(e.log);
                no_subset |= e.no_subset;
                ours.insert(xref_of(f)?);
                embedded += 1;
                installed += 1;
            }
            FontPlan::Blocked { reason, .. } => log.push(format!("Not embedded: {reason}")),
        }
    }
    if cid > 0 {
        log.push(format!("Added the CIDToGIDMap to {}", plural(cid, "font", "fonts")));
    }
    if a1_encoding > 0 {
        log.push(format!(
            "Left {} as they are: PDF/A-1 wants WinAnsiEncoding or MacRomanEncoding without Differences on a TrueType font, and these have another Encoding",
            plural(a1_encoding, "embedded TrueType font", "embedded TrueType fonts")
        ));
    }
    // A full installed font is large: keep the glyphs the pages use. The subsetting works on the
    // whole document, so it only runs when nothing else would be cut down with them.
    if installed > 0 {
        match no_subsetting_reason(doc, &ours, own, no_subset)? {
            Some(why) => log.push(format!("Embedded the fonts in full: {why}")),
            None => {
                if let Err(e) = doc.subset_fonts() {
                    log.push(format!("Fonts embedded in full (subsetting failed: {e})"));
                } else {
                    log.push("Subset the embedded fonts to the glyphs the pages use".into());
                }
            }
        }
    }
    Ok(embedded + cid > 0)
}

/// Why the fonts must not be subset (`None` when they can): a font whose licence forbids it, fonts the
/// document brought with it, or a form's default-resource fonts, which fields draw with later.
pub(crate) fn no_subsetting_reason(doc: &PdfDocument, ours: &std::collections::HashSet<i32>, own: usize, forbidden: bool) -> Result<Option<String>> {
    if forbidden {
        return Ok(Some("a font's licence flags forbid subsetting it".into()));
    }
    if own > 0 {
        return Ok(Some("the document has fonts of its own, which subsetting would cut down too".into()));
    }
    let cat = doc.catalog()?;
    let why = || Some("the form's default resources use fonts that stay complete so fields can still be edited".to_string());
    if let Some(dr) = dict_of(&cat, "AcroForm").and_then(|a| dict_of(&a, "DR")) {
        if dr_has_font_that_stays(&dr, ours)? {
            return Ok(why());
        }
    }
    // A field can carry default resources of its own (on the widget or on a parent field).
    for page in pages_of(doc)? {
        let Some(an) = page.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) else { continue };
        for a in an.array_iter()?.flatten() {
            let mut cur = Some(a);
            for _ in 0..8 {
                let Some(w) = cur.take() else { break };
                if !w.is_dict()? {
                    break;
                }
                if let Some(dr) = dict_of(&w, "DR") {
                    if dr_has_font_that_stays(&dr, ours)? {
                        return Ok(why());
                    }
                }
                cur = w.get_dict("Parent")?;
            }
        }
    }
    Ok(None)
}

/// Whether a default-resources dictionary names a font that has a program or is one the conversion embedded.
fn dr_has_font_that_stays(dr: &PdfObject, ours: &std::collections::HashSet<i32>) -> Result<bool> {
    let Some(fonts) = dict_of(dr, "Font") else { return Ok(false) };
    for k in 0..fonts.dict_len()? as i32 {
        let Some(v) = fonts.get_dict_val(k)? else { continue };
        let has_file = v.is_dict()? && v.get_dict("FontDescriptor")?.is_some_and(|d| ["FontFile", "FontFile2", "FontFile3"].iter().any(|f| d.get_dict(*f).ok().flatten().is_some()));
        if has_file || (v.is_indirect()? && ours.contains(&v.as_indirect()?)) {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---- appearance ----------------------------------------------------------------------------------

fn appearance(doc: &mut PdfDocument, fam: Family, survey: Option<&check::Survey>, log: &mut Vec<String>) -> Result<bool> {
    let Some(s) = survey else { return Ok(false) };
    let mut n = 0usize;
    for g in &s.found.gstates {
        let mut g = g.share();
        let mut touched = false;
        if fam == Family::A1 {
            for k in ["CA", "ca"] {
                if g.get_dict(k)?.filter(|v| v.is_number().unwrap_or(false)).and_then(|v| v.as_float().ok()).is_some_and(|v| v < 1.0) {
                    g.dict_put(k, PdfObject::new_real(1.0)?)?;
                    touched = true;
                }
            }
            if let Some(m) = g.get_dict("SMask")? {
                let none = m.is_name()? && m.as_name().map(|n| n == b"None").unwrap_or(false);
                if !none {
                    g.dict_put("SMask", PdfObject::new_name("None")?)?;
                    touched = true;
                }
            }
            if let Some(b) = g.get_dict("BM")? {
                let first = if b.is_array()? { b.get_array(0)? } else { Some(b) };
                let ok = first.is_some_and(|f| f.is_name().unwrap_or(false) && f.as_name().map(|n| n == b"Normal" || n == b"Compatible").unwrap_or(false));
                if !ok {
                    g.dict_put("BM", PdfObject::new_name("Normal")?)?;
                    touched = true;
                }
            }
        }
        for k in ["TR", "TR2"] {
            if g.get_dict(k)?.is_some() {
                g.dict_delete(k)?;
                touched = true;
            }
        }
        n += touched as usize;
    }
    if fam == Family::A1 {
        for img in &s.found.images {
            let mut o = img.share();
            let mut touched = false;
            for k in ["SMask", "SMaskInData"] {
                if o.get_dict(k)?.is_some() {
                    o.dict_delete(k)?;
                    touched = true;
                }
            }
            n += touched as usize;
        }
        let mut groups: Vec<PdfObject> = s.found.forms.iter().map(PdfObject::share).collect();
        groups.extend(pages_of(doc)?);
        for mut o in groups {
            if let Some(g) = dict_of(&o, "Group") {
                if name(&g, "S")?.as_deref() == Some("Transparency") {
                    o.dict_delete("Group")?;
                    n += 1;
                }
            }
        }
    }
    if n > 0 {
        log.push(format!("Made {} opaque or dropped their transfer functions", plural(n, "object", "objects")));
    }
    Ok(n > 0)
}

// ---- accessibility -------------------------------------------------------------------------------

fn accessibility(doc: &mut PdfDocument, c: &Convert, stem: &str, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool, log: &mut Vec<String>) -> Result<bool> {
    let mut changed = false;
    let mut try_fix = |doc: &mut PdfDocument, fix: Fix, what: &str, log: &mut Vec<String>| -> Result<bool> {
        match a11y::apply_fix_with(doc, &fix, stem, progress, cancel) {
            Ok(_) => {
                log.push(what.to_string());
                Ok(true)
            }
            // Nothing to do, or no usable input (a language that isn't a tag): the others still apply.
            Err(EngineError::Invalid(_)) => Ok(false),
            Err(e) => Err(e),
        }
    };
    let title = doc.trailer()?.get_dict("Info")?.filter(|i| i.is_dict().unwrap_or(false)).and_then(|i| text(&i, "Title").ok().flatten()).filter(|t| !t.trim().is_empty());
    if title.is_none() {
        try_fix(doc, Fix::Title { title: None }, "Set the document title from the file name", log)?;
    }
    let has_lang = doc.catalog()?.get_dict("Lang")?.is_some();
    if !has_lang {
        if let Some(l) = c.lang.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            try_fix(doc, Fix::Language { lang: l.to_string() }, &format!("Set the document language to {l}"), log)?;
        }
    }
    try_fix(doc, Fix::DisplayDocTitle, "Showed the document title in the title bar", log)?;
    changed |= try_fix(doc, Fix::TabOrder, "Set the tab order to structure order", log)?;
    changed |= try_fix(doc, Fix::FieldTooltips, "Added form field tooltips", log)?;
    Ok(changed)
}

// ---- metadata ------------------------------------------------------------------------------------

/// Whether the file would pass `profile` once the metadata is in place: every rule the other groups
/// answer for, checked on the document as it is now. What only a new file's writing settles (header,
/// encryption, object streams) counts as settled for a new file. In the PDF/A profiles a rule that
/// needs a person's check (an embedded PDF's own conformance, a missing modification date) blocks the
/// identification too: the file does not say it is PDF/A while a part of that is unchecked.
/// `Err(names)` lists what still fails or is unchecked.
fn conforms_but_for_metadata(doc: &PdfDocument, profile: Profile, new_file: bool, head: &[u8], cancel: &AtomicBool) -> Result<std::result::Result<(), Vec<&'static str>>> {
    let report = check::run(doc, head, profile, &mut |_| {}, cancel)?;
    let open: Vec<&'static str> = report
        .results
        .iter()
        .filter(|r| r.status == Status::Failed || (r.status == Status::Manual && profile.pdfa().is_some()))
        // The metadata step itself settles its own group.
        .filter(|r| r.fix != Some(FixGroup::Metadata))
        .filter(|r| !(new_file && !r.findings.is_empty() && r.findings.iter().all(|f| f.new_file)))
        .map(|r| r.name)
        .collect();
    Ok(if open.is_empty() { Ok(()) } else { Err(open) })
}

fn metadata(doc: &mut PdfDocument, profile: Profile, new_file: bool, head: &[u8], cancel: &AtomicBool, log: &mut Vec<String>) -> Result<bool> {
    let mut cat = doc.catalog()?;
    let mut trailer = doc.trailer()?;
    let mut info = dict_of(&trailer, "Info");
    if profile == Profile::Pdfx4 {
        let mut i = match info.take() {
            Some(i) => i,
            None => {
                let i = doc.add_object(&doc.new_dict()?)?;
                trailer.dict_put("Info", i.clone())?;
                i
            }
        };
        let now = crate::annot::pdf_now();
        for k in ["CreationDate", "ModDate"] {
            if text(&i, k)?.is_none() {
                i.dict_put(k, PdfObject::new_string(&now)?)?;
            }
        }
        let trapped = i.get_dict("Trapped")?.is_some_and(|t| {
            let s = if t.is_name().unwrap_or(false) { t.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned()) } else { t.as_string().ok() };
            matches!(s.as_deref(), Some("True") | Some("False"))
        });
        if !trapped {
            i.dict_put("Trapped", PdfObject::new_name("False")?)?;
        }
        info = Some(i);
    }
    let props = check::info_props(info.as_ref());
    // The file says what it is only when it is: the identification is written after the file passes the
    // profile's other rules. Otherwise the XMP is only synced with the document information.
    let identified = match conforms_but_for_metadata(doc, profile, new_file, head, cancel)? {
        Ok(()) => true,
        Err(names) => {
            let shown: Vec<&str> = names.iter().copied().take(3).collect();
            let more = if names.len() > shown.len() { ", ..." } else { "" };
            log.push(format!(
                "Did not identify the file as {}: {} ({}{more})",
                profile.label(),
                plural(names.len(), "rule still fails or needs a check", "rules still fail or need a check"),
                shown.join(", ")
            ));
            false
        }
    };
    let ids = match (identified, profile.pdfa()) {
        (false, _) => Vec::new(),
        (true, Some((part, conf))) => vec![Ident::pdfa(part, conf)],
        (true, None) if profile == Profile::Pdfua1 => vec![Ident::pdfua(1)],
        (true, None) => vec![Ident::pdfx("PDF/X-4")],
    };
    let meta = cat.get_dict("Metadata")?.filter(|m| m.is_stream().unwrap_or(false));
    let xml = match &meta {
        Some(m) => {
            let old = String::from_utf8_lossy(&m.read_stream().unwrap_or_default()).into_owned();
            // A claim the check does not back is taken out, not kept: the file was changed here.
            let clear: &[&str] = match (identified, profile.family()) {
                (true, _) => &[],
                (false, Family::A1 | Family::A23) => &["pdfaid:part", "pdfaid:conformance", "pdfaid:amd", "pdfaid:rev"],
                (false, Family::Ua) => &["pdfuaid:part", "pdfuaid:rev"],
                (false, Family::X) => &["pdfxid:GTS_PDFXVersion", "pdfxid:GTS_PDFXConformance"],
            };
            xmp::merge(&old, &props, &ids, clear)
        }
        None => xmp::fresh(&props, &ids),
    };
    let buf = Buffer::from_bytes(xml.as_bytes())?;
    match meta {
        Some(mut m) => {
            // A stream with a filter becomes plain: other tools read the packet without decoding it.
            m.write_stream_buffer(&buf)?;
            m.dict_put("Type", PdfObject::new_name("Metadata")?)?;
            m.dict_put("Subtype", PdfObject::new_name("XML")?)?;
        }
        None => {
            let mut sd = doc.new_dict()?;
            sd.dict_put("Type", PdfObject::new_name("Metadata")?)?;
            sd.dict_put("Subtype", PdfObject::new_name("XML")?)?;
            let m = doc.add_stream(&buf, Some(&sd), false)?;
            cat.dict_put("Metadata", m)?;
        }
    }
    if identified {
        log.push(format!("Wrote XMP metadata that identifies the file as {} and matches the document information", profile.label()));
    } else {
        log.push("Wrote XMP metadata that matches the document information".into());
    }
    Ok(identified)
}
