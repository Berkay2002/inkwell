//! A small content-stream lexer: operators with their operands and byte spans, enough to follow
//! marked content (BMC/BDC/EMC), text positioning, the CTM and painting, and to rewrite a stream
//! by inserting text at operator boundaries (autotag).

use mupdf::Matrix;

/// An operand, kept only as far as the checks need it.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    Num(f32),
    Name(Vec<u8>),
    Dict(Vec<(Vec<u8>, Operand)>),
    /// A literal or hex string, decoded to its bytes.
    Str(Vec<u8>),
    Array(Vec<Operand>),
    Other,
}

impl Operand {
    pub fn num(&self) -> f32 {
        if let Operand::Num(n) = self { *n } else { 0.0 }
    }

    pub fn name(&self) -> Option<&[u8]> {
        if let Operand::Name(n) = self { Some(n) } else { None }
    }

    pub fn str(&self) -> Option<&[u8]> {
        if let Operand::Str(s) = self { Some(s) } else { None }
    }

    pub fn get(&self, key: &[u8]) -> Option<&Operand> {
        if let Operand::Dict(d) = self { d.iter().find(|(k, _)| k == key).map(|(_, v)| v) } else { None }
    }
}

/// One operator. `start` is where its first operand begins (or the operator, without operands),
/// `end` just past the operator; an inline image (`BI ... ID data EI`) is one op named `BI`.
#[derive(Clone, Debug)]
pub struct Op {
    pub name: Vec<u8>,
    pub operands: Vec<Operand>,
    pub start: usize,
    pub end: usize,
}

impl Op {
    pub fn is(&self, n: &[u8]) -> bool {
        self.name == n
    }

    pub fn n(&self, i: usize) -> f32 {
        self.operands.get(i).map_or(0.0, Operand::num)
    }
}

fn white(c: u8) -> bool {
    matches!(c, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

/// Nesting deeper than this in arrays and dictionaries is read as one opaque operand.
const MAX_NEST: usize = 64;

impl Lexer<'_> {
    fn skip_space(&mut self) {
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if white(c) {
                self.i += 1;
            } else if c == b'%' {
                while self.i < self.s.len() && !matches!(self.s[self.i], b'\n' | b'\r') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn regular(&mut self) -> &[u8] {
        let a = self.i;
        while self.i < self.s.len() && !white(self.s[self.i]) && !delim(self.s[self.i]) {
            self.i += 1;
        }
        &self.s[a..self.i]
    }

    fn name(&mut self) -> Vec<u8> {
        self.i += 1; // '/'
        let raw = self.regular().to_vec();
        let mut out = Vec::with_capacity(raw.len());
        let mut k = 0;
        while k < raw.len() {
            if raw[k] == b'#'
                && k + 3 <= raw.len()
                && let Some(v) = std::str::from_utf8(&raw[k + 1..k + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                k += 3;
                continue;
            }
            out.push(raw[k]);
            k += 1;
        }
        out
    }

    fn literal(&mut self) -> Vec<u8> {
        self.i += 1;
        let mut depth = 1;
        let mut out = Vec::new();
        while self.i < self.s.len() && depth > 0 {
            let c = self.s[self.i];
            match c {
                b'\\' => {
                    self.i += 1;
                    match self.s.get(self.i).copied() {
                        Some(b'n') => out.push(b'\n'),
                        Some(b'r') => out.push(b'\r'),
                        Some(b't') => out.push(b'\t'),
                        Some(b'b') => out.push(8),
                        Some(b'f') => out.push(12),
                        Some(d @ b'0'..=b'7') => {
                            let mut v = u32::from(d - b'0');
                            for _ in 0..2 {
                                match self.s.get(self.i + 1) {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        // A backslash before a line break continues the string on the next line.
                        Some(b'\r') => {
                            if self.s.get(self.i + 1) == Some(&b'\n') {
                                self.i += 1;
                            }
                        }
                        Some(b'\n') | None => {}
                        Some(o) => out.push(o),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth > 0 {
                        out.push(c);
                    }
                }
                _ => out.push(c),
            }
            self.i += 1;
        }
        out
    }

    fn hex(&mut self) -> Vec<u8> {
        self.i += 1;
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while self.i < self.s.len() && self.s[self.i] != b'>' {
            if let Some(d) = (self.s[self.i] as char).to_digit(16) {
                match hi.take() {
                    Some(h) => out.push(h << 4 | d as u8),
                    None => hi = Some(d as u8),
                }
            }
            self.i += 1;
        }
        if let Some(h) = hi {
            out.push(h << 4);
        }
        self.i += 1;
        out
    }

    /// One operand, or `Err(keyword)` when the next token is an operator. `None` at the end.
    fn token(&mut self, depth: usize) -> Option<Result<Operand, Vec<u8>>> {
        self.skip_space();
        let c = *self.s.get(self.i)?;
        Some(match c {
            b'/' => Ok(Operand::Name(self.name())),
            b'(' => Ok(Operand::Str(self.literal())),
            b'<' if self.s.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                let mut d = Vec::new();
                loop {
                    self.skip_space();
                    match self.s.get(self.i) {
                        None => break,
                        Some(b'>') => {
                            self.i += 2;
                            break;
                        }
                        Some(b'/') => {
                            let k = self.name();
                            match self.token(depth + 1) {
                                Some(Ok(v)) if depth < MAX_NEST => d.push((k, v)),
                                Some(_) => {}
                                None => break,
                            }
                        }
                        Some(_) => {
                            // Malformed: skip a token.
                            if self.token(depth + 1).is_none() {
                                break;
                            }
                        }
                    }
                }
                Ok(Operand::Dict(d))
            }
            b'<' => Ok(Operand::Str(self.hex())),
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    match self.s.get(self.i) {
                        None => break,
                        Some(b']') => {
                            self.i += 1;
                            break;
                        }
                        Some(_) => match self.token(depth + 1) {
                            Some(Ok(v)) if depth < MAX_NEST => items.push(v),
                            Some(_) => {}
                            None => break,
                        },
                    }
                }
                Ok(Operand::Array(items))
            }
            b')' | b'>' | b']' | b'{' | b'}' => {
                self.i += 1;
                Ok(Operand::Other)
            }
            _ => {
                let t = self.regular();
                if t.is_empty() {
                    self.i += 1;
                    return Some(Ok(Operand::Other));
                }
                if matches!(t[0], b'0'..=b'9' | b'+' | b'-' | b'.') {
                    Ok(std::str::from_utf8(t).ok().and_then(|s| s.parse::<f32>().ok()).map_or(Operand::Other, Operand::Num))
                } else if matches!(t, b"true" | b"false" | b"null") {
                    Ok(Operand::Other)
                } else {
                    Err(t.to_vec())
                }
            }
        })
    }

    /// Skips inline image data after `ID` up to and including `EI`.
    fn inline_image(&mut self) {
        // One whitespace byte follows ID.
        self.i += 1;
        while self.i + 2 <= self.s.len() {
            if self.s[self.i] == b'E'
                && self.s[self.i + 1] == b'I'
                && white(self.s[self.i - 1])
                && self.s.get(self.i + 2).is_none_or(|c| white(*c) || delim(*c))
            {
                self.i += 2;
                return;
            }
            self.i += 1;
        }
        self.i = self.s.len();
    }
}

/// Every operator of `data`, in order.
pub fn parse(data: &[u8]) -> Vec<Op> {
    let mut lx = Lexer { s: data, i: 0 };
    let mut ops = Vec::new();
    let mut operands = Vec::new();
    let mut start = None;
    loop {
        lx.skip_space();
        let at = lx.i;
        let Some(t) = lx.token(0) else { break };
        match t {
            Ok(o) => {
                start.get_or_insert(at);
                operands.push(o);
            }
            Err(name) => {
                let s = start.take().unwrap_or(at);
                if name == b"BI" {
                    // Dictionary operands up to ID, then the data.
                    loop {
                        lx.skip_space();
                        match lx.token(0) {
                            Some(Err(k)) if k == b"ID" => {
                                lx.inline_image();
                                break;
                            }
                            Some(_) => {}
                            None => break,
                        }
                    }
                    operands.clear();
                    ops.push(Op { name, operands: Vec::new(), start: s, end: lx.i });
                    continue;
                }
                ops.push(Op { name, operands: std::mem::take(&mut operands), start: s, end: lx.i });
            }
        }
    }
    ops
}

pub fn matrix(op: &Op) -> Matrix {
    Matrix::new(op.n(0), op.n(1), op.n(2), op.n(3), op.n(4), op.n(5))
}

/// Text-showing operators.
pub fn shows(op: &Op) -> bool {
    matches!(op.name.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"")
}

/// Path-painting operators (`n` ends a path without painting).
pub fn paints_path(op: &Op) -> bool {
    matches!(op.name.as_slice(), b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*")
}

pub fn builds_path(op: &Op) -> bool {
    matches!(op.name.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
}

/// The text state needed to know where a show operator starts.
#[derive(Clone)]
pub struct TextPos {
    pub tm: Matrix,
    pub tlm: Matrix,
    pub leading: f32,
}

impl Default for TextPos {
    fn default() -> Self {
        Self { tm: Matrix::IDENTITY, tlm: Matrix::IDENTITY, leading: 0.0 }
    }
}

impl TextPos {
    fn next_line(&mut self, tx: f32, ty: f32) {
        self.tlm = Matrix::new_translate(tx, ty) * &self.tlm;
        self.tm = self.tlm.clone();
    }

    /// Follows a text operator (positioning and the line moves of `'` and `"`); call before
    /// reading `tm` for a show operator.
    pub fn apply(&mut self, op: &Op) {
        match op.name.as_slice() {
            b"BT" => *self = TextPos { leading: self.leading, ..TextPos::default() },
            b"Td" => self.next_line(op.n(0), op.n(1)),
            b"TD" => {
                self.leading = -op.n(1);
                self.next_line(op.n(0), op.n(1));
            }
            b"Tm" => {
                self.tlm = matrix(op);
                self.tm = self.tlm.clone();
            }
            b"T*" | b"'" | b"\"" => self.next_line(0.0, -self.leading),
            b"TL" => self.leading = op.n(0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_operators_operands_and_inline_images() {
        let s = b"q 1 0 0 1 10 20 cm /P <</MCID 3 /Lang (en)>> BDC BT /F1 12 Tf [(a) -20 (b)] TJ ET EMC\nBI /W 2 /H 1 /BPC 8 /CS /G ID \x00EI\xff EI Q % note\n/Im#201 Do";
        let ops = parse(s);
        let names: Vec<&[u8]> = ops.iter().map(|o| o.name.as_slice()).collect();
        assert_eq!(names, [&b"q"[..], b"cm", b"BDC", b"BT", b"Tf", b"TJ", b"ET", b"EMC", b"BI", b"Q", b"Do"]);
        let bdc = &ops[2];
        assert_eq!(bdc.operands[0].name(), Some(&b"P"[..]));
        assert_eq!(bdc.operands[1].get(b"MCID"), Some(&Operand::Num(3.0)));
        assert_eq!(&s[bdc.start..bdc.end], b"/P <</MCID 3 /Lang (en)>> BDC");
        assert_eq!(&s[ops[8].start..ops[8].end], b"BI /W 2 /H 1 /BPC 8 /CS /G ID \x00EI\xff EI");
        assert_eq!(ops[10].operands[0].name(), Some(&b"Im 1"[..]));
    }

    #[test]
    fn strings_and_arrays_keep_their_bytes() {
        let ops = parse(b"[(a\\(b\\) \\101\\n) -250 <48 65 6C6c6F> (nested (par) ens)] TJ <4>Tj");
        let Operand::Array(items) = &ops[0].operands[0] else { panic!("{:?}", ops[0]) };
        assert_eq!(items[0].str(), Some(&b"a(b) A\n"[..]));
        assert_eq!(items[1], Operand::Num(-250.0));
        assert_eq!(items[2].str(), Some(&b"Hello"[..]));
        assert_eq!(items[3].str(), Some(&b"nested (par) ens"[..]));
        assert_eq!(ops[1].operands[0].str(), Some(&[0x40u8][..]), "an odd hex digit is padded with 0");
    }
}
