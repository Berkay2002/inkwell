//! What a document's signatures forbid. One reading of a PDF's signature structure, shared by the
//! tools that must not break a signature (OCR, the assistant's action tools):
//!
//! - **signed**: a signature field with a value (`/FT /Sig` with `/V`), a certification
//!   (`/Perms /DocMDP`), or the AcroForm `SigFlags` bit 1. Any content change after it makes the
//!   signature report "modified"; a change that needs a full rewrite makes it invalid.
//! - **certified** (`/DocMDP` `/P`): 1 allows no change, 2 form filling and signing, 3 also
//!   annotations. Without a certification a later change is allowed and only reported.
//! - **locks** (`/FieldMDP`, a signature field's `/Lock` or its `/Reference`): which form fields
//!   the signature froze (`All`, `Include` the listed ones, `Exclude` all but the listed ones).
//! - **rewrite**: the file cannot be saved as an incremental update (it was repaired on open, or
//!   redacted), so saving it would rewrite it and destroy a signature.

use mupdf::pdf::{PdfDocument, PdfObject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockAction {
    All,
    Include,
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub action: LockAction,
    /// Fully qualified field names.
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signatures {
    pub signed: bool,
    /// `/DocMDP` `/P` (1, 2 or 3) when the document is certified.
    pub certified: Option<u8>,
    pub locks: Vec<Lock>,
    /// Saving would have to rewrite the file.
    pub rewrite: bool,
}

impl Signatures {
    /// A signature froze the form field with this fully qualified name.
    pub fn locks_field(&self, name: &str) -> bool {
        self.locks.iter().any(|l| match l.action {
            LockAction::All => true,
            LockAction::Include => l.fields.iter().any(|f| f == name),
            LockAction::Exclude => !l.fields.iter().any(|f| f == name),
        })
    }
}

const MAX_NODES: usize = 20_000;
const MAX_DEPTH: usize = 32;

fn name_of(o: &PdfObject, key: &str) -> Option<String> {
    let v = o.get_dict(key).ok().flatten()?;
    String::from_utf8(v.as_name().ok()?).ok()
}

fn has_value(o: &PdfObject, key: &str) -> bool {
    o.get_dict(key).ok().flatten().is_some_and(|v| v.is_null().ok() == Some(false))
}

fn strings(arr: &PdfObject) -> Vec<String> {
    let n = arr.len().unwrap_or(0).min(10_000);
    (0..n).filter_map(|i| arr.get_array(i as i32).ok().flatten()?.as_string().ok()).collect()
}

/// `{ /Action /All|/Include|/Exclude /Fields [...] }`, from a `/Lock` or a `/TransformParams`.
fn lock_of(params: &PdfObject) -> Option<Lock> {
    let action = match name_of(params, "Action")?.as_str() {
        "All" => LockAction::All,
        "Include" => LockAction::Include,
        "Exclude" => LockAction::Exclude,
        _ => return None,
    };
    let fields = params.get_dict("Fields").ok().flatten().map(|f| strings(&f)).unwrap_or_default();
    Some(Lock { action, fields })
}

/// The `/Reference` transforms of a signature dictionary: (`DocMDP` P, field lock).
fn transforms(sig: &PdfObject) -> (Option<u8>, Option<Lock>) {
    let (mut p, mut lock) = (None, None);
    let Some(refs) = sig.get_dict("Reference").ok().flatten() else { return (p, lock) };
    for i in 0..refs.len().unwrap_or(0).min(16) {
        let Some(r) = refs.get_array(i as i32).ok().flatten() else { continue };
        let params = r.get_dict("TransformParams").ok().flatten();
        match name_of(&r, "TransformMethod").as_deref() {
            Some("DocMDP") => {
                let v = params.as_ref().and_then(|t| t.get_dict("P").ok().flatten()).and_then(|v| v.as_int().ok()).unwrap_or(2);
                p = Some(v.clamp(1, 3) as u8);
            }
            Some("FieldMDP") => lock = lock.or_else(|| params.as_ref().and_then(lock_of)),
            _ => {}
        }
    }
    (p, lock)
}

pub fn inspect(doc: &PdfDocument) -> Signatures {
    let mut s = Signatures::default();
    let Ok(cat) = doc.catalog() else { return s };
    let form = cat.get_dict("AcroForm").ok().flatten();
    if form.as_ref().and_then(|f| f.get_dict("SigFlags").ok().flatten()).and_then(|v| v.as_int().ok()).is_some_and(|f| f & 1 != 0) {
        s.signed = true;
    }
    // a certification: the catalog's /Perms /DocMDP is the certifying signature
    if let Some(sig) = cat.get_dict("Perms").ok().flatten().and_then(|p| p.get_dict("DocMDP").ok().flatten()) {
        if sig.is_dict().unwrap_or(false) {
            s.signed = true;
            s.certified = Some(transforms(&sig).0.unwrap_or(2));
        }
    }
    // signed signature fields and what they locked
    if let Some(fields) = form.and_then(|f| f.get_dict("Fields").ok().flatten()) {
        let mut stack: Vec<(PdfObject, String, Option<String>, usize)> = Vec::new();
        for i in 0..fields.len().unwrap_or(0).min(MAX_NODES) {
            if let Some(f) = fields.get_array(i as i32).ok().flatten() {
                stack.push((f, String::new(), None, 0));
            }
        }
        let mut seen = 0;
        while let Some((node, prefix, ft, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_NODES || depth > MAX_DEPTH {
                break;
            }
            let ft = name_of(&node, "FT").or(ft);
            let name = match node.get_dict("T").ok().flatten().and_then(|t| t.as_string().ok()) {
                Some(t) if prefix.is_empty() => t,
                Some(t) => format!("{prefix}.{t}"),
                None => prefix.clone(),
            };
            if ft.as_deref() == Some("Sig") && has_value(&node, "V") {
                s.signed = true;
                if let Some(v) = node.get_dict("V").ok().flatten() {
                    let (p, lock) = transforms(&v);
                    if let Some(p) = p {
                        s.certified = Some(s.certified.map_or(p, |c| c.min(p)));
                    }
                    s.locks.extend(lock);
                }
                if let Some(l) = node.get_dict("Lock").ok().flatten().as_ref().and_then(lock_of) {
                    s.locks.push(l);
                }
            }
            if let Some(kids) = node.get_dict("Kids").ok().flatten() {
                for i in 0..kids.len().unwrap_or(0).min(MAX_NODES) {
                    if let Some(k) = kids.get_array(i as i32).ok().flatten() {
                        stack.push((k, name.clone(), ft.clone(), depth + 1));
                    }
                }
            }
        }
    }
    s.rewrite = s.signed && !doc.can_be_saved_incrementally();
    s
}

/// The simple question: does the document carry a signature?
pub fn is_signed(doc: &PdfDocument) -> bool {
    inspect(doc).signed
}
