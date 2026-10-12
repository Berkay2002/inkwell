//! Keys (DPAPI-wrapped), object encryption (XChaCha20-Poly1305) and keyed hashes (BLAKE3).

use super::types::{ChunkId, DocId, Hash, HistoryError, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS random generator failed");
    b
}

/// Object kinds bound into the AAD so one object can't be swapped for another.
#[derive(Clone, Copy)]
pub enum Obj {
    Chunk = b'c' as isize,
    Manifest = b'v' as isize,
    Names = b'n' as isize,
}

/// Everything derived from one document's random key. Wiped on drop (the AEAD wipes its own key).
pub struct Keys {
    aead: XChaCha20Poly1305,
    chunk_id: [u8; 32],
    content: [u8; 32],
}

impl Keys {
    pub fn new(doc_key: &[u8; 32]) -> Self {
        let aead_key = Zeroizing::new(blake3::derive_key("inkwell history 2026-10 object encryption", doc_key));
        Keys {
            aead: XChaCha20Poly1305::new((&*aead_key).into()),
            chunk_id: blake3::derive_key("inkwell history 2026-10 chunk id", doc_key),
            content: blake3::derive_key("inkwell history 2026-10 content hash", doc_key),
        }
    }

    pub fn chunk_id(&self, plain: &[u8]) -> ChunkId {
        ChunkId(*blake3::keyed_hash(&self.chunk_id, plain).as_bytes())
    }

    pub fn content_hash(&self, bytes: &[u8]) -> Hash {
        Hash(*blake3::keyed_hash(&self.content, bytes).as_bytes())
    }

    /// nonce(24) || ciphertext of (flag || zstd-or-raw plaintext).
    pub fn seal(&self, doc: DocId, kind: Obj, id: &[u8], plain: &[u8]) -> Vec<u8> {
        let packed = pack(plain);
        let nonce: [u8; 24] = random();
        let aad = aad(doc, kind, id);
        let ct = self
            .aead
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &packed, aad: &aad })
            .expect("encryption of an in-memory buffer cannot fail");
        let mut out = Vec::with_capacity(24 + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    pub fn open(&self, doc: DocId, kind: Obj, id: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < 24 + 16 + 1 {
            return Err(HistoryError::Integrity("object too short".into()));
        }
        let aad = aad(doc, kind, id);
        let packed = self
            .aead
            .decrypt(XNonce::from_slice(&sealed[..24]), Payload { msg: &sealed[24..], aad: &aad })
            .map_err(|_| HistoryError::Integrity("decryption failed".into()))?;
        unpack(&packed)
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.chunk_id.zeroize();
        self.content.zeroize();
    }
}

fn aad(doc: DocId, kind: Obj, id: &[u8]) -> Vec<u8> {
    let mut a = Vec::with_capacity(16 + 1 + id.len());
    a.extend_from_slice(&doc.0);
    a.push(kind as u8);
    a.extend_from_slice(id);
    a
}

const RAW: u8 = 0;
const ZSTD: u8 = 1;

/// zstd level 3, unless it doesn't shrink.
fn pack(plain: &[u8]) -> Vec<u8> {
    let z = zstd::bulk::compress(plain, 3).unwrap_or_default();
    let mut out = Vec::with_capacity(1 + plain.len().min(z.len()));
    if !z.is_empty() && z.len() < plain.len() {
        out.push(ZSTD);
        out.extend_from_slice(&z);
    } else {
        out.push(RAW);
        out.extend_from_slice(plain);
    }
    out
}

fn unpack(packed: &[u8]) -> Result<Vec<u8>> {
    match packed.split_first() {
        Some((&RAW, rest)) => Ok(rest.to_vec()),
        Some((&ZSTD, rest)) => {
            zstd::stream::decode_all(rest).map_err(|e| HistoryError::Integrity(format!("decompression failed: {e}")))
        }
        _ => Err(HistoryError::Integrity("unknown object format".into())),
    }
}

/// Non-Windows builds (engine tests on macOS/Linux) have no DPAPI: the store refuses to make or open a key.
#[cfg(not(windows))]
pub fn protect(_key: &[u8; 32]) -> Result<Vec<u8>> {
    Err(HistoryError::Dpapi)
}

#[cfg(not(windows))]
pub fn unprotect(_blob: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    Err(HistoryError::Dpapi)
}

#[cfg(windows)]
const ENTROPY: &[u8] = b"inkwell history key v1";

/// Wraps a key for the current Windows user (no UI).
#[cfg(windows)]
pub fn protect(key: &[u8; 32]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData};
    let input = CRYPT_INTEGER_BLOB { cbData: 32, pbData: key.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: all blobs point at live buffers; `out` is allocated by DPAPI and freed below.
    let ok = unsafe {
        CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    take_blob(ok, out).map(|v| v.to_vec())
}

#[cfg(windows)]
pub fn unprotect(blob: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: as in `protect`.
    let ok = unsafe {
        CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    let v = take_blob(ok, out)?;
    let k: [u8; 32] = v.as_slice().try_into().map_err(|_| HistoryError::Dpapi)?;
    Ok(Zeroizing::new(k))
}

#[cfg(windows)]
fn take_blob(ok: windows_sys::core::BOOL, out: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Result<Zeroizing<Vec<u8>>> {
    if ok == 0 || out.pbData.is_null() {
        return Err(HistoryError::Dpapi);
    }
    // SAFETY: DPAPI returned `cbData` bytes at `pbData`, allocated with LocalAlloc.
    unsafe {
        let v = Zeroizing::new(std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec());
        std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        windows_sys::Win32::Foundation::LocalFree(out.pbData as _);
        Ok(v)
    }
}
