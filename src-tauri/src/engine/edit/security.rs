//! Password security as edit steps: set (or change) it, remove it, and read what a document has.
//!
//! A security step does not change the document, only how the next save writes it: see
//! `Step::security` and `Editor::save_opts`. It is one undo step, listed in the history, and
//! saved by the normal Save (or Save as, which is how the original is left untouched).

use super::{Document, EditSource, EditState, Editor, PdfDocument, Saved, Step, invalid, now_ms};
use crate::engine::{DocEntry, DocId, Engine, Result, authenticate};
use crate::protect::{self, ProtectSpec, SecurityChange, SecurityInfo};
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
        protect::read_security(&self.doc.doc, entry.password.lock().as_ref().map(|p| p.as_str()))
    }

    /// The session may change the security: the file has none, the permissions password was given to
    /// open it, or `password` is the permissions password.
    fn ensure_unlocked(&self, entry: &DocEntry, password: Option<&str>) -> Result<()> {
        if self.security_info(entry)?.unlocked {
            return Ok(());
        }
        let pw = password.filter(|p| !p.is_empty()).ok_or_else(|| invalid(NEEDS_OWNER))?;
        // A fresh copy of the snapshot, so the failed attempts leave the authoritative document alone.
        let snap = self.step().snap.clone();
        // SAFETY: snapshots are never mutated, and `snap` outlives the document below.
        let d = unsafe { Document::from_shared_bytes(&snap.bytes[..snap.len], "application/pdf")? };
        let pdf = PdfDocument::try_from(d)?;
        // 4 is the owner password; the open password alone (2) leaves the restrictions in force, and
        // so does it for a file that restricts nothing: the owner password is still the owner's.
        // As typed first, then in NFKC (what Protect writes; MuPDF does no SASLprep).
        if pdf.authenticate_access(pw)? & 4 != 0 {
            return Ok(());
        }
        match protect::nfkc_form(pw) {
            Some(n) if pdf.authenticate_access(&n)? & 4 != 0 => Ok(()),
            _ => Err(invalid(BAD_OWNER)),
        }
    }

    /// Pages or content may leave this document as an unprotected file only when its owner allows
    /// copying and page changes, or this session holds the permissions password: extract, split
    /// and export write plain files, which would otherwise lift the restrictions silently.
    pub(super) fn ensure_extractable(&self, entry: &DocEntry) -> Result<()> {
        let info = self.security_info(entry)?;
        if info.encrypted && !info.unlocked && !info.rights.may_extract() {
            return Err(invalid(NEEDS_OWNER));
        }
        Ok(())
    }

    /// Setting or removing password security makes the save a rewrite of every object: neither a certification nor
    /// a signature survives it.
    fn ensure_signatures_allow_security(&self) -> Result<()> {
        // A signed file that was repaired on open: its signature cannot be told from the repaired copy, so the
        // rewrite would go over the original as an unsigned-looking file.
        self.refuse_if_repaired_signed()?;
        crate::outline::refuse_if_certified(&self.doc.doc, "Changing its password security")?;
        if crate::outline::signed(&self.doc.doc) {
            return Err(invalid(
                "This document is signed. Changing its password security rewrites the whole file and would break the signature, so Inkwell leaves it as it is.",
            ));
        }
        Ok(())
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
        protect::read_security(&PdfDocument::try_from(d)?, entry.password.lock().as_ref().map(|p| p.as_str()))
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
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        self.protect_locked(&entry, &mut lane, spec, permissions_password, source)
    }

    /// [`Engine::protect`] with the edit lane held by the caller.
    fn protect_locked(
        &self,
        entry: &DocEntry,
        lane: &mut Option<Editor>,
        spec: ProtectSpec,
        permissions_password: Option<&str>,
        source: EditSource,
    ) -> Result<EditState> {
        let mut spec = spec;
        spec.normalize();
        spec.validate().map_err(|e| invalid(e.code()))?;
        let ed = self.lane(entry, lane)?;
        ed.ensure_signatures_allow_security()?;
        ed.ensure_unlocked(entry, permissions_password)?;
        let had = ed.security_info(entry)?.encrypted;
        let cipher = match spec.cipher {
            protect::Cipher::Aes256 => "AES-256",
            protect::Cipher::Aes128 => "AES-128",
        };
        let label = if had { format!("Change password security ({cipher})") } else { format!("Protect with a password ({cipher})") };
        ed.push_security(label, source, SecurityChange::Set(Arc::new(spec)));
        Ok(ed.state(entry))
    }

    /// [`Editor::ensure_extractable`] for a document that is not being edited right now.
    pub fn ensure_extractable(&self, id: DocId) -> Result<()> {
        let info = self.security_info(id)?;
        if info.encrypted && !info.unlocked && !info.rights.may_extract() {
            return Err(invalid(NEEDS_OWNER));
        }
        Ok(())
    }

    /// [`Engine::protect`] and a save (over the file, or to `to`) as one operation: when the save
    /// fails the protect step is taken back, so a later Save cannot write the protection nobody
    /// asked for (in place, without the confirm) and a retry does not stack a second step.
    pub fn protect_and_save(
        &self,
        id: DocId,
        spec: ProtectSpec,
        permissions_password: Option<&str>,
        to: Option<&str>,
    ) -> Result<(EditState, Option<Saved>)> {
        let entry = self.entry(id)?;
        // One guard for the step and the save: an edit, a save or an assistant turn cannot slip in
        // between, so the rollback below always takes back exactly this step.
        let mut lane = entry.editor.lock();
        let (redo, saved) = {
            let ed = self.lane(&entry, &mut lane)?;
            (ed.steps[ed.cur + 1..].to_vec(), ed.saved)
        };
        self.protect_locked(&entry, &mut lane, spec, permissions_password, EditSource::User)?;
        self.save_locked(id, &entry, &mut lane, to).inspect_err(|_| {
            // The step goes and nothing else does: what Redo had is back (the push dropped it).
            if let Some(ed) = lane.as_mut() {
                ed.steps.truncate(ed.cur);
                ed.cur -= 1;
                ed.steps.extend(redo);
                ed.saved = saved;
            }
        })
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
        ed.ensure_signatures_allow_security()?;
        ed.ensure_unlocked(&entry, permissions_password)?;
        ed.push_security("Remove password security".into(), source, SecurityChange::Remove);
        Ok(ed.state(&entry))
    }
}
