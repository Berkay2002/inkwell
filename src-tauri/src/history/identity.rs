//! What identifies a file across renames and moves: the NTFS file id + volume serial, the PDF
//! trailer /ID, and the content.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// NTFS file id (128-bit, or the 64-bit index as fallback) on a volume. Not sensitive.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileId {
    pub volume: u64,
    pub id: u128,
}

pub fn file_id(path: &Path) -> Option<FileId> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
    };
    let f = std::fs::File::open(path).ok()?;
    let h = f.as_raw_handle() as _;
    // SAFETY: `h` is a live handle for the duration of the calls; the out structs are plain data.
    unsafe {
        let mut info: FILE_ID_INFO = std::mem::zeroed();
        if GetFileInformationByHandleEx(h, FileIdInfo, &mut info as *mut _ as _, std::mem::size_of::<FILE_ID_INFO>() as u32) != 0 {
            return Some(FileId { volume: info.VolumeSerialNumber, id: u128::from_le_bytes(info.FileId.Identifier) });
        }
        let mut bh: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(h, &mut bh) != 0 {
            let idx = ((bh.nFileIndexHigh as u64) << 32) | bh.nFileIndexLow as u64;
            return Some(FileId { volume: bh.dwVolumeSerialNumber as u64, id: idx as u128 });
        }
    }
    None
}

/// The first (permanent) element of the last trailer's /ID, raw string bytes. Works for classic
/// trailers and xref streams (their dictionary is not compressed).
pub fn pdf_id(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut end = bytes.len();
    while let Some(at) = rfind(&bytes[..end], b"/ID") {
        end = at;
        let mut i = at + 3;
        skip_ws(bytes, &mut i);
        if bytes.get(i) != Some(&b'[') {
            continue;
        }
        i += 1;
        skip_ws(bytes, &mut i);
        if let Some(s) = pdf_string(bytes, i) {
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

fn rfind(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).rposition(|w| w == n)
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while b.get(*i).is_some_and(|c| c.is_ascii_whitespace() || *c == 0) {
        *i += 1;
    }
}

fn pdf_string(b: &[u8], i: usize) -> Option<Vec<u8>> {
    match b.get(i)? {
        b'<' => {
            let end = i + 1 + b.get(i + 1..)?.iter().position(|&c| c == b'>')?;
            let hex: Vec<u8> = b[i + 1..end].iter().copied().filter(|c| c.is_ascii_hexdigit()).collect();
            let mut out = Vec::with_capacity(hex.len().div_ceil(2));
            for pair in hex.chunks(2) {
                let hi = (pair[0] as char).to_digit(16)?;
                let lo = pair.get(1).map_or(Some(0), |c| (*c as char).to_digit(16))?;
                out.push((hi * 16 + lo) as u8);
            }
            Some(out)
        }
        b'(' => {
            // Raw bytes up to the matching paren; escapes kept as-is (only equality matters).
            let (mut depth, mut j, mut out) = (1, i + 1, Vec::new());
            while let Some(&c) = b.get(j) {
                match c {
                    b'\\' => {
                        out.push(c);
                        j += 1;
                        out.push(*b.get(j)?);
                    }
                    b'(' => {
                        depth += 1;
                        out.push(c);
                    }
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(out);
                        }
                        out.push(c);
                    }
                    _ => out.push(c),
                }
                j += 1;
            }
            None
        }
        _ => None,
    }
}

/// Lower-cased absolute path, so case and `\\?\`-prefix differences don't change the hash.
pub fn normalized_path(path: &Path) -> String {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_lowercase()
}
