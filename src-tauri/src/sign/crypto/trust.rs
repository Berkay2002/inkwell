//! Whether the machine trusts a signer's certificate: Windows' own chain building and base policy
//! (the same trust store Acrobat-style "valid signer" checks use). Never touches the network and
//! never checks revocation. Other platforms have no trust store here yet and say "untrusted root".

use std::time::SystemTime;

/// What the system says about a signer certificate at a point in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// The chain ends in a root the system trusts and every certificate was valid at the time.
    Trusted,
    /// Self-signed, or issued by a root (or an unknown issuer) the system does not trust.
    UntrustedRoot,
    /// A certificate in the chain had expired at the time.
    Expired,
    /// The signer's certificate was not yet valid at the time.
    NotYetValid,
    /// A certificate in the chain is revoked.
    Revoked,
    /// Any other failure, with the Windows error code and its meaning.
    Other(String),
}

/// Check `signer_der` (a certificate), helped by `extra_certs_der` (the other certificates found in
/// the signature) as intermediates, at `at` (now when `None`).
#[cfg(not(windows))]
pub fn check(_signer_der: &[u8], _extra_certs_der: &[impl AsRef<[u8]>], _at: Option<SystemTime>) -> Trust {
    Trust::UntrustedRoot
}

#[cfg(windows)]
pub fn check(signer_der: &[u8], extra_certs_der: &[impl AsRef<[u8]>], at: Option<SystemTime>) -> Trust {
    let at = at.unwrap_or_else(SystemTime::now);
    match win::verify(signer_der, extra_certs_der, at) {
        Ok(code) => classify(code, signer_der, at),
        Err(e) => Trust::Other(e.to_string()),
    }
}

#[cfg(windows)]
fn classify(code: u32, signer_der: &[u8], at: SystemTime) -> Trust {
    use super::der::Time;
    use super::x509::Certificate;
    match code {
        0 => Trust::Trusted,
        // CERT_E_UNTRUSTEDROOT, CERT_E_CHAINING, CERT_E_UNTRUSTEDTESTROOT, CERT_E_UNTRUSTEDCA
        0x800B_0109 | 0x800B_010A | 0x800B_010D | 0x800B_0112 => Trust::UntrustedRoot,
        // CERT_E_EXPIRED covers "not yet valid" as well; the signer's own dates tell them apart.
        0x800B_0101 => {
            let secs = at.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            match Certificate::parse(signer_der) {
                Ok(c) if Time::from_unix(secs) < c.not_before => Trust::NotYetValid,
                _ => Trust::Expired,
            }
        }
        // CERT_E_REVOKED, CRYPT_E_REVOKED
        0x800B_010C | 0x8009_2010 => Trust::Revoked,
        0x800B_0114 => Trust::Other(format!("the certificate's name is invalid (Windows error 0x{code:08X})")),
        0x800B_0106 | 0x800B_0110 => Trust::Other(format!("the certificate is not valid for this purpose (Windows error 0x{code:08X})")),
        0x800B_0104 => Trust::Other(format!("the certification path is longer than a CA allows (Windows error 0x{code:08X})")),
        0x8009_6004 => Trust::Other(format!("a certificate's signature is invalid (Windows error 0x{code:08X})")),
        _ => Trust::Other(format!("Windows error 0x{code:08X}")),
    }
}

/// Owned Windows certificate handles and the chain engine calls, shared with `winstore`.
#[cfg(windows)]
pub(super) mod win {
    use std::ptr::{null, null_mut};
    use std::time::SystemTime;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Security::Cryptography::*;

    use super::super::SignError;

    /// A certificate store handle, closed on drop.
    pub struct Store(pub HCERTSTORE);

    impl Drop for Store {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the handle came from CertOpenStore/PFXImportCertStore and is closed once.
                unsafe { CertCloseStore(self.0, 0) };
            }
        }
    }

    /// A reference-counted certificate context, released on drop.
    pub struct Cert(pub *const CERT_CONTEXT);

    // SAFETY: certificate contexts are immutable and reference counted by crypt32, which is thread safe.
    unsafe impl Send for Cert {}
    unsafe impl Sync for Cert {}

    impl Drop for Cert {
        fn drop(&mut self) {
            // SAFETY: the context is owned by this value (created or duplicated), released once.
            unsafe { CertFreeCertificateContext(self.0) };
        }
    }

    impl Cert {
        /// A context for a DER certificate, not in any store.
        pub fn from_der(der: &[u8]) -> Result<Cert, SignError> {
            let len = u32::try_from(der.len()).map_err(|_| SignError::Malformed("certificate too large".into()))?;
            // SAFETY: pointer and length describe `der`, which CertCreateCertificateContext copies.
            let p = unsafe { CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), len) };
            if p.is_null() { Err(SignError::Malformed("Windows could not read the certificate".into())) } else { Ok(Cert(p)) }
        }

        /// The certificate's DER encoding.
        pub fn der(&self) -> &[u8] {
            // SAFETY: a live context points at `cbCertEncoded` bytes that stay valid while it lives.
            unsafe { std::slice::from_raw_parts((*self.0).pbCertEncoded, (*self.0).cbCertEncoded as usize) }
        }

        /// Whether the context has a private key attached (a key provider reference, or a live key handle).
        pub fn has_key(&self) -> bool {
            [CERT_KEY_PROV_INFO_PROP_ID, CERT_KEY_PROV_HANDLE_PROP_ID, CERT_KEY_CONTEXT_PROP_ID, CERT_NCRYPT_KEY_HANDLE_PROP_ID].into_iter().any(|prop| {
                let mut size = 0u32;
                // SAFETY: a size-only query (null buffer) on a live context.
                unsafe { CertGetCertificateContextProperty(self.0, prop, null_mut(), &mut size) != 0 }
            })
        }
    }

    impl Store {
        /// An empty in-memory store (never persisted).
        pub fn memory() -> Result<Store, SignError> {
            // SAFETY: CERT_STORE_PROV_MEMORY takes no parameters.
            let h = unsafe { CertOpenStore(CERT_STORE_PROV_MEMORY, 0, 0, 0, null()) };
            if h.is_null() { Err(SignError::Crypto("Windows could not create a certificate store".into())) } else { Ok(Store(h)) }
        }

        pub fn add_der(&self, der: &[u8]) -> bool {
            let Ok(len) = u32::try_from(der.len()) else { return false };
            // SAFETY: pointer and length describe `der`; the store copies it.
            unsafe { CertAddEncodedCertificateToStore(self.0, X509_ASN_ENCODING, der.as_ptr(), len, CERT_STORE_ADD_USE_EXISTING, null_mut()) != 0 }
        }

        /// Every certificate in the store, as independent references.
        pub fn certs(&self) -> Vec<Cert> {
            let mut out = Vec::new();
            let mut prev: *const CERT_CONTEXT = null();
            loop {
                // SAFETY: `prev` is null or the context the previous call returned (which this call releases).
                let next = unsafe { CertEnumCertificatesInStore(self.0, prev) };
                if next.is_null() {
                    return out;
                }
                // SAFETY: `next` is a live context; the duplicate is our own reference.
                out.push(Cert(unsafe { CertDuplicateCertificateContext(next) }));
                prev = next;
            }
        }
    }

    /// A built certificate chain, freed on drop.
    pub struct Chain(*const CERT_CHAIN_CONTEXT);

    impl Drop for Chain {
        fn drop(&mut self) {
            // SAFETY: owned chain context from CertGetCertificateChain, freed once.
            unsafe { CertFreeCertificateChain(self.0) };
        }
    }

    impl Chain {
        /// The certificates of the first (best) chain, leaf first, root last when it was found.
        pub fn certs_der(&self) -> Vec<Vec<u8>> {
            let mut out = Vec::new();
            // SAFETY: the chain context and the arrays it points to are valid while `self` lives,
            // with the counts the structures state.
            unsafe {
                let chain = &*self.0;
                if chain.cChain == 0 {
                    return out;
                }
                let simple = &**chain.rgpChain;
                for i in 0..simple.cElement as usize {
                    let element = &**simple.rgpElement.add(i);
                    let ctx = element.pCertContext;
                    if !ctx.is_null() {
                        out.push(std::slice::from_raw_parts((*ctx).pbCertEncoded, (*ctx).cbCertEncoded as usize).to_vec());
                    }
                }
            }
            out
        }
    }

    fn filetime(t: SystemTime) -> FILETIME {
        // 100 ns ticks since 1601-01-01.
        let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let ticks = (secs + 11_644_473_600) * 10_000_000;
        FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 }
    }

    /// Build the chain for `cert`, helped by `extra` (may hold intermediates). Only the local
    /// caches are used for AIA and CRL URLs, so this never goes to the network; no revocation checks.
    pub fn chain(cert: &Cert, extra: Option<&Store>, at: Option<SystemTime>) -> Result<Chain, SignError> {
        // SAFETY: zeroed CERT_CHAIN_PARA with cbSize set is the documented "no usage restriction" form.
        let mut para: CERT_CHAIN_PARA = unsafe { std::mem::zeroed() };
        para.cbSize = std::mem::size_of::<CERT_CHAIN_PARA>() as u32;
        let time = at.map(filetime);
        let mut out: *mut CERT_CHAIN_CONTEXT = null_mut();
        // SAFETY: all pointers are valid for the call; the chain context is returned to `out`.
        let ok = unsafe {
            CertGetCertificateChain(
                null_mut(),
                cert.0,
                time.as_ref().map_or(null(), |t| t as *const FILETIME),
                extra.map_or(null_mut(), |s| s.0),
                &para,
                CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL,
                null(),
                &mut out,
            )
        };
        if ok == 0 || out.is_null() { Err(SignError::Crypto("Windows could not build the certificate chain".into())) } else { Ok(Chain(out)) }
    }

    /// The base chain policy's verdict (`dwError`, 0 for trusted) for `signer_der`.
    pub fn verify(signer_der: &[u8], extra: &[impl AsRef<[u8]>], at: SystemTime) -> Result<u32, SignError> {
        let signer = Cert::from_der(signer_der)?;
        let store = Store::memory()?;
        for e in extra {
            // A certificate Windows can't read only loses a possible intermediate.
            store.add_der(e.as_ref());
        }
        let chain = chain(&signer, Some(&store), Some(at))?;
        // SAFETY: zeroed policy structures with cbSize set, as the API documents.
        let (para, mut status) = unsafe {
            let mut para: CERT_CHAIN_POLICY_PARA = std::mem::zeroed();
            para.cbSize = std::mem::size_of::<CERT_CHAIN_POLICY_PARA>() as u32;
            let mut status: CERT_CHAIN_POLICY_STATUS = std::mem::zeroed();
            status.cbSize = std::mem::size_of::<CERT_CHAIN_POLICY_STATUS>() as u32;
            (para, status)
        };
        // SAFETY: a valid chain context and policy structures for the call.
        let ok = unsafe { CertVerifyCertificateChainPolicy(CERT_CHAIN_POLICY_BASE, chain.0, &para, &mut status) };
        if ok == 0 { Err(SignError::Crypto("Windows could not evaluate the certificate chain".into())) } else { Ok(status.dwError) }
    }
}
