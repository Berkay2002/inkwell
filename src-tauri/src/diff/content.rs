//! A small content-stream tokenizer: which named resources a stream paints with, and whether it
//! contains invisible text (`3 Tr`, the OCR text layer). Not a full interpreter: operands are not
//! evaluated, only the last name before an operator is remembered.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResCat {
    Font,
    XObject,
    ExtGState,
    ColorSpace,
    Pattern,
    Shading,
    Properties,
}

impl ResCat {
    /// Key of the sub-dictionary in /Resources.
    pub fn key(self) -> &'static str {
        match self {
            ResCat::Font => "Font",
            ResCat::XObject => "XObject",
            ResCat::ExtGState => "ExtGState",
            ResCat::ColorSpace => "ColorSpace",
            ResCat::Pattern => "Pattern",
            ResCat::Shading => "Shading",
            ResCat::Properties => "Properties",
        }
    }
}

#[derive(Default, Debug)]
pub struct Scan {
    /// Sorted, deduplicated.
    pub used: Vec<(ResCat, Vec<u8>)>,
    pub invisible_text: bool,
}

fn is_ws(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

pub fn scan(data: &[u8]) -> Scan {
    let mut s = Scan::default();
    let n = data.len();
    let mut i = 0;
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut prev_num: Option<f64> = None;
    while i < n {
        let c = data[i];
        if is_ws(c) {
            i += 1;
        } else if c == b'%' {
            while i < n && data[i] != b'\n' && data[i] != b'\r' {
                i += 1;
            }
        } else if c == b'(' {
            let mut depth = 1;
            i += 1;
            while i < n && depth > 0 {
                match data[i] {
                    b'\\' => i += 1,
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            prev_num = None;
        } else if c == b'<' {
            if i + 1 < n && data[i + 1] == b'<' {
                i += 2;
            } else {
                while i < n && data[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
            prev_num = None;
        } else if c == b'/' {
            let st = i + 1;
            i = st;
            while i < n && !is_ws(data[i]) && !is_delim(data[i]) {
                i += 1;
            }
            names.push(decode_name(&data[st..i]));
            prev_num = None;
        } else if is_delim(c) {
            i += 1; // ) > [ ] { }
        } else {
            let st = i;
            while i < n && !is_ws(data[i]) && !is_delim(data[i]) {
                i += 1;
            }
            let tok = &data[st..i];
            if let Some(v) = std::str::from_utf8(tok).ok().and_then(|t| t.parse::<f64>().ok()) {
                prev_num = Some(v);
                continue;
            }
            // An operator.
            let last = names.last().cloned();
            let mut push = |cat: ResCat, nm: Option<Vec<u8>>| {
                if let Some(nm) = nm {
                    s.used.push((cat, nm));
                }
            };
            match tok {
                b"Tf" => push(ResCat::Font, last),
                b"Do" => push(ResCat::XObject, last),
                b"gs" => push(ResCat::ExtGState, last),
                b"cs" | b"CS" => push(ResCat::ColorSpace, last),
                b"scn" | b"SCN" => push(ResCat::Pattern, last),
                b"sh" => push(ResCat::Shading, last),
                b"BDC" | b"DP" => push(ResCat::Properties, last),
                b"Tr" if prev_num == Some(3.0) => s.invisible_text = true,
                b"BI" => {
                    // Inline image: skip to "EI" delimited by whitespace.
                    while i < n {
                        if data[i] == b'E'
                            && i + 1 < n
                            && data[i + 1] == b'I'
                            && i > 0
                            && is_ws(data[i - 1])
                            && (i + 2 >= n || is_ws(data[i + 2]))
                        {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            names.clear();
            prev_num = None;
        }
    }
    s.used.sort();
    s.used.dedup();
    s
}

/// Hash of what a stream paints besides text: everything outside `BT .. ET`, minus the text-state operators that may sit
/// outside it (`Tf Tc Tw Tz TL Tr Ts` and the showing and positioning ones). With `skip_colour` the colour operators go too, for comparing two pages whose
/// text was recoloured (the colour is often set just before the `BT`).
pub fn graphics_hash(data: &[u8], skip_colour: bool) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    let n = data.len();
    let (mut i, mut start) = (0, 0);
    let mut in_text = false;
    while i < n {
        let c = data[i];
        if is_ws(c) {
            i += 1;
        } else if c == b'%' {
            while i < n && data[i] != b'\n' && data[i] != b'\r' {
                i += 1;
            }
        } else if c == b'(' {
            let mut depth = 1;
            i += 1;
            while i < n && depth > 0 {
                match data[i] {
                    b'\\' => i += 1,
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
        } else if c == b'<' {
            if i + 1 < n && data[i + 1] == b'<' {
                i += 2;
            } else {
                while i < n && data[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
        } else if c == b'/' {
            i += 1;
            while i < n && !is_ws(data[i]) && !is_delim(data[i]) {
                i += 1;
            }
        } else if is_delim(c) {
            i += 1;
        } else {
            let st = i;
            while i < n && !is_ws(data[i]) && !is_delim(data[i]) {
                i += 1;
            }
            let tok = &data[st..i];
            if tok.first().is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.')) {
                continue; // an operand
            }
            if tok == b"BI" {
                while i < n {
                    if data[i] == b'E' && i + 1 < n && data[i + 1] == b'I' && i > 0 && is_ws(data[i - 1]) && (i + 2 >= n || is_ws(data[i + 2])) {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            let text_state = matches!(tok, b"Tf" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tr" | b"Ts" | b"Tj" | b"TJ" | b"Td" | b"TD" | b"Tm" | b"T*" | b"'" | b"\"");
            let colour = matches!(tok, b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" | b"cs" | b"CS" | b"sc" | b"scn" | b"SC" | b"SCN");
            match tok {
                b"BT" => in_text = true,
                b"ET" => in_text = false,
                _ if in_text || text_state || (skip_colour && colour) => {}
                _ => {
                    h.update(&data[start..i]);
                    h.update(b"\n");
                }
            }
            start = i;
        }
    }
    *h.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_hash_ignores_text_and_optionally_colour() {
        let a = b"0 g 10 10 50 50 re f 1 0 0 rg BT /F1 12 Tf 72 700 Td (Hello) Tj ET 0 g";
        let b = b"0 g 10 10 50 50 re f 0 0 1 rg BT /F2 16 Tf 80 650 Td (World there) Tj ET 0 g";
        assert_ne!(graphics_hash(a, false), graphics_hash(b, false), "a colour set outside BT is graphics state");
        assert_eq!(graphics_hash(a, true), graphics_hash(b, true));
        let c = b"0 g 10 10 60 50 re f 1 0 0 rg BT /F1 12 Tf 72 700 Td (Hello) Tj ET 0 g";
        assert_ne!(graphics_hash(a, true), graphics_hash(c, true), "a moved rectangle is a graphics change");
    }

    #[test]
    fn finds_resources_and_invisible_text() {
        let s = scan(b"q /GS0 gs BT /F1 12 Tf 3 Tr (a /Fake Tf) Tj ET /Im0 Do % /C Do\n/P1 /Properties_x BDC");
        assert!(s.invisible_text);
        assert!(s.used.contains(&(ResCat::Font, b"F1".to_vec())));
        assert!(s.used.contains(&(ResCat::ExtGState, b"GS0".to_vec())));
        assert!(s.used.contains(&(ResCat::XObject, b"Im0".to_vec())));
        assert!(!s.used.iter().any(|(_, n)| n == b"Fake" || n == b"C"));
        assert!(s.used.contains(&(ResCat::Properties, b"Properties_x".to_vec())));
        let s = scan(b"BT 0 Tr /F1 9 Tf ET BI /W 1 /H 1 ID \x00 3 Tr EI /Im1 Do");
        assert!(!s.invisible_text);
        assert!(s.used.contains(&(ResCat::XObject, b"Im1".to_vec())));
    }
}
