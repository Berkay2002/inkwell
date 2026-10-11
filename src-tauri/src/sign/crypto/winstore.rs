//! Signing identities in the Windows Current User Personal ("My") certificate store. The private
//! key never leaves the store: signing goes through CNG (NCrypt), acquired without the silent flag
//! so a smart card or token can ask for its PIN. Inspired by pdfcraft's `windows.rs` (MIT OR
//! Apache-2.0), reimplemented over windows-sys.
//!
//! The store is opened read-only and nothing is ever added to it. [`import_pfx_ephemeral`] loads a
//! `.pfx` into a throw-away in-memory store with a non-persisted CNG key, which is how the signing
//! path is exercised without a real store.

use std::ptr::null;
use std::sync::Arc;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::Cryptography::*;
use windows_sys::core::{PCWSTR, w};
use zeroize::Zeroize;

use super::der::Time;
use super::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey};
use super::trust::win::{Cert, Chain, Store};
use super::{Certificate, DigitalId, SignError};

/// A certificate in the Personal store, and whether Inkwell can sign with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreIdentity {
    /// The subject's common name (else organization, else the DN).
    pub display_name: String,
    /// The issuer's display name.
    pub issuer: String,
    pub not_after: Option<Time>,
    /// SHA-256 of the certificate as lower-case hex: the stable handle for [`open_identity`].
    pub fingerprint: String,
    /// The store holds a private key reference for it.
    pub has_key: bool,
    /// Why it can't sign, if it can't.
    pub unusable: Option<String>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fingerprint(cert: &Cert) -> String {
    hex(&DigestAlg::Sha256.digest(&[cert.der()]))
}

/// How the entry looks in a list, and its parsed certificate when it parses.
fn describe(cert: &Cert) -> (StoreIdentity, Option<Certificate>) {
    let has_key = cert.has_key();
    let mut id = StoreIdentity {
        display_name: "(unreadable certificate)".into(),
        issuer: String::new(),
        not_after: None,
        fingerprint: fingerprint(cert),
        has_key,
        unusable: None,
    };
    let parsed = match Certificate::parse(cert.der()) {
        Ok(c) => c,
        Err(e) => {
            id.unusable = Some(format!("the certificate can't be read: {e}"));
            return (id, None);
        }
    };
    id.display_name = parsed.display_name();
    id.issuer = parsed.issuer.display_name();
    id.not_after = Some(parsed.not_after);
    id.unusable = if !has_key {
        Some("it has no private key in the store".into())
    } else if parsed.public_key.store_signing_key().is_none() {
        Some(format!("its {} key can't sign through Inkwell (RSA, P-256 and P-384 only)", parsed.public_key.describe()))
    } else {
        parsed.signing_problem()
    };
    (id, Some(parsed))
}

/// Current User > Personal, opened read-only.
fn open_personal() -> Result<Store, SignError> {
    // SAFETY: the provider name is a NUL-terminated wide string that outlives the call.
    let h = unsafe {
        CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            0,
            0,
            CERT_SYSTEM_STORE_CURRENT_USER | CERT_STORE_READONLY_FLAG | CERT_STORE_OPEN_EXISTING_FLAG,
            w!("My") as *const core::ffi::c_void,
        )
    };
    if h.is_null() {
        return Err(SignError::Crypto(format!("the Windows Current User Personal store couldn't be opened (Windows error 0x{:08X})", unsafe {
            GetLastError()
        })));
    }
    Ok(Store(h))
}

/// The certificates in Current User > Personal with whether each can sign. Only certificate
/// data is read; no key is opened.
pub fn list() -> Result<Vec<StoreIdentity>, SignError> {
    Ok(open_personal()?.certs().iter().map(|c| describe(c).0).collect())
}

/// The identity with this SHA-256 `fingerprint` (as [`StoreIdentity::fingerprint`]) from the
/// Personal store. Its key signs through CNG; `chain` holds the issuers Windows finds.
pub fn open_identity(fingerprint_hex: &str) -> Result<DigitalId, SignError> {
    let store = open_personal()?;
    let wanted = fingerprint_hex.trim().to_ascii_lowercase();
    let cert = store
        .certs()
        .into_iter()
        .find(|c| fingerprint(c) == wanted)
        .ok_or_else(|| SignError::Crypto("that certificate is no longer in the Windows certificate store".into()))?;
    let chain = super::trust::win::chain(&cert, None, None).map(|c: Chain| c.certs_der()).unwrap_or_default();
    let others: Vec<Vec<u8>> = chain.into_iter().filter(|d| d != cert.der()).collect();
    identity(cert, others)
}

/// A [`DigitalId`] whose key signs through the certificate context. `others` are DER issuers.
fn identity(cert: Cert, others: Vec<Vec<u8>>) -> Result<DigitalId, SignError> {
    let (info, parsed) = describe(&cert);
    if let Some(why) = info.unusable {
        return Err(SignError::Unsupported(format!("{}: {why}", info.display_name)));
    }
    let parsed = parsed.ok_or_else(|| SignError::Malformed("certificate".into()))?;
    let public = parsed.public_key.clone();
    let key = PrivateKey::external(public.clone(), Arc::new(StoreKey { cert, public }));
    let chain = others.iter().filter_map(|d| Certificate::parse(d).ok()).collect();
    Ok(DigitalId { key, cert: parsed, chain, friendly_name: Some(info.display_name) })
}

/// Load a `.pfx` into an in-memory store (key not persisted, always a CNG key) and return the
/// identity with a private key. Nothing is written to any system store or key container.
pub fn import_pfx_ephemeral(bytes: &[u8], password: &str) -> Result<DigitalId, SignError> {
    let blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| SignError::Malformed("PFX too large".into()))?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut pw: Vec<u16> = password.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `blob` points at `bytes` (read only by the call) and `pw` is NUL terminated.
    let h = unsafe { PFXImportCertStore(&blob, pw.as_ptr() as PCWSTR, PKCS12_NO_PERSIST_KEY | PKCS12_ALWAYS_CNG_KSP) };
    pw.zeroize();
    if h.is_null() {
        // SAFETY: reads the calling thread's last error.
        return Err(match unsafe { GetLastError() } {
            86 => SignError::WrongPassword,
            code => SignError::Crypto(format!("Windows could not import the PFX (Windows error 0x{code:08X})")),
        });
    }
    let store = Store(h);
    let (mut signer, mut others) = (None, Vec::new());
    for c in store.certs() {
        if signer.is_none() && c.has_key() {
            signer = Some(c);
        } else {
            others.push(c.der().to_vec());
        }
    }
    let signer = signer.ok_or_else(|| SignError::Malformed("the PFX holds no certificate with a private key".into()))?;
    identity(signer, others)
}

/// A key held by Windows, used through the certificate it belongs to.
struct StoreKey {
    cert: Cert,
    public: PublicKey,
}

/// The key handle `CryptAcquireCertificatePrivateKey` returned, freed when it says the caller owns it.
struct KeyHandle {
    handle: usize,
    owned: bool,
}

impl Drop for KeyHandle {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: a CNG key handle this value owns, freed once.
            unsafe { NCryptFreeObject(self.handle) };
        }
    }
}

impl ExternalKey for StoreKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let digest = alg.digest(&[msg]);
        let (mut handle, mut spec, mut owned) = (0usize, 0u32, 0i32);
        // Not silent: a smart card or token that needs a PIN or touch can show its own prompt.
        // SAFETY: the context is live; the out parameters are valid for the call.
        let ok = unsafe { CryptAcquireCertificatePrivateKey(self.cert.0, CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG, null(), &mut handle, &mut spec, &mut owned) };
        if ok == 0 {
            // SAFETY: reads the calling thread's last error.
            let code = unsafe { GetLastError() };
            return Err(SignError::Crypto(format!(
                "the Windows certificate store key couldn't be opened for signing (is the smart card or token connected?): {}",
                explain(code)
            )));
        }
        let key = KeyHandle { handle, owned: owned != 0 };
        // CNG adds the DigestInfo for PKCS #1 v1.5 when given the hash algorithm name; ECDSA takes none.
        let rsa = matches!(self.public, PublicKey::Rsa { .. });
        let name: PCWSTR = match alg {
            DigestAlg::Sha384 => w!("SHA384"),
            DigestAlg::Sha512 => w!("SHA512"),
            _ => w!("SHA256"),
        };
        let padding = BCRYPT_PKCS1_PADDING_INFO { pszAlgId: name };
        let (pad_ptr, flags) = if rsa { (&padding as *const _ as *const core::ffi::c_void, BCRYPT_PAD_PKCS1) } else { (null(), 0) };
        let len = u32::try_from(digest.len()).map_err(|_| SignError::Crypto("digest too long".into()))?;
        let mut size = 0u32;
        // SAFETY: a size query, then the signing call into a buffer of the size CNG reported.
        let status = unsafe { NCryptSignHash(key.handle, pad_ptr, digest.as_ptr(), len, std::ptr::null_mut(), 0, &mut size, flags) };
        if status != 0 {
            return Err(sign_failed(status));
        }
        let mut sig = vec![0u8; size as usize];
        // SAFETY: as above; `sig` has `size` bytes.
        let status = unsafe { NCryptSignHash(key.handle, pad_ptr, digest.as_ptr(), len, sig.as_mut_ptr(), size, &mut size, flags) };
        if status != 0 {
            return Err(sign_failed(status));
        }
        sig.truncate(size as usize);
        self.public.store_signature_der(&sig)
    }
}

fn sign_failed(status: i32) -> SignError {
    SignError::Crypto(format!("the Windows certificate store didn't sign (key use may have been cancelled): {}", explain(status as u32)))
}

/// A Windows error code with what it usually means for a key.
fn explain(code: u32) -> String {
    let meaning = match code {
        0x8009_000D | 0x8009_0016 | 0x8009_0011 => "the key is missing from its key store (deleted, or imported under another Windows account)",
        0x8009_0010 => "access to the key was denied",
        0x8009_0013 | 0x8009_0014 | 0x8009_001E => "its key provider isn't available to CNG (a legacy CryptoAPI-only provider, or its driver isn't installed)",
        0x8009_0029 => "its key provider doesn't support the operation",
        0x8009_0030 | 0x8009_0035 => "the smart card or token isn't available",
        0x8009_0036 => "the request was cancelled",
        0x8009_0022 | 0x8009_002E => "the key needs a prompt to open",
        0x8009_200B => "the certificate has no private key",
        0x8010_0000..=0x8010_FFFF => "the smart card isn't available",
        _ => return format!("Windows error 0x{code:08X}"),
    };
    format!("{meaning} (Windows error 0x{code:08X})")
}
