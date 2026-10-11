// Adapted from pdfcraft (MIT OR Apache-2.0), crates/content/src/lib.rs @ b8ddf71
//! Content streams (ISO 32000-2 §7.8.2): parse a stream into operators that keep their source
//! spans, write operators back, splice a page's streams, and the matrix arithmetic content
//! interpretation needs.
//!
//! The parser is tolerant: bytes it cannot read are skipped instead of failing the stream, and it
//! never panics. Inline images (`BI … ID … EI`) are kept whole. pdfcraft parses objects with its
//! own COS crate; Inkwell has no COS layer besides MuPDF, so the small object model and lexer
//! needed for operands live here.

/// An operand.
#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Vec<(Vec<u8>, Obj)>),
}

impl Obj {
    pub fn name(s: &str) -> Obj {
        Obj::Name(s.as_bytes().to_vec())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(*i as f64),
            Obj::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
}

/// One operator with its operands.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    /// The operator keyword (`Tj`, `re`, `BI` for inline images…).
    pub op: Vec<u8>,
    pub operands: Vec<Obj>,
    /// The bytes of the source stream this operator came from (operands included).
    pub span: std::ops::Range<usize>,
}

impl Op {
    pub fn new(op: &str, operands: Vec<Obj>) -> Self {
        Op { op: op.as_bytes().to_vec(), operands, span: 0..0 }
    }

    /// An operator written back exactly as `bytes` (an original operator's source, e.g. an
    /// inline image with its parameters and data, which the parsed form does not keep).
    pub fn verbatim(bytes: &[u8]) -> Self {
        Op { op: bytes.to_vec(), operands: Vec::new(), span: 0..0 }
    }

    pub fn is(&self, op: &str) -> bool {
        self.op == op.as_bytes()
    }

    pub fn num(&self, i: usize) -> Option<f64> {
        self.operands.get(i).and_then(Obj::as_f64)
    }

    /// The last `n` operands as numbers (operators take their operands from the end).
    pub fn nums<const N: usize>(&self) -> Option<[f64; N]> {
        let k = self.operands.len().checked_sub(N)?;
        let mut out = [0.0; N];
        for (o, v) in out.iter_mut().zip(&self.operands[k..]) {
            *o = v.as_f64()?;
        }
        Some(out)
    }

    pub fn name(&self, i: usize) -> Option<&[u8]> {
        self.operands.get(i).and_then(Obj::as_name)
    }
}

fn is_ws(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Nesting limit for arrays and dictionaries in operands.
const MAX_DEPTH: usize = 32;

struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn at(&self, i: usize) -> Option<u8> {
        self.data.get(i).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.at(self.pos) {
            if is_ws(b) {
                self.pos += 1;
            } else if b == b'%' {
                while self.at(self.pos).is_some_and(|c| c != b'\n' && c != b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// A run of regular characters.
    fn token(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.at(self.pos).is_some_and(|b| !is_ws(b) && !is_delim(b)) {
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    fn object(&mut self, depth: usize) -> Option<Obj> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.skip_ws();
        let b = self.at(self.pos)?;
        match b {
            b'/' => {
                self.pos += 1;
                let raw = self.token();
                Some(Obj::Name(unescape_name(raw)))
            }
            b'(' => self.literal().map(Obj::Str),
            b'<' if self.at(self.pos + 1) == Some(b'<') => {
                self.pos += 2;
                let mut d = Vec::new();
                loop {
                    self.skip_ws();
                    match self.at(self.pos)? {
                        b'>' if self.at(self.pos + 1) == Some(b'>') => {
                            self.pos += 2;
                            return Some(Obj::Dict(d));
                        }
                        b'/' => {
                            let Obj::Name(k) = self.object(depth + 1)? else {
                                return None;
                            };
                            let v = self.object(depth + 1)?;
                            d.push((k, v));
                        }
                        _ => return None,
                    }
                }
            }
            b'<' => {
                self.pos += 1;
                let mut digits = Vec::new();
                loop {
                    let c = self.at(self.pos)?;
                    self.pos += 1;
                    match c {
                        b'>' => break,
                        c if is_ws(c) => {}
                        c => digits.push((c as char).to_digit(16)? as u8),
                    }
                }
                if digits.len() % 2 == 1 {
                    digits.push(0);
                }
                Some(Obj::Str(digits.chunks(2).map(|p| p[0] << 4 | p[1]).collect()))
            }
            b'[' => {
                self.pos += 1;
                let mut a = Vec::new();
                loop {
                    self.skip_ws();
                    match self.at(self.pos)? {
                        b']' => {
                            self.pos += 1;
                            return Some(Obj::Array(a));
                        }
                        _ => {
                            let at = self.pos;
                            match self.object(depth + 1) {
                                Some(o) => a.push(o),
                                None => {
                                    // Keywords inside arrays (true, false, null) or junk.
                                    self.pos = at;
                                    let t = self.token();
                                    match t {
                                        b"true" => a.push(Obj::Bool(true)),
                                        b"false" => a.push(Obj::Bool(false)),
                                        b"null" => a.push(Obj::Null),
                                        _ => return None,
                                    }
                                }
                            }
                        }
                    }
                }
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                let t = self.token();
                let s = std::str::from_utf8(t).ok()?;
                if let Ok(i) = s.parse::<i64>() {
                    return Some(Obj::Int(i));
                }
                // PDF reals have no exponent; tolerate "--5" and "5." style writers.
                let s = s.trim_start_matches("--");
                s.parse::<f64>().ok().filter(|v| v.is_finite()).map(Obj::Real).or(Some(Obj::Int(0)))
            }
            _ => None,
        }
    }

    /// A `( … )` string with its escapes resolved; None when unterminated.
    fn literal(&mut self) -> Option<Vec<u8>> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut depth = 1usize;
        loop {
            let c = self.at(self.pos)?;
            self.pos += 1;
            match c {
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
                b'\\' => {
                    let e = self.at(self.pos)?;
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'\r' => {
                            if self.at(self.pos) == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.at(self.pos) {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xFF) as u8);
                        }
                        other => out.push(other),
                    }
                }
                _ => out.push(c),
            }
        }
    }
}

fn unescape_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 3 <= raw.len() {
            let h = std::str::from_utf8(&raw[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = h {
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

/// Parse a (decoded) content stream.
pub fn parse(data: &[u8]) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut lx = Lexer { data, pos: 0 };
    let mut operands: Vec<Obj> = Vec::new();
    let mut start: Option<usize> = None;
    loop {
        lx.skip_ws();
        let Some(b) = lx.at(lx.pos) else { break };
        let at = lx.pos;
        let operand_start = matches!(b, b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.') || b.is_ascii_digit();
        if operand_start {
            match lx.object(0) {
                Some(o) => {
                    start.get_or_insert(at);
                    operands.push(o);
                }
                // An unterminated string runs to the end of the stream.
                None if b == b'(' => lx.pos = data.len(),
                None => lx.pos = at + 1,
            }
            continue;
        }
        if is_delim(b) {
            lx.pos += 1;
            continue;
        }
        let kw = lx.token();
        if kw.is_empty() {
            lx.pos = at + 1;
            continue;
        }
        match kw {
            b"true" | b"false" | b"null" => {
                start.get_or_insert(at);
                operands.push(match kw {
                    b"true" => Obj::Bool(true),
                    b"false" => Obj::Bool(false),
                    _ => Obj::Null,
                });
            }
            b"BI" => {
                let end = inline_image_end(data, lx.pos);
                lx.pos = end;
                ops.push(Op { op: b"BI".to_vec(), operands: Vec::new(), span: start.take().unwrap_or(at)..end });
                operands.clear();
            }
            _ => {
                let s = start.take().unwrap_or(at);
                ops.push(Op { op: kw.to_vec(), operands: std::mem::take(&mut operands), span: s..lx.pos });
            }
        }
    }
    ops
}

/// The parameters of an inline image (the bytes of its `BI` operator), with abbreviated keys
/// expanded (`W` → `Width`, `CS` → `ColorSpace`, …; ISO 32000-2 Table 91).
pub fn inline_dict(data: &[u8]) -> Vec<(Vec<u8>, Obj)> {
    let start = data.windows(2).position(|w| w == b"BI").map_or(0, |p| p + 2);
    let mut lx = Lexer { data, pos: start };
    let mut out = Vec::new();
    loop {
        lx.skip_ws();
        if data.get(lx.pos..lx.pos + 2) == Some(b"ID") || lx.pos >= data.len() {
            break;
        }
        let Some(Obj::Name(k)) = lx.object(0) else { break };
        lx.skip_ws();
        let v = match lx.object(0) {
            Some(v) => v,
            None => match lx.token() {
                b"true" => Obj::Bool(true),
                b"false" => Obj::Bool(false),
                _ => break,
            },
        };
        let key: &[u8] = match k.as_slice() {
            b"W" => b"Width",
            b"H" => b"Height",
            b"CS" => b"ColorSpace",
            b"BPC" => b"BitsPerComponent",
            b"F" => b"Filter",
            b"DP" => b"DecodeParms",
            b"IM" => b"ImageMask",
            b"D" => b"Decode",
            b"I" => b"Interpolate",
            other => other,
        };
        out.push((key.to_vec(), v));
    }
    out
}

/// Where an inline image that starts after `BI` ends (after its `EI`).
fn inline_image_end(data: &[u8], pos: usize) -> usize {
    let mut i = pos;
    // Past the `ID` keyword.
    while i + 1 < data.len() {
        if data[i] == b'I' && data[i + 1] == b'D' && (i == 0 || is_ws(data[i - 1]) || is_delim(data[i - 1])) {
            break;
        }
        i += 1;
    }
    let begin = (i + 3).min(data.len());
    let mut j = begin;
    while j + 2 <= data.len() {
        if data[j] == b'E'
            && data[j + 1] == b'I'
            && (j == begin || is_ws(data[j - 1]))
            && data.get(j + 2).is_none_or(|b| is_ws(*b) || is_delim(*b))
        {
            return j + 2;
        }
        j += 1;
    }
    data.len()
}

/// A number operand, written as an integer when it is one.
pub fn num(v: f64) -> Obj {
    if v.fract() == 0.0 && v.abs() < 1e15 { Obj::Int(v as i64) } else { Obj::Real((v * 10_000.0).round() / 10_000.0) }
}

/// Serialise one operand.
pub fn write_obj(o: &Obj, out: &mut Vec<u8>) {
    match o {
        Obj::Null => out.extend_from_slice(b"null"),
        Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Obj::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Obj::Real(r) => {
            let s = format!("{r:.4}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            out.extend_from_slice(if s.is_empty() || s == "-" || s == "-0" { b"0" } else { s.as_bytes() });
        }
        Obj::Name(n) => {
            out.push(b'/');
            for &b in n {
                if b.is_ascii_graphic() && !is_delim(b) && b != b'#' {
                    out.push(b);
                } else {
                    out.extend_from_slice(format!("#{b:02X}").as_bytes());
                }
            }
        }
        Obj::Str(s) => {
            out.push(b'<');
            for b in s {
                out.extend_from_slice(format!("{b:02X}").as_bytes());
            }
            out.push(b'>');
        }
        Obj::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_obj(x, out);
            }
            out.push(b']');
        }
        Obj::Dict(d) => {
            out.extend_from_slice(b"<<");
            for (k, v) in d {
                write_obj(&Obj::Name(k.clone()), out);
                out.push(b' ');
                write_obj(v, out);
                out.push(b' ');
            }
            out.extend_from_slice(b">>");
        }
    }
}

/// Write operators (one per line).
pub fn serialize_ops(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        for o in &op.operands {
            write_obj(o, &mut out);
            out.push(b' ');
        }
        out.extend_from_slice(&op.op);
        out.push(b'\n');
    }
    out
}

/// A page's content streams joined into the one stream they are. Writers may split it between
/// any two tokens (ISO 32000-2 §7.8.2), so the pieces are parsed joined and positions mapped
/// back with [`Pieces::piece_of`]. A newline separates the pieces so tokens never run together.
#[derive(Clone, Debug, Default)]
pub struct Pieces {
    data: Vec<u8>,
    /// Where each piece ends in `data`, after its separator.
    ends: Vec<usize>,
}

impl Pieces {
    pub fn join<D: AsRef<[u8]>>(pieces: &[D]) -> Self {
        let mut data = Vec::new();
        let mut ends = Vec::with_capacity(pieces.len());
        for p in pieces {
            data.extend_from_slice(p.as_ref());
            data.push(b'\n');
            ends.push(data.len());
        }
        Pieces { data, ends }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn parse(&self) -> Vec<Op> {
        parse(&self.data)
    }

    /// The piece holding joined position `at` (the last piece for positions past the end).
    pub fn piece_of(&self, at: usize) -> usize {
        self.ends.iter().position(|&e| at < e).unwrap_or(self.ends.len().saturating_sub(1))
    }

    /// The joined range of piece `i`, without its separator.
    fn range(&self, i: usize) -> std::ops::Range<usize> {
        let start = match i.checked_sub(1) {
            None => 0,
            Some(p) => self.ends.get(p).copied().unwrap_or(self.data.len()),
        };
        let end = self.ends.get(i).map_or(self.data.len(), |e| e.saturating_sub(1)).max(start);
        start..end
    }

    /// Every piece's bytes after replacing joined byte ranges: `edits` are (range, bytes) in
    /// ascending, non-overlapping order. A replacement goes into the piece its range starts in,
    /// and the rest of a range running on into later pieces is removed from them. Everything else
    /// is copied byte for byte (unchanged pieces come back equal).
    pub fn splice(&self, edits: impl IntoIterator<Item = (std::ops::Range<usize>, Vec<u8>)>) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = (0..self.len()).map(|i| Vec::with_capacity(self.range(i).len())).collect();
        let copy = |out: &mut Vec<Vec<u8>>, from: usize, to: usize| {
            for (i, piece) in out.iter_mut().enumerate() {
                let r = self.range(i);
                let (a, b) = (from.max(r.start), to.min(r.end));
                if a < b {
                    piece.extend_from_slice(&self.data[a..b]);
                }
            }
        };
        let mut at = 0;
        for (range, bytes) in edits {
            let start = range.start.clamp(at, self.data.len());
            copy(&mut out, at, start);
            if !bytes.is_empty() {
                if let Some(piece) = out.get_mut(self.piece_of(start)) {
                    piece.extend_from_slice(&bytes);
                }
            }
            at = range.end.clamp(start, self.data.len());
        }
        copy(&mut out, at, self.data.len());
        out
    }
}

/// An affine matrix `[a b c d e f]` (PDF order: a point maps to `(a·x + c·y + e, b·x + d·y + f)`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f64; 6]);

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(x: f64, y: f64) -> Self {
        Matrix([1.0, 0.0, 0.0, 1.0, x, y])
    }

    /// `self` then `then` (`self × then` in PDF's row-vector convention).
    pub fn then(&self, then: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = then.0;
        Matrix([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// A vector (no translation).
    pub fn apply_vec(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, _, _] = self.0;
        (a * x + c * y, b * x + d * y)
    }

    pub fn invert(&self) -> Option<Matrix> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Matrix([ia, ib, ic, id, -(e * ia + f * ic), -(e * ib + f * id)]))
    }

    /// The bounding box of a rectangle `[x0 y0 x1 y1]` after this transform.
    pub fn bbox(&self, r: [f64; 4]) -> [f64; 4] {
        let pts = [self.apply(r[0], r[1]), self.apply(r[2], r[1]), self.apply(r[0], r[3]), self.apply(r[2], r[3])];
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (x, y) in pts {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_operators_with_spans() {
        let data = b"BT /F1 12 Tf 72 700 Td (Hello \\(world\\)) Tj [(A) -120 <0042>] TJ ET";
        let ops = parse(data);
        let names: Vec<&[u8]> = ops.iter().map(|o| o.op.as_slice()).collect();
        assert_eq!(names, [&b"BT"[..], b"Tf", b"Td", b"Tj", b"TJ", b"ET"]);
        assert_eq!(ops[3].operands, vec![Obj::Str(b"Hello (world)".to_vec())]);
        assert_eq!(ops[4].operands, vec![Obj::Array(vec![Obj::Str(b"A".to_vec()), Obj::Int(-120), Obj::Str(vec![0, b'B'])])]);
        assert_eq!(&data[ops[1].span.clone()], b"/F1 12 Tf");
    }

    #[test]
    fn keeps_inline_images_whole() {
        let data = b"q BI /W 1 /H 1 /BPC 8 /CS /G ID \x00EI Q";
        let ops = parse(data);
        let names: Vec<&[u8]> = ops.iter().map(|o| o.op.as_slice()).collect();
        assert_eq!(names, [&b"q"[..], b"BI", b"Q"]);
    }

    #[test]
    fn splices_across_pieces() {
        let p = Pieces::join(&[&b"BT /F1 12"[..], b"Tf (a) Tj ET"]);
        let ops = p.parse();
        let tf = ops.iter().find(|o| o.is("Tf")).unwrap();
        let out = p.splice([(tf.span.clone(), b"/F2 10 Tf".to_vec())]);
        assert_eq!(out[0], b"BT /F2 10 Tf");
        assert_eq!(out[1], b" (a) Tj ET");
    }

    #[test]
    fn writes_numbers_compactly() {
        let mut out = Vec::new();
        for v in [num(12.0), num(0.5), num(-0.00001), num(1.23456)] {
            write_obj(&v, &mut out);
            out.push(b' ');
        }
        assert_eq!(out, b"12 0.5 0 1.2346 ");
    }
}
