// Adapted from pdfcraft (MIT OR Apache-2.0), crates/sign/src/keys.rs @ b8ddf71
//! Digests, public keys (verification everywhere) and private keys (signing).
//!
//! Verification uses RustCrypto. RSA private-key operations use the `rsa` crate's blinded
//! PKCS #1 v1.5 signing; ECDSA signing uses RustCrypto with deterministic nonces (RFC 6979).
//! Not supported (reported as [`SignError::Unsupported`]): P-521, the Brainpool curves, Ed25519,
//! SHA-3 and RIPEMD-160.

use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey};

use super::SignError;
use super::der::{self, Tlv, tag};

pub mod oid {
    pub const SHA1: &str = "1.3.14.3.2.26";
    pub const SHA224: &str = "2.16.840.1.101.3.4.2.4";
    pub const SHA256: &str = "2.16.840.1.101.3.4.2.1";
    pub const SHA384: &str = "2.16.840.1.101.3.4.2.2";
    pub const SHA512: &str = "2.16.840.1.101.3.4.2.3";
    pub const SHA512_224: &str = "2.16.840.1.101.3.4.2.5";
    pub const SHA512_256: &str = "2.16.840.1.101.3.4.2.6";
    pub const RSA: &str = "1.2.840.113549.1.1.1";
    pub const RSA_PSS: &str = "1.2.840.113549.1.1.10";
    pub const MGF1: &str = "1.2.840.113549.1.1.8";
    pub const EC: &str = "1.2.840.10045.2.1";
    pub const P256: &str = "1.2.840.10045.3.1.7";
    pub const P384: &str = "1.3.132.0.34";
}

/// Message digests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DigestAlg {
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha512_224,
    Sha512_256,
}

impl DigestAlg {
    /// Every digest, for tables.
    pub const ALL: [DigestAlg; 7] =
        [DigestAlg::Sha1, DigestAlg::Sha224, DigestAlg::Sha256, DigestAlg::Sha384, DigestAlg::Sha512, DigestAlg::Sha512_224, DigestAlg::Sha512_256];

    pub fn from_oid(o: &str) -> Option<DigestAlg> {
        DigestAlg::ALL.into_iter().find(|d| d.oid() == o)
    }

    pub fn oid(self) -> &'static str {
        match self {
            DigestAlg::Sha1 => oid::SHA1,
            DigestAlg::Sha224 => oid::SHA224,
            DigestAlg::Sha256 => oid::SHA256,
            DigestAlg::Sha384 => oid::SHA384,
            DigestAlg::Sha512 => oid::SHA512,
            DigestAlg::Sha512_224 => oid::SHA512_224,
            DigestAlg::Sha512_256 => oid::SHA512_256,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DigestAlg::Sha1 => "SHA-1",
            DigestAlg::Sha224 => "SHA-224",
            DigestAlg::Sha256 => "SHA-256",
            DigestAlg::Sha384 => "SHA-384",
            DigestAlg::Sha512 => "SHA-512",
            DigestAlg::Sha512_224 => "SHA-512/224",
            DigestAlg::Sha512_256 => "SHA-512/256",
        }
    }

    /// The digest of the concatenation of `parts` (hash two PDF byte ranges without joining them).
    pub fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: sha2::Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut h = D::new();
            for p in parts {
                h.update(p);
            }
            h.finalize().to_vec()
        }
        match self {
            DigestAlg::Sha1 => run::<sha1::Sha1>(parts),
            DigestAlg::Sha224 => run::<sha2::Sha224>(parts),
            DigestAlg::Sha256 => run::<sha2::Sha256>(parts),
            DigestAlg::Sha384 => run::<sha2::Sha384>(parts),
            DigestAlg::Sha512 => run::<sha2::Sha512>(parts),
            DigestAlg::Sha512_224 => run::<sha2::Sha512_224>(parts),
            DigestAlg::Sha512_256 => run::<sha2::Sha512_256>(parts),
        }
    }

    /// What to tell the user when a signature that still validates uses this digest, or `None`
    /// for one that is fine. Old documents carry SHA-1, so they validate, but a signature panel
    /// that showed them like a SHA-256 one would overstate what they prove.
    pub fn weakness(self) -> Option<&'static str> {
        match self {
            DigestAlg::Sha1 => Some(
                "This signature uses SHA-1, a weak hash algorithm: collisions in it have been demonstrated. It still validates because many documents signed years ago use it, but it should not be relied on.",
            ),
            _ => None,
        }
    }

    /// The digest length in bytes.
    pub fn output_len(self) -> usize {
        match self {
            DigestAlg::Sha1 => 20,
            DigestAlg::Sha224 | DigestAlg::Sha512_224 => 28,
            DigestAlg::Sha256 | DigestAlg::Sha512_256 => 32,
            DigestAlg::Sha384 => 48,
            DigestAlg::Sha512 => 64,
        }
    }

    /// The `AlgorithmIdentifier` (no parameters, as RFC 5754 recommends).
    pub fn algorithm(self) -> Vec<u8> {
        der::algorithm(self.oid(), None)
    }
}

/// RSASSA-PSS parameters (RFC 4055 §3.1) besides the message digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PssParams {
    /// The MGF1 hash.
    pub mgf: DigestAlg,
    /// The declared salt length; `None` recovers it from the signature.
    pub salt_len: Option<usize>,
}

/// How a signature value is computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    RsaPkcs1,
    RsaPss(PssParams),
    /// ECDSA over a digest, the signature DER or plain `r ‖ s`.
    Ecdsa,
}

/// Signature algorithm OIDs that name both the scheme and the digest.
const SIGNATURE_OIDS: &[(&str, Scheme, DigestAlg)] = &[
    ("1.2.840.113549.1.1.5", Scheme::RsaPkcs1, DigestAlg::Sha1),
    ("1.2.840.113549.1.1.14", Scheme::RsaPkcs1, DigestAlg::Sha224),
    ("1.2.840.113549.1.1.11", Scheme::RsaPkcs1, DigestAlg::Sha256),
    ("1.2.840.113549.1.1.12", Scheme::RsaPkcs1, DigestAlg::Sha384),
    ("1.2.840.113549.1.1.13", Scheme::RsaPkcs1, DigestAlg::Sha512),
    ("1.2.840.113549.1.1.15", Scheme::RsaPkcs1, DigestAlg::Sha512_224),
    ("1.2.840.113549.1.1.16", Scheme::RsaPkcs1, DigestAlg::Sha512_256),
    ("1.2.840.10045.4.1", Scheme::Ecdsa, DigestAlg::Sha1),
    ("1.2.840.10045.4.3.1", Scheme::Ecdsa, DigestAlg::Sha224),
    ("1.2.840.10045.4.3.2", Scheme::Ecdsa, DigestAlg::Sha256),
    ("1.2.840.10045.4.3.3", Scheme::Ecdsa, DigestAlg::Sha384),
    ("1.2.840.10045.4.3.4", Scheme::Ecdsa, DigestAlg::Sha512),
    // BSI TR-03111 "plain" ECDSA (r ‖ s), as German signature cards write it.
    ("0.4.0.127.0.7.1.1.4.1.1", Scheme::Ecdsa, DigestAlg::Sha1),
    ("0.4.0.127.0.7.1.1.4.1.2", Scheme::Ecdsa, DigestAlg::Sha224),
    ("0.4.0.127.0.7.1.1.4.1.3", Scheme::Ecdsa, DigestAlg::Sha256),
    ("0.4.0.127.0.7.1.1.4.1.4", Scheme::Ecdsa, DigestAlg::Sha384),
    ("0.4.0.127.0.7.1.1.4.1.5", Scheme::Ecdsa, DigestAlg::Sha512),
];

/// A signature `AlgorithmIdentifier`: the scheme and, when the identifier names one, the digest.
pub fn signature_algorithm(alg: &Tlv<'_>) -> Result<(Scheme, Option<DigestAlg>), SignError> {
    let parts = alg.children()?;
    let o = parts.first().ok_or_else(|| SignError::Malformed("empty algorithm".into()))?.oid()?;
    if let Some((_, scheme, digest)) = SIGNATURE_OIDS.iter().find(|(k, _, _)| *k == o) {
        return Ok((*scheme, Some(*digest)));
    }
    Ok(match o.as_str() {
        oid::RSA => (Scheme::RsaPkcs1, None),
        oid::EC => (Scheme::Ecdsa, None),
        oid::RSA_PSS => {
            let (hash, params) = pss_params(parts.get(1))?;
            (Scheme::RsaPss(params), Some(hash))
        }
        other => return Err(SignError::Unsupported(format!("signature algorithm {other}"))),
    })
}

/// `RSASSA-PSS-params` (RFC 4055): `[0]` hash (default SHA-1), `[1]` mask generation function
/// (default MGF1 with SHA-1), `[2]` salt length (default 20). Absent parameters mean the defaults.
fn pss_params(params: Option<&Tlv<'_>>) -> Result<(DigestAlg, PssParams), SignError> {
    let hash_of = |alg: &Tlv<'_>| -> Result<DigestAlg, SignError> {
        let o = alg.children()?.first().ok_or_else(|| SignError::Malformed("PSS hash".into()))?.oid()?;
        DigestAlg::from_oid(&o).ok_or_else(|| SignError::Unsupported(format!("digest {o}")))
    };
    let mut hash = DigestAlg::Sha1;
    let mut mgf = DigestAlg::Sha1;
    let mut salt_len = 20usize;
    if let Some(p) = params.filter(|p| p.tag == tag::SEQUENCE) {
        for field in p.children()? {
            match field.tag {
                t if t == tag::ctx(0) => hash = hash_of(&field.inner()?)?,
                t if t == tag::ctx(1) => {
                    let m = field.inner()?.children()?;
                    if m.first().map(|o| o.oid()).transpose()?.as_deref() != Some(oid::MGF1) {
                        return Err(SignError::Unsupported("PSS mask generation function other than MGF1".into()));
                    }
                    mgf = hash_of(m.get(1).ok_or_else(|| SignError::Malformed("MGF1 hash".into()))?)?;
                }
                t if t == tag::ctx(2) => {
                    salt_len = usize::try_from(field.inner()?.u64()?).map_err(|_| SignError::Malformed("PSS salt length".into()))?
                }
                _ => {}
            }
        }
    }
    Ok((hash, PssParams { mgf, salt_len: Some(salt_len) }))
}

/// A public key from a `SubjectPublicKeyInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicKey {
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    P256(Vec<u8>),
    P384(Vec<u8>),
    /// A key algorithm or curve Inkwell can't check signatures with (P-521, Brainpool, Ed25519…).
    /// The certificate still parses (so its name and dates show); verifying with it gives
    /// [`SignError::Unsupported`].
    Unsupported {
        what: String,
        spki: Vec<u8>,
    },
}

/// The private key a platform key store (the Windows certificate store, through CNG) must
/// hold to sign for a certificate, see [`PublicKey::store_signing_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKey {
    Rsa,
    /// ECDSA with a key of this many bits.
    Ecdsa(u32),
}

impl PublicKey {
    /// The `subjectPublicKey` BIT STRING contents. RSA keys are re-encoded; EC points are the
    /// stored bytes.
    pub fn key_bits(&self) -> Vec<u8> {
        match self {
            PublicKey::Rsa { n, e } => der::seq(&[&der::uint(n), &der::uint(e)]),
            PublicKey::P256(bits) | PublicKey::P384(bits) => bits.clone(),
            // The BIT STRING of the stored SubjectPublicKeyInfo (empty if it doesn't parse).
            PublicKey::Unsupported { spki, .. } => Tlv::parse(spki)
                .ok()
                .and_then(|(t, _)| t.children().ok())
                .and_then(|c| c.get(1).and_then(|k| k.bits().ok().map(<[u8]>::to_vec)))
                .unwrap_or_default(),
        }
    }

    pub fn from_spki(spki: &Tlv<'_>) -> Result<PublicKey, SignError> {
        let parts = spki.children()?;
        let [alg, key] = parts.as_slice() else { return Err(SignError::Malformed("SubjectPublicKeyInfo".into())) };
        let alg = alg.children()?;
        let o = alg.first().ok_or_else(|| SignError::Malformed("key algorithm".into()))?.oid()?;
        let bits = key.bits()?;
        match o.as_str() {
            oid::RSA => {
                let k = Tlv::parse_all(bits)?.children()?;
                let [n, e] = k.as_slice() else { return Err(SignError::Malformed("RSAPublicKey".into())) };
                Ok(PublicKey::Rsa { n: n.uint_bytes().to_vec(), e: e.uint_bytes().to_vec() })
            }
            oid::EC => match alg.get(1).map(|c| c.oid()).transpose()?.as_deref() {
                Some(oid::P256) => Ok(PublicKey::P256(bits.to_vec())),
                Some(oid::P384) => Ok(PublicKey::P384(bits.to_vec())),
                other => Ok(PublicKey::Unsupported { what: format!("elliptic curve {}", other.unwrap_or("?")), spki: spki.raw.to_vec() }),
            },
            other => Ok(PublicKey::Unsupported { what: format!("public key algorithm {other}"), spki: spki.raw.to_vec() }),
        }
    }

    /// The `SubjectPublicKeyInfo` encoding.
    pub fn spki(&self) -> Vec<u8> {
        match self {
            PublicKey::Rsa { n, e } => {
                let rsa = der::seq(&[&der::uint(n), &der::uint(e)]);
                der::seq(&[&der::algorithm(oid::RSA, Some(&der::null())), &der::bit_string(&rsa)])
            }
            PublicKey::P256(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::P256)]), &der::bit_string(p)]),
            PublicKey::P384(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::P384)]), &der::bit_string(p)]),
            PublicKey::Unsupported { spki, .. } => spki.clone(),
        }
    }

    /// "RSA 2048-bit", "ECDSA P-256"…
    pub fn describe(&self) -> String {
        match self {
            PublicKey::Rsa { n, .. } => format!("RSA {}-bit", n.len() * 8),
            PublicKey::P256(_) => "ECDSA P-256".into(),
            PublicKey::P384(_) => "ECDSA P-384".into(),
            PublicKey::Unsupported { what, .. } => format!("unsupported ({what})"),
        }
    }

    // The two decisions below are what `winstore.rs` asks of a key. They live here, with every
    // variant named and no `_` arm, so that a new variant fails to compile on every platform
    // rather than only in the Windows-only module.

    /// The key a platform key store must hold to sign for this public key, or `None` when
    /// Inkwell doesn't sign with this kind of key, so the store's identity is skipped.
    pub fn store_signing_key(&self) -> Option<StoreKey> {
        match self {
            PublicKey::Rsa { .. } => Some(StoreKey::Rsa),
            PublicKey::P256(_) => Some(StoreKey::Ecdsa(256)),
            PublicKey::P384(_) => Some(StoreKey::Ecdsa(384)),
            PublicKey::Unsupported { .. } => None,
        }
    }

    /// A key store's signature as a CMS signature carries it: RSA unchanged, ECDSA's fixed-size
    /// `r ‖ s` (what CNG returns) as a DER `ECDSA-Sig-Value`. Keys [`Self::store_signing_key`]
    /// rejects give [`SignError::Unsupported`].
    pub fn store_signature_der(&self, raw: &[u8]) -> Result<Vec<u8>, SignError> {
        match self {
            PublicKey::Rsa { .. } => Ok(raw.to_vec()),
            PublicKey::P256(_) => p256::ecdsa::Signature::from_slice(raw)
                .map(|s| s.to_der().as_bytes().to_vec())
                .map_err(|e| SignError::Crypto(format!("the key store returned an invalid P-256 signature: {e}"))),
            PublicKey::P384(_) => p384::ecdsa::Signature::from_slice(raw)
                .map(|s| s.to_der().as_bytes().to_vec())
                .map_err(|e| SignError::Crypto(format!("the key store returned an invalid P-384 signature: {e}"))),
            PublicKey::Unsupported { .. } => Err(SignError::Unsupported(format!("signing with {} keys", self.describe()))),
        }
    }

    /// Check `sig` over a message whose digest (with `alg`) is `digest`. Encodings are read
    /// leniently (see [`super::rsa_pad`], [`ecdsa_rs`]); the cryptographic check never is.
    /// `Err(Unsupported)` means "can't tell", as opposed to `Ok(false)`: "doesn't match".
    pub fn verify(&self, scheme: Scheme, alg: DigestAlg, digest: &[u8], sig: &[u8]) -> Result<bool, SignError> {
        self.verify_noting(scheme, alg, digest, sig, &mut Vec::new())
    }

    /// [`PublicKey::verify`], noting in `notes` each encoding irregularity it tolerated. Only
    /// meaningful when it returns `Ok(true)`: a signature that doesn't verify may leave notes
    /// about how far it got.
    pub fn verify_noting(&self, scheme: Scheme, alg: DigestAlg, digest: &[u8], sig: &[u8], notes: &mut Vec<&'static str>) -> Result<bool, SignError> {
        match (self, scheme) {
            (PublicKey::Unsupported { what, .. }, _) => Err(SignError::Unsupported(what.clone())),
            (PublicKey::Rsa { n, e }, Scheme::RsaPkcs1) => super::rsa_pad::pkcs1_verify(n, e, alg, digest, sig, notes),
            (PublicKey::Rsa { n, e }, Scheme::RsaPss(params)) => super::rsa_pad::pss_verify(n, e, alg, params, digest, sig, notes),
            (PublicKey::P256(p), Scheme::Ecdsa) => {
                use p256::ecdsa::signature::hazmat::PrehashVerifier;
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(p).map_err(|_| SignError::Malformed("P-256 key".into()))?;
                let Some(s) = ecdsa_rs(sig, 32, notes).and_then(|rs| p256::ecdsa::Signature::from_slice(&rs).ok()) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            (PublicKey::P384(p), Scheme::Ecdsa) => {
                use p384::ecdsa::signature::hazmat::PrehashVerifier;
                let key = p384::ecdsa::VerifyingKey::from_sec1_bytes(p).map_err(|_| SignError::Malformed("P-384 key".into()))?;
                let Some(s) = ecdsa_rs(sig, 48, notes).and_then(|rs| p384::ecdsa::Signature::from_slice(&rs).ok()) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            _ => Ok(false),
        }
    }
}

/// An ECDSA signature as the fixed-width `r ‖ s` the curve crates read: from the standard DER
/// `SEQUENCE { INTEGER r, INTEGER s }` (also with integers that have redundant leading zeros or
/// the sign byte missing), or already raw `r ‖ s` as PKCS #11 tokens return it. `notes` gets an
/// entry when the signature was not canonical DER.
fn ecdsa_rs(sig: &[u8], field_len: usize, notes: &mut Vec<&'static str>) -> Option<Vec<u8>> {
    let pad = |int: &[u8]| -> Option<Vec<u8>> {
        let int = int.get(int.iter().position(|b| *b != 0).unwrap_or(int.len())..).unwrap_or_default();
        let zeros = field_len.checked_sub(int.len())?;
        Some(std::iter::repeat_n(0u8, zeros).chain(int.iter().copied()).collect())
    };
    if let Ok((t, rest)) = Tlv::parse(sig)
        && t.tag == tag::SEQUENCE
        && rest.iter().all(|b| *b == 0)
        && let Ok(parts) = t.children()
        && let [r, s] = parts.as_slice()
        && r.tag == tag::INTEGER
        && s.tag == tag::INTEGER
    {
        // Canonical DER: nothing after the sequence, minimal positive integers.
        let minimal = |i: &Tlv<'_>| i.value.first().is_some_and(|b| b & 0x80 == 0) && !matches!(i.value, [0, b, ..] if b & 0x80 == 0);
        if !rest.is_empty() || !minimal(r) || !minimal(s) {
            notes.push("The ECDSA signature is not in canonical DER (trailing bytes, padding or a missing sign byte); it was read anyway.");
        }
        let mut out = pad(r.value)?;
        out.extend(pad(s.value)?);
        return Some(out);
    }
    (sig.len() == field_len * 2).then(|| {
        notes.push("The ECDSA signature is the raw r and s values instead of a DER sequence; it was read anyway.");
        sig.to_vec()
    })
}

/// The operating system's random number generator, as the `rsa` crate wants one.
struct SysRng;

impl rsa::rand_core::TryRng for SysRng {
    // Infallible as the `rsa` crate requires: an OS without randomness can't sign safely anyway.
    type Error = std::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut b = [0u8; 4];
        fill(&mut b);
        Ok(u32::from_le_bytes(b))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut b = [0u8; 8];
        fill(&mut b);
        Ok(u64::from_le_bytes(b))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        fill(dst);
        Ok(())
    }
}

fn fill(dst: &mut [u8]) {
    getrandom::fill(dst).expect("OS random generator failed");
}

impl rsa::rand_core::TryCryptoRng for SysRng {}

enum Inner {
    Rsa(Box<rsa::RsaPrivateKey>),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
    /// A key held elsewhere (the Windows certificate store, a token) that signs on request.
    External(std::sync::Arc<dyn ExternalKey>),
}

/// A private key Inkwell can't read, only ask to sign (OS key stores, tokens).
pub trait ExternalKey: Send + Sync {
    /// Sign `msg`, hashing it with `alg` (PKCS #1 v1.5 for RSA, DER-encoded ECDSA).
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError>;
}

/// A private key that can sign.
pub struct PrivateKey {
    inner: Inner,
    public: PublicKey,
    /// The PKCS#8 `PrivateKeyInfo` it came from (to write it back into a PKCS#12 file).
    pkcs8: Vec<u8>,
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PrivateKey({})", self.public.describe())
    }
}

impl PrivateKey {
    /// A key held outside Inkwell's memory (its public half is `public`). It can't be
    /// exported to a PKCS #12 file.
    pub fn external(public: PublicKey, key: std::sync::Arc<dyn ExternalKey>) -> PrivateKey {
        PrivateKey { inner: Inner::External(key), public, pkcs8: Vec::new() }
    }

    /// Whether the key lives outside the app (and so can't be exported).
    pub fn is_external(&self) -> bool {
        matches!(self.inner, Inner::External(_))
    }

    /// From a PKCS#8 `PrivateKeyInfo`.
    pub fn from_pkcs8(info: &[u8]) -> Result<PrivateKey, SignError> {
        let t = Tlv::parse_all(info)?.children()?;
        let [_version, alg, key, ..] = t.as_slice() else { return Err(SignError::Malformed("PrivateKeyInfo".into())) };
        let alg = alg.children()?;
        let o = alg.first().ok_or_else(|| SignError::Malformed("key algorithm".into()))?.oid()?;
        let key = key.expect(tag::OCTET_STRING, "private key")?;
        match o.as_str() {
            oid::RSA => {
                let k = Tlv::parse_all(key.value)?.children()?;
                let (n, e) = match k.as_slice() {
                    [_, n, e, ..] => (n.uint_bytes().to_vec(), e.uint_bytes().to_vec()),
                    _ => return Err(SignError::Malformed("RSAPrivateKey".into())),
                };
                let rsa = rsa::RsaPrivateKey::from_pkcs8_der(info).map_err(|e| SignError::Malformed(format!("RSA private key: {e}")))?;
                Ok(PrivateKey { inner: Inner::Rsa(Box::new(rsa)), public: PublicKey::Rsa { n, e }, pkcs8: info.to_vec() })
            }
            oid::EC => {
                // ECPrivateKey: version, privateKey OCTET STRING, [0] parameters, [1] publicKey.
                let ec = Tlv::parse_all(key.value)?.children()?;
                let d = ec.get(1).ok_or_else(|| SignError::Malformed("ECPrivateKey".into()))?.expect(tag::OCTET_STRING, "EC private key")?.value;
                let curve = match alg.get(1).map(|c| c.oid()).transpose()? {
                    Some(c) => c,
                    None => ec
                        .iter()
                        .find(|t| t.tag == tag::ctx(0))
                        .and_then(|p| p.inner().ok())
                        .and_then(|p| p.oid().ok())
                        .ok_or_else(|| SignError::Malformed("EC curve".into()))?,
                };
                match curve.as_str() {
                    oid::P256 => {
                        let k = p256::ecdsa::SigningKey::from_slice(d).map_err(|_| SignError::Malformed("P-256 key".into()))?;
                        let public = PublicKey::P256(k.verifying_key().to_sec1_point(false).as_bytes().to_vec());
                        Ok(PrivateKey { inner: Inner::P256(k), public, pkcs8: info.to_vec() })
                    }
                    oid::P384 => {
                        let k = p384::ecdsa::SigningKey::from_slice(d).map_err(|_| SignError::Malformed("P-384 key".into()))?;
                        let public = PublicKey::P384(k.verifying_key().to_sec1_point(false).as_bytes().to_vec());
                        Ok(PrivateKey { inner: Inner::P384(k), public, pkcs8: info.to_vec() })
                    }
                    other => Err(SignError::Unsupported(format!("elliptic curve {other}"))),
                }
            }
            other => Err(SignError::Unsupported(format!("private key algorithm {other}"))),
        }
    }

    /// A new RSA key (2048-bit is Acrobat's default for a self-signed digital ID).
    pub fn generate_rsa(bits: usize) -> Result<PrivateKey, SignError> {
        if !matches!(bits, 2048 | 3072 | 4096) {
            return Err(SignError::Unsupported(format!("{bits}-bit RSA keys")));
        }
        let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::UnwrapErr(SysRng), bits)
            .map_err(|e| SignError::Crypto(format!("RSA key generation failed: {e}")))?;
        let der = key.to_pkcs8_der().map_err(|e| SignError::Crypto(format!("RSA key encoding failed: {e}")))?;
        PrivateKey::from_pkcs8(der.as_bytes())
    }

    /// A new P-256 key.
    pub fn generate_p256() -> Result<PrivateKey, SignError> {
        loop {
            let mut d = [0u8; 32];
            getrandom::fill(&mut d).map_err(|e| SignError::Crypto(format!("no randomness: {e}")))?;
            if let Ok(k) = p256::ecdsa::SigningKey::from_slice(&d) {
                let point = k.verifying_key().to_sec1_point(false).as_bytes().to_vec();
                // PKCS#8 with an ECPrivateKey (RFC 5915) carrying the public key.
                let ec = der::seq(&[&der::int(1), &der::octets(&d), &der::explicit(1, &der::bit_string(&point))]);
                let info = der::seq(&[&der::int(0), &der::seq(&[&der::oid(oid::EC), &der::oid(oid::P256)]), &der::octets(&ec)]);
                return PrivateKey::from_pkcs8(&info);
            }
        }
    }

    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    pub fn pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }

    /// The signature `AlgorithmIdentifier` this key writes with `alg`.
    pub fn signature_algorithm(&self, alg: DigestAlg) -> Vec<u8> {
        let rsa = matches!(self.public, PublicKey::Rsa { .. });
        let scheme = if rsa { Scheme::RsaPkcs1 } else { Scheme::Ecdsa };
        // `sign` only writes SHA-256/384/512, which the table has for both schemes.
        let oid = SIGNATURE_OIDS
            .iter()
            .find(|(_, s, d)| *s == scheme && *d == alg)
            .map_or(if rsa { "1.2.840.113549.1.1.11" } else { "1.2.840.10045.4.3.2" }, |(o, _, _)| *o);
        der::algorithm(oid, rsa.then(der::null).as_deref())
    }

    /// The digest a signature with this key should use: SHA-256, or SHA-384 for P-384.
    pub fn preferred_digest(&self) -> DigestAlg {
        match self.public {
            PublicKey::P384(_) => DigestAlg::Sha384,
            _ => DigestAlg::Sha256,
        }
    }

    /// Sign `msg` (hashed with `alg`). New signatures never use SHA-1.
    pub fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        if !matches!(alg, DigestAlg::Sha256 | DigestAlg::Sha384 | DigestAlg::Sha512) {
            return Err(SignError::Unsupported(format!("{} for new signatures", alg.name())));
        }
        match &self.inner {
            Inner::Rsa(k) => {
                // PKCS #1 v1.5: the DigestInfo is built here and padded by the `rsa` crate; the RNG
                // blinds the private-key operation (which also checks its result against the public key).
                let info = super::rsa_pad::digest_info(alg, &alg.digest(&[msg]), true);
                k.sign_with_rng(&mut SysRng, rsa::Pkcs1v15Sign::new_unprefixed(), &info).map_err(|e| SignError::Crypto(format!("RSA signing failed: {e}")))
            }
            Inner::P256(k) => {
                use p256::ecdsa::signature::hazmat::PrehashSigner;
                let s: p256::ecdsa::Signature = k.sign_prehash(&alg.digest(&[msg])).map_err(|e| SignError::Crypto(e.to_string()))?;
                Ok(s.to_der().as_bytes().to_vec())
            }
            Inner::External(k) => k.sign(alg, msg),
            Inner::P384(k) => {
                use p384::ecdsa::signature::hazmat::PrehashSigner;
                let s: p384::ecdsa::Signature = k.sign_prehash(&alg.digest(&[msg])).map_err(|e| SignError::Crypto(e.to_string()))?;
                Ok(s.to_der().as_bytes().to_vec())
            }
        }
    }
}
