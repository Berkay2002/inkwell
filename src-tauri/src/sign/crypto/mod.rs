//! The cryptographic layer of digital signatures: X.509, CMS (PAdES B-B), PKCS #12, RSA and
//! ECDSA, the Windows certificate store and chain trust. Adapted from pdfcraft's `sign` crate
//! (MIT OR Apache-2.0, see third_party/pdfcraft); no PDF handling lives here.
//!
//! Not supported, reported as [`SignError::Unsupported`]: P-521, Brainpool and Ed25519 keys,
//! SHA-3 and RIPEMD-160 digests.

pub mod cms;
pub mod der;
pub mod keys;
pub mod pkcs12;
mod rsa_pad;
pub mod trust;
#[cfg(windows)]
pub mod winstore;
pub mod x509;

#[cfg(test)]
mod tests;

pub use der::Time;
pub use keys::{DigestAlg, PrivateKey, PublicKey};
pub use pkcs12::DigitalId;
pub use trust::Trust;
pub use x509::{Certificate, Name};

#[derive(Debug, thiserror::Error)]
pub enum SignError {
    #[error("malformed data: {0}")]
    Malformed(String),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("{0}")]
    Crypto(String),
    #[error("{0}")]
    Pdf(String),
}
