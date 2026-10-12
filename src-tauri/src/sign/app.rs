//! Tauri commands for digital signatures: the signing identities (Windows certificate store, PFX
//! files opened this session, self-signed test IDs), signing a document, and its signature list.
//!
//! Identities from files live in memory for the session only; nothing is written to the
//! certificate store, and a password is used once to open the file and then dropped.

use super::crypto::{self, Certificate, DigitalId, SignError, Trust};
use super::pdf::{self, SignDate, SignList, SignRequest};
use crate::engine::edit::EditState;
use crate::engine::{DocId, Engine, EngineError};
use crate::history_ipc::History;
use crate::paths::Paths;
use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{State, Window};

type Result<T> = std::result::Result<T, EngineError>;

fn err(e: SignError) -> EngineError {
    match e {
        SignError::WrongPassword => EngineError::Invalid("the password is incorrect".into()),
        e => EngineError::Invalid(e.to_string()),
    }
}

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// Digital IDs opened from files (or created as test IDs) this session, by handle.
#[derive(Default)]
pub struct Ids {
    ids: Mutex<HashMap<u32, (Arc<DigitalId>, Source)>>,
    next: Mutex<u32>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// The Windows certificate store (the key stays in it; Windows may ask for a PIN).
    Store,
    /// A .pfx / .p12 file the user opened.
    File,
    /// A self-signed test ID Inkwell created for this session (memory only).
    Test,
}

impl Ids {
    fn add(&self, id: DigitalId, source: Source) -> Identity {
        let mut next = self.next.lock();
        *next += 1;
        let handle = *next;
        let dto = Identity::of(&id, source, Some(handle), None);
        self.ids.lock().insert(handle, (Arc::new(id), source));
        dto
    }
}

/// A signing identity as the ID picker shows it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub source: Source,
    /// Session IDs.
    pub handle: Option<u32>,
    /// Store IDs: SHA-256 of the certificate (hex).
    pub fingerprint: Option<String>,
    pub name: String,
    pub email: String,
    pub issuer: String,
    pub self_signed: bool,
    /// `YYYY-MM-DD`.
    pub not_after: String,
    /// Why it can't sign, when it can't.
    pub unusable: Option<String>,
}

impl Identity {
    fn of(id: &DigitalId, source: Source, handle: Option<u32>, fingerprint: Option<String>) -> Identity {
        let c = &id.cert;
        Identity {
            source,
            handle,
            fingerprint,
            name: id.friendly_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| c.display_name()),
            email: c.subject.email().unwrap_or_default().to_string(),
            issuer: c.issuer.display_name(),
            self_signed: c.is_self_signed(),
            not_after: day(&c.not_after),
            unusable: c.signing_problem(),
        }
    }
}

fn day(t: &crypto::Time) -> String {
    format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
}

/// Which identity signs.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum IdRef {
    Store { fingerprint: String },
    Session { handle: u32 },
}

/// The store's signing identities (read only) and this session's file and test IDs.
/// `INKWELL_NO_CERT_STORE=1` leaves the store out (test runs and screenshots never show a real ID).
#[tauri::command]
pub async fn sign_identities(ids: State<'_, Ids>) -> Result<Vec<Identity>> {
    let mut out: Vec<Identity> = {
        let ids = ids.ids.lock();
        let mut v: Vec<_> = ids.iter().map(|(h, (id, s))| Identity::of(id, *s, Some(*h), None)).collect();
        v.sort_by_key(|i| i.handle);
        v
    };
    if std::env::var("INKWELL_NO_CERT_STORE").as_deref() != Ok("1") {
        out.extend(blocking(store_identities).await?);
    }
    Ok(out)
}

#[cfg(windows)]
fn store_identities() -> Result<Vec<Identity>> {
    let list = crypto::winstore::list().map_err(err)?;
    Ok(list
        .into_iter()
        .filter(|s| s.has_key)
        .map(|s| Identity {
            source: Source::Store,
            handle: None,
            fingerprint: Some(s.fingerprint),
            name: s.display_name,
            email: String::new(),
            issuer: s.issuer,
            self_signed: false,
            not_after: s.not_after.as_ref().map(day).unwrap_or_default(),
            unusable: s.unusable,
        })
        .collect())
}

#[cfg(not(windows))]
fn store_identities() -> Result<Vec<Identity>> {
    Ok(Vec::new())
}

/// Opens a .pfx / .p12 (an open token from `pick_open`) with its password, for this session.
#[tauri::command]
pub async fn sign_open_id_file(window: Window, reg: State<'_, Paths>, ids: State<'_, Ids>, file: String, password: String) -> Result<Identity> {
    let path = reg.open(window.label(), &file)?;
    // Wiped when the closure is done with it.
    let password = zeroize::Zeroizing::new(password);
    let id = blocking(move || {
        let bytes = std::fs::read(&path).map_err(|e| EngineError::Io(e.to_string()))?;
        if bytes.len() > 1 << 20 {
            return Err(EngineError::Invalid("that file is too large to be a digital ID".into()));
        }
        crypto::pkcs12::open(&bytes, &password).map_err(err)
    })
    .await?;
    Ok(ids.add(id, Source::File))
}

/// Creates a self-signed test ID (RSA 2048, 5 years) for this session. It lives in memory only:
/// nothing is written to disk, the certificate store or the user's folders.
#[tauri::command]
pub async fn sign_create_test_id(ids: State<'_, Ids>, name: String, email: String, org: String) -> Result<Identity> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 64 || email.trim().is_empty() || email.len() > 128 || org.len() > 64 {
        return Err(EngineError::Invalid("a test ID needs a name and an email".into()));
    }
    let id = blocking(move || DigitalId::create_self_signed(&name, email.trim(), org.trim(), 5).map_err(err)).await?;
    Ok(ids.add(id, Source::Test))
}

/// Signing options from the UI. `rect` is in page space (points, y down).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SignOptions {
    pub field: Option<String>,
    #[serde(default)]
    pub page: u32,
    pub rect: Option<[f32; 4]>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub certify: Option<u8>,
    /// A PNG or JPEG for the appearance, base64 (no data: prefix).
    pub image: Option<String>,
    /// The user's time zone, minutes east of UTC.
    #[serde(default)]
    pub offset_min: i32,
}

impl SignOptions {
    fn request(&self) -> Result<SignRequest> {
        let image = match &self.image {
            Some(b) if b.len() > 4 << 20 => return Err(EngineError::Invalid("the signature picture is too large".into())),
            Some(b) => Some(base64::engine::general_purpose::STANDARD.decode(b).map_err(|_| EngineError::Invalid("the signature picture is not valid".into()))?),
            None => None,
        };
        let clip = |s: &Option<String>| s.as_ref().map(|s| s.chars().take(200).collect::<String>());
        Ok(SignRequest {
            field: self.field.clone(),
            page: self.page,
            rect: self.rect,
            reason: clip(&self.reason),
            location: clip(&self.location),
            contact: None,
            certify: self.certify.map(|p| p.clamp(1, 3)),
            image,
            date: Some(SignDate::now(self.offset_min)),
        })
    }
}

fn identity(ids: &Ids, which: &IdRef) -> Result<Arc<DigitalId>> {
    match which {
        IdRef::Session { handle } => ids.ids.lock().get(handle).map(|(id, _)| id.clone()).ok_or_else(|| EngineError::Invalid("that digital ID is no longer open".into())),
        #[cfg(windows)]
        IdRef::Store { fingerprint } => crypto::winstore::open_identity(fingerprint).map(Arc::new).map_err(err),
        #[cfg(not(windows))]
        IdRef::Store { .. } => Err(EngineError::Invalid("no certificate store on this system".into())),
    }
}

/// Signs `doc` (one undo step: an incremental update appended to what Save writes) and saves
/// it: over the file, or to `to` (a save token). Pending page deletions or redactions are
/// written out first, as Save would.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn sign_document(
    window: Window,
    app: tauri::AppHandle,
    eng: State<'_, Arc<Engine>>,
    reg: State<'_, Paths>,
    hist: State<'_, Arc<History>>,
    ids: State<'_, Ids>,
    doc: DocId,
    id: IdRef,
    options: SignOptions,
    to: Option<String>,
) -> Result<EditState> {
    let to = to.map(|t| reg.take_save(window.label(), &t)).transpose()?;
    let req = options.request()?;
    let (eng, hist) = (eng.inner().clone(), hist.inner().clone());
    let ids_ref = identity(&ids, &id);
    blocking(move || {
        let signer = ids_ref?;
        let label = format!("Sign as {}", signer.cert.display_name());
        sign_step(&eng, doc, &signer, &req, label)?;
        crate::save_and_record(&window, app, &eng, &hist, doc, to)
    })
    .await
}

/// The signing step itself, without the save (tests call it).
pub fn sign_step(eng: &Engine, doc: DocId, id: &DigitalId, req: &SignRequest, label: String) -> Result<EditState> {
    eng.apply_signature(doc, label, |base| pdf::sign(base, id, req).map_err(err))
}

/// Every signature field with its validation, against what Save would write now.
#[tauri::command]
pub async fn sign_list(eng: State<'_, Arc<Engine>>, doc: DocId) -> Result<SignList> {
    let eng = eng.inner().clone();
    blocking(move || list(&eng, doc)).await
}

pub fn list(eng: &Engine, doc: DocId) -> Result<SignList> {
    // Most documents have no form: skip copying and reparsing them after every edit.
    let (has_fields, certify) = eng.read_custom(doc, |d| {
        let fields = d.catalog()?.get_dict("AcroForm")?.and_then(|f| f.get_dict("Fields").ok().flatten());
        let has = match fields {
            Some(f) => f.len()? > 0,
            None => false,
        };
        Ok((has, pdf::docmdp_level(d)))
    })?;
    if !has_fields {
        return Ok(SignList { certify, signatures: Vec::new(), error: None });
    }
    let bytes = eng.current_bytes(doc)?;
    let d = mupdf::pdf::PdfDocument::from_bytes(&bytes).map_err(EngineError::from)?;
    // Signatures that can't be checked (too many) still leave the level signing refuses by, and say why they
    // weren't checked: the UI must not read that as an unsigned document.
    Ok(pdf::list(&d, &bytes, &system_trust).unwrap_or_else(|e| SignList { certify, signatures: Vec::new(), error: Some(e.to_string()) }))
}

/// Windows' chain engine and trusted roots, at the current time.
pub fn system_trust(cert: &Certificate, others: &[Certificate]) -> (bool, String) {
    let extra: Vec<&[u8]> = others.iter().map(|c| c.raw.as_slice()).collect();
    match crypto::trust::check(&cert.raw, &extra, None) {
        Trust::Trusted => (true, String::new()),
        Trust::UntrustedRoot if cert.is_self_signed() => (false, "the certificate is self-signed and not in your trusted list".into()),
        Trust::UntrustedRoot => (false, "the certificate's issuer is not trusted on this PC".into()),
        Trust::Expired => (false, "the certificate has expired".into()),
        Trust::NotYetValid => (false, "the certificate is not valid yet".into()),
        Trust::Revoked => (false, "the certificate was revoked".into()),
        Trust::Other(why) => (false, why),
    }
}
