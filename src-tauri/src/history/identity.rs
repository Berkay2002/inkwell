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

/// The file id, only where it is stable and unique: NTFS and ReFS. FAT/exFAT, SMB shares and cloud
/// drives can return 0 or reuse ids, so there the caller falls back to content, /ID and path.
/// Non-Windows builds (engine tests on macOS/Linux): no stable id, callers fall back.
#[cfg(not(windows))]
pub fn file_id(_path: &Path) -> Option<FileId> {
    None
}

#[cfg(windows)]
pub fn file_id(path: &Path) -> Option<FileId> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        GetVolumeInformationByHandleW,
    };
    let f = std::fs::File::open(path).ok()?;
    let h = f.as_raw_handle() as _;
    // SAFETY: `h` is a live handle for the duration of the calls; the out structs are plain data.
    let id = unsafe {
        let mut fs = [0u16; 64];
        let n = std::ptr::null_mut();
        if GetVolumeInformationByHandleW(h, n, 0, n as _, n as _, n as _, fs.as_mut_ptr(), fs.len() as u32) == 0 {
            return None;
        }
        let len = fs.iter().position(|&c| c == 0).unwrap_or(fs.len());
        if !stable_ids(&String::from_utf16_lossy(&fs[..len])) {
            return None;
        }
        let mut info: FILE_ID_INFO = std::mem::zeroed();
        let mut bh: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandleEx(h, FileIdInfo, &mut info as *mut _ as _, std::mem::size_of::<FILE_ID_INFO>() as u32) != 0 {
            FileId { volume: info.VolumeSerialNumber, id: u128::from_le_bytes(info.FileId.Identifier) }
        } else if GetFileInformationByHandle(h, &mut bh) != 0 {
            let idx = ((bh.nFileIndexHigh as u64) << 32) | bh.nFileIndexLow as u64;
            FileId { volume: bh.dwVolumeSerialNumber as u64, id: idx as u128 }
        } else {
            return None;
        }
    };
    usable(id)
}

/// Filesystems whose file ids survive renames and are not reused while the file exists.
#[cfg(windows)]
pub fn stable_ids(fs_name: &str) -> bool {
    fs_name.eq_ignore_ascii_case("NTFS") || fs_name.eq_ignore_ascii_case("ReFS")
}

/// A zero id is "no id" (some drivers return it for every file).
pub fn usable(f: FileId) -> Option<FileId> {
    (f.id != 0).then_some(f)
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
