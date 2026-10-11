//! Password security as edit steps: set (or change) it, remove it, and read what a document has.
//!
//! A security step does not change the document, only how the next save writes it: see
//! `Step::security` and `Editor::save_opts`. It is one undo step, listed in the history, and
//! saved by the normal Save (or Save as, which is how the original is left untouched).

use super::{Document, EditSource, EditState, Editor, PdfDocument, Step, invalid, now_ms};
use crate::engine::{DocEntry, DocId, Engine, Result, authenticate};
use crate::protect::{self, ProtectSpec, Rights, SecurityChange, SecurityInfo};
use std::sync::Arc;

/// The UI matches on these messages (`FRIENDLY` in the stores).
const NEEDS_OWNER: &str = "needs-permissions-password";
const BAD_OWNER: &str = "bad-permissions-password";

impl Editor {
    /// Security as Save would write it: the latest security step, else what the file carries.
    fn security_info(&self, entry: &DocEntry) -> Result<SecurityInfo> {
        let step = self.steps[..=self.cur].iter().enumerate().rev().find(|(_, s)| s.security.is_some());
        if let Some((i, s)) = step {
            let written = self.saved.is_some_and(|w| w >= i);
            return Ok(SecurityInfo::of_change(s.security.as_ref().expect("found by security"), !written));
        }
        protect::read_security(&self.doc.doc, entry.password.lock().is_some())
    }

    /// The session may change the security: no restrictions, the permissions password was given to
    /// open it, or `password` is the permissions password.
    fn ensure_unlocked(&self, entry: &DocEntry, password: Option<&str>) -> Result<()> {
        if self.security_info(entry)?.unlocked {
            return Ok(());
        }
        let pw = password.filter(|p| !p.is_empty()).ok_or_else(|| invalid(NEEDS_OWNER))?;
        // A fresh copy of the snapshot, so the failed attempts leave the authoritative document alone.
        let snap = self.step().snap.clone();
        // SAFETY: snapshots are never mutated, and `snap` outlives the document below.
        let mut d = unsafe { Document::from_shared_bytes(&snap.bytes[..snap.len], "application/pdf")? };
        if !d.authenticate(pw)? {
            return Err(invalid(BAD_OWNER));
        }
        let pdf = PdfDocument::try_from(d)?;
        if Rights::from_p(pdf.permissions().bits() as i32).unrestricted() {
            Ok(())
        } else {
            // Authenticated, but as the open password: the restrictions still hold.
            Err(invalid(BAD_OWNER))
        }
    }

    fn push_security(&mut self, label: String, source: EditSource, change: SecurityChange) {
        let cur = self.step().clone();
        self.push(Step { label, source, at_ms: now_ms(), pages: vec![], drops: false, redacts: false, security: Some(change), exact: false, ..cur });
    }
}

impl Engine {
    /// The password security of the document as Save would write it. Read-only: the file as opened
    /// is read without creating the edit lane.
    pub fn security_info(&self, id: DocId) -> Result<SecurityInfo> {
        let entry = self.entry(id)?;
        let lane = entry.editor.lock();
        if let Some(ed) = lane.as_ref() {
            return ed.security_info(&entry);
        }
        let _ = mupdf::Context::get();
        let mut d = Document::open(entry.path().as_str())?;
        authenticate(&mut d, &entry)?;
        protect::read_security(&PdfDocument::try_from(d)?, entry.password.lock().is_some())
    }

    /// Protects the document with `spec` (one undo step; the next save writes it). Changing the
    /// security of a file that has a permissions password needs it: `permissions_password`.
    pub fn protect(
        &self,
        id: DocId,
        spec: ProtectSpec,
        permissions_password: Option<&str>,
        source: EditSource,
    ) -> Result<EditState> {
        spec.validate().map_err(|e| invalid(e.code()))?;
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        ed.ensure_unlocked(&entry, permissions_password)?;
        let had = ed.security_info(&entry)?.encrypted;
        let cipher = match spec.cipher {
            protect::Cipher::Aes256 => "AES-256",
            protect::Cipher::Aes128 => "AES-128",
        };
        let label = if had { format!("Change password security ({cipher})") } else { format!("Protect with a password ({cipher})") };
        ed.push_security(label, source, SecurityChange::Set(Arc::new(spec)));
        Ok(ed.state(&entry))
    }

    /// Removes password security (one undo step). A document with restrictions needs its
    /// permissions password, unless it was opened with it.
    pub fn remove_security(&self, id: DocId, permissions_password: Option<&str>, source: EditSource) -> Result<EditState> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        if !ed.security_info(&entry)?.encrypted {
            return Err(invalid("This document has no password security."));
        }
        ed.ensure_unlocked(&entry, permissions_password)?;
        ed.push_security("Remove password security".into(), source, SecurityChange::Remove);
        Ok(ed.state(&entry))
    }
}
