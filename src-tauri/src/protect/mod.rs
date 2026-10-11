//! Password protection: what a Protect sheet asks for, what a file's security looks like, and the
//! write options that apply it.
//!
//! Encryption is a property of the *written* file, not of the open document: a protect step in the
//! edit core (`engine/edit/security.rs`) records a [`SecurityChange`], and the save that includes it
//! rewrites the file with these options (MuPDF's `pdf_write_options`: AES-256 revision 6, or
//! AES-128 for old readers). Nothing here keeps a password longer than the step that holds it.
//!
//! The permission and strength mapping follows Acrobat's Password Security dialog and pdfcraft's
//! `Protection` (crates/engine/src/lib.rs @ b8ddf71, MIT OR Apache-2.0).

use mupdf::pdf::{Encryption, PdfDocument, PdfWriteOptions, Permission};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zeroize::Zeroize;

pub mod app;
#[cfg(test)]
mod tests;

/// MuPDF keeps a password in a 128-byte buffer, terminator included.
pub const MAX_PASSWORD_BYTES: usize = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Printing {
    None,
    Low,
    #[default]
    High,
}

/// "Changes allowed" in Acrobat's dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Changes {
    None,
    /// Inserting, deleting and rotating pages.
    Pages,
    /// Filling in form fields and signing existing signature fields.
    Forms,
    /// Commenting, filling in form fields and signing.
    Comments,
    /// Any except extracting pages: everything but what copying controls, so no restriction.
    #[default]
    AllButExtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Cipher {
    /// Acrobat X and later (revision 6).
    #[default]
    Aes256,
    /// Acrobat 7 and later: for old readers.
    Aes128,
}

/// What the Protect sheet sends. Empty passwords count as none.
#[derive(Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtectSpec {
    /// Required to open the document (the user password).
    #[serde(default)]
    pub open_password: Option<String>,
    /// Required to change the security and lift the restrictions below (the owner password).
    #[serde(default)]
    pub permissions_password: Option<String>,
    #[serde(default)]
    pub printing: Printing,
    #[serde(default)]
    pub changes: Changes,
    #[serde(default = "yes")]
    pub copy: bool,
    #[serde(default = "yes")]
    pub accessibility: bool,
    #[serde(default)]
    pub cipher: Cipher,
}

fn yes() -> bool {
    true
}

impl std::fmt::Debug for ProtectSpec {
    // Never print a password: steps end up in logs and error messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectSpec")
            .field("open_password", &self.open_password.as_ref().map(|_| "…"))
            .field("permissions_password", &self.permissions_password.as_ref().map(|_| "…"))
            .field("printing", &self.printing)
            .field("changes", &self.changes)
            .field("copy", &self.copy)
            .field("accessibility", &self.accessibility)
            .field("cipher", &self.cipher)
            .finish()
    }
}

impl Drop for ProtectSpec {
    fn drop(&mut self) {
        self.open_password.zeroize();
        self.permissions_password.zeroize();
    }
}

/// Why a spec is refused. The UI matches on the code (`code()`), the text is for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    NothingToSet,
    SamePassword,
    TooLong,
    NeedsPermissionsPassword,
    PlainTextOnly,
    ControlCharacter,
}

impl SpecError {
    pub fn code(self) -> &'static str {
        match self {
            SpecError::NothingToSet => "nothing-to-set",
            SpecError::SamePassword => "same-password",
            SpecError::TooLong => "password-too-long",
            SpecError::NeedsPermissionsPassword => "needs-permissions-password",
            SpecError::PlainTextOnly => "plain-text-only",
            SpecError::ControlCharacter => "control-character",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            SpecError::NothingToSet => "Set a password to open the document, a permissions password, or both.",
            SpecError::SamePassword => "The open password and the permissions password must be different.",
            SpecError::TooLong => "A password can be at most 127 bytes long.",
            SpecError::NeedsPermissionsPassword => "Restrictions need a permissions password: without one anyone could lift them.",
            SpecError::PlainTextOnly => "AES-128 passwords can only use plain letters, digits and punctuation. Use AES-256 for other characters.",
            SpecError::ControlCharacter => "A password can't contain control characters.",
        }
    }
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

fn nonempty(p: &Option<String>) -> Option<&str> {
    p.as_deref().filter(|s| !s.is_empty())
}

impl ProtectSpec {
    pub fn open(&self) -> Option<&str> {
        nonempty(&self.open_password)
    }

    pub fn owner(&self) -> Option<&str> {
        nonempty(&self.permissions_password)
    }

    /// The rights this spec grants everyone who opens the file with the open password.
    pub fn rights(&self) -> Rights {
        let mut r = Rights {
            printing: self.printing,
            modify: false,
            annotate: false,
            forms: false,
            assemble: false,
            copy: self.copy,
            // Acrobat grants screen readers whenever copying is on (the box is forced).
            accessibility: self.accessibility || self.copy,
        };
        match self.changes {
            Changes::None => {}
            Changes::Pages => r.assemble = true,
            Changes::Forms => r.forms = true,
            Changes::Comments => (r.annotate, r.forms) = (true, true),
            Changes::AllButExtract => (r.modify, r.annotate, r.forms, r.assemble) = (true, true, true, true),
        }
        r
    }

    /// Puts both passwords in Unicode NFKC, the form SASLprep (the PDF 2.0 password rule) starts from:
    /// MuPDF hashes the bytes it is given, so a decomposed or compatibility form would not open in
    /// Acrobat or qpdf after the person types the composed one there. Call before [`Self::validate`].
    pub fn normalize(&mut self) {
        let nfkc = icu_normalizer::ComposingNormalizerBorrowed::new_nfkc();
        for p in [&mut self.open_password, &mut self.permissions_password] {
            if let Some(s) = p.as_mut().filter(|s| !nfkc.is_normalized(s)) {
                let mut n = nfkc.normalize(s).into_owned();
                std::mem::swap(s, &mut n);
                n.zeroize();
            }
        }
    }

    pub fn validate(&self) -> Result<(), SpecError> {
        let (open, owner) = (self.open(), self.owner());
        if open.is_none() && owner.is_none() {
            return Err(SpecError::NothingToSet);
        }
        if open.is_some() && open == owner {
            return Err(SpecError::SamePassword);
        }
        if [open, owner].into_iter().flatten().any(|p| p.len() > MAX_PASSWORD_BYTES) {
            return Err(SpecError::TooLong);
        }
        if [open, owner].into_iter().flatten().any(|p| p.chars().any(char::is_control)) {
            return Err(SpecError::ControlCharacter);
        }
        if self.cipher == Cipher::Aes128 && [open, owner].into_iter().flatten().any(|p| p.chars().any(|c| !(' '..='~').contains(&c))) {
            return Err(SpecError::PlainTextOnly);
        }
        // Restrictions without an owner password could be lifted by anyone: refuse rather than pretend.
        if owner.is_none() && !self.rights().unrestricted() {
            return Err(SpecError::NeedsPermissionsPassword);
        }
        Ok(())
    }
}

/// The permission bits of a file, decoded (PDF 32000-1 table 22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rights {
    pub printing: Printing,
    /// Bit 4: edit the content.
    pub modify: bool,
    /// Bit 6: add or change comments, fill forms.
    pub annotate: bool,
    /// Bit 9: fill in form fields.
    pub forms: bool,
    /// Bit 11: insert, delete and rotate pages.
    pub assemble: bool,
    /// Bit 5: copy text and graphics.
    pub copy: bool,
    /// Bit 10: text access for screen readers.
    pub accessibility: bool,
}

impl Rights {
    pub const ALL: Rights =
        Rights { printing: Printing::High, modify: true, annotate: true, forms: true, assemble: true, copy: true, accessibility: true };

    pub fn from_p(p: i32) -> Rights {
        let has = |f: Permission| p as u32 & f.bits() != 0;
        Rights {
            printing: match (has(Permission::PRINT), has(Permission::PRINT_HQ)) {
                (false, _) => Printing::None,
                (true, false) => Printing::Low,
                (true, true) => Printing::High,
            },
            modify: has(Permission::MODIFY),
            annotate: has(Permission::ANNOTATE),
            forms: has(Permission::FORM),
            assemble: has(Permission::ASSEMBLE),
            copy: has(Permission::COPY),
            accessibility: has(Permission::ACCESSIBILITY),
        }
    }

    pub fn unrestricted(&self) -> bool {
        *self == Rights::ALL
    }

    /// The owner lets content and pages leave the file: copying, and page changes (Acrobat counts
    /// editing as page changes too).
    pub fn may_extract(&self) -> bool {
        self.copy && (self.assemble || self.modify)
    }

    /// The bits MuPDF writes for these rights.
    pub fn permission(&self) -> Permission {
        let mut p = Permission::empty();
        match self.printing {
            Printing::None => {}
            Printing::Low => p |= Permission::PRINT,
            Printing::High => p |= Permission::PRINT | Permission::PRINT_HQ,
        }
        for (on, flag) in [
            (self.modify, Permission::MODIFY),
            (self.annotate, Permission::ANNOTATE),
            (self.forms, Permission::FORM),
            (self.assemble, Permission::ASSEMBLE),
            (self.copy, Permission::COPY),
            (self.accessibility, Permission::ACCESSIBILITY),
        ] {
            if on {
                p |= flag;
            }
        }
        p
    }

    /// The nearest "Changes allowed" choice (a file from another tool may combine bits freely).
    pub fn changes(&self) -> Changes {
        match (self.modify, self.annotate, self.forms, self.assemble) {
            (true, ..) => Changes::AllButExtract,
            (_, true, ..) => Changes::Comments,
            (_, _, true, _) => Changes::Forms,
            (_, _, _, true) => Changes::Pages,
            _ => Changes::None,
        }
    }
}

/// A security change recorded as an edit step, applied by the save that includes it.
#[derive(Debug, Clone, PartialEq)]
pub enum SecurityChange {
    Set(Arc<ProtectSpec>),
    Remove,
}

/// `base` plus the encryption of `change`: AES with the passwords and rights, or none at all.
pub fn write_options(mut base: PdfWriteOptions, change: &SecurityChange) -> PdfWriteOptions {
    match change {
        SecurityChange::Remove => {
            base.set_encryption(Encryption::None);
        }
        SecurityChange::Set(s) => {
            base.set_encryption(match s.cipher {
                Cipher::Aes256 => Encryption::Aes256,
                Cipher::Aes128 => Encryption::Aes128,
            });
            base.set_user_password(s.open().unwrap_or(""));
            // Without a permissions password the owner password is the open password (the spec's
            // default, and what Acrobat writes). An empty one would be hashed as such: qpdf, pikepdf
            // and pypdf then accept "" as the owner password and skip the open password altogether.
            base.set_owner_password(s.owner().or(s.open()).unwrap_or(""));
            base.set_permissions(s.rights().permission());
        }
    }
    base
}

/// Where a [`SecurityInfo`] comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// The file as it was opened.
    File,
    /// Set by an edit of this session.
    Set,
    /// Removed by an edit of this session.
    Removed,
}

/// The security of a document as Save would write it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityInfo {
    pub encrypted: bool,
    /// "AES-256", "AES-128", "RC4-128" or "RC4-40".
    pub method: Option<String>,
    /// The PDF encryption revision (6 for AES-256 from MuPDF or Acrobat X and later).
    pub revision: Option<i32>,
    /// A password is needed to open the file.
    pub open_password: bool,
    pub rights: Rights,
    /// `rights` is not everything: someone set a permissions password.
    pub restricted: bool,
    /// This session may change the security without a further password: the file has none, it was
    /// opened with its permissions (owner) password, or this session set the security.
    pub unlocked: bool,
    pub origin: Origin,
    /// `origin` is an edit that no save has written yet.
    pub unsaved: bool,
}

impl SecurityInfo {
    pub fn plain() -> SecurityInfo {
        SecurityInfo {
            encrypted: false,
            method: None,
            revision: None,
            open_password: false,
            rights: Rights::ALL,
            restricted: false,
            unlocked: true,
            origin: Origin::File,
            unsaved: false,
        }
    }

    /// What `change` makes of the file once written.
    pub fn of_change(change: &SecurityChange, unsaved: bool) -> SecurityInfo {
        match change {
            SecurityChange::Remove => SecurityInfo { origin: Origin::Removed, unsaved, ..SecurityInfo::plain() },
            SecurityChange::Set(s) => {
                let rights = s.rights();
                SecurityInfo {
                    encrypted: true,
                    method: Some(
                        match s.cipher {
                            Cipher::Aes256 => "AES-256",
                            Cipher::Aes128 => "AES-128",
                        }
                        .into(),
                    ),
                    revision: Some(match s.cipher {
                        Cipher::Aes256 => 6,
                        Cipher::Aes128 => 4,
                    }),
                    open_password: s.open().is_some(),
                    restricted: !rights.unrestricted(),
                    rights,
                    unlocked: true,
                    origin: Origin::Set,
                    unsaved,
                }
            }
        }
    }
}

/// The security a document carries, from its `Encrypt` dictionary. `password`: what the document was
/// opened with (`None`: it asked for none). This session is `unlocked` only when that password is the
/// file's owner (permissions) password: a file with an owner password of its own keeps it for the
/// security even when its P value allows everything.
pub fn read_security(doc: &PdfDocument, password: Option<&str>) -> crate::engine::Result<SecurityInfo> {
    let Some(enc) = doc.trailer()?.get_dict("Encrypt")? else {
        return Ok(SecurityInfo::plain());
    };
    if enc.is_null()? {
        return Ok(SecurityInfo::plain());
    }
    let int = |k: &str| -> crate::engine::Result<Option<i32>> {
        Ok(match enc.get_dict(k)? {
            Some(o) if o.is_int()? => Some(o.as_int()?),
            _ => None,
        })
    };
    let (v, r) = (int("V")?, int("R")?);
    let length = int("Length")?;
    // The stream crypt filter's method (V 4 and 5), by the name /StmF points at.
    let cfm = (|| -> crate::engine::Result<Option<String>> {
        let Some(cf) = enc.get_dict("CF")? else { return Ok(None) };
        let name = match enc.get_dict("StmF")? {
            Some(n) => String::from_utf8_lossy(&n.as_name()?).into_owned(),
            None => "StdCF".into(),
        };
        let Some(f) = cf.get_dict(name.as_str())? else { return Ok(None) };
        Ok(f.get_dict("CFM")?.map(|m| m.as_name()).transpose()?.map(|n| String::from_utf8_lossy(&n).into_owned()))
    })()?;
    let method = match (v, cfm.as_deref()) {
        (_, Some("AESV3")) | (Some(5), _) => "AES-256",
        (_, Some("AESV2")) => "AES-128",
        (Some(1), _) => "RC4-40",
        (Some(2), _) | (_, Some("V2")) => match length {
            Some(l) if l <= 40 => "RC4-40",
            _ => "RC4-128",
        },
        _ => "RC4-128",
    };
    let p = int("P")?.unwrap_or(-1);
    let rights = Rights::from_p(p);
    let restricted = !rights.unrestricted();
    // The access MuPDF granted this session: 4 is the owner password (an empty one only counts when
    // the file needs no open password, as in Acrobat).
    let owner = doc.authenticate_access(password.unwrap_or(""))? & 4 != 0;
    Ok(SecurityInfo {
        encrypted: true,
        method: Some(method.into()),
        revision: r,
        open_password: password.is_some(),
        rights,
        restricted,
        unlocked: owner,
        origin: Origin::File,
        unsaved: false,
    })
}
