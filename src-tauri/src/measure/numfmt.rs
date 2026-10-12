//! PDF number formats (`/Type /NumberFormat`, ISO 32000-1 table 262): how a measured value reads.

/// `/F`: how the value is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// `/D`: decimal, rounded to 1/`d`.
    Decimal,
    /// `/F`: whole and fraction, denominator at most `d`.
    Fraction,
    /// `/R`: rounded to a whole number.
    Round,
    /// `/T`: truncated to a whole number.
    Truncate,
}

impl Style {
    pub fn name(self) -> &'static str {
        match self {
            Style::Decimal => "D",
            Style::Fraction => "F",
            Style::Round => "R",
            Style::Truncate => "T",
        }
    }

    pub fn from_name(n: &str) -> Style {
        match n {
            "F" => Style::Fraction,
            "R" => Style::Round,
            "T" => Style::Truncate,
            _ => Style::Decimal,
        }
    }
}

/// One element of a number format array (`/X`, `/Y`, `/D`, `/A`).
#[derive(Clone, Debug, PartialEq)]
pub struct NumberFormat {
    /// `/U`: the label, "ft", "sq ft".
    pub unit: String,
    /// `/C`: for the first element, the factor from the quantity the array is applied to; for a later one, how many of
    /// it make one of the previous element (`C 12` after "ft" means "in").
    pub c: f64,
    pub style: Style,
    /// `/D`: precision. 100 is two decimals; for a fraction the largest denominator.
    pub d: f64,
    /// `/FD`: keep trailing zeros of a decimal (and do not reduce a fraction).
    pub fixed: bool,
    /// `/RT`: thousands separator.
    pub thousands: String,
    /// `/RD`: decimal separator.
    pub decimal: String,
    /// `/PS`: text between the number and the label.
    pub ps: String,
    /// `/SS`: text after the label.
    pub ss: String,
    /// `/O` is `/P`: the label comes first.
    pub prefix: bool,
}

impl NumberFormat {
    /// What this app writes: two decimals with trailing zeros, "," and ".", a space before the label.
    pub fn new(unit: &str, c: f64) -> NumberFormat {
        NumberFormat {
            unit: unit.to_owned(),
            c,
            style: Style::Decimal,
            d: 100.0,
            fixed: true,
            thousands: ",".into(),
            decimal: ".".into(),
            ps: " ".into(),
            ss: String::new(),
            prefix: false,
        }
    }

    /// `/D` as used: the precision, or the largest denominator of a fraction.
    fn precision(&self) -> f64 {
        if self.d.is_finite() && self.d >= 1.0 { self.d.min(1e9) } else { 100.0 }
    }

    /// `v` as this format would round it (what "1,234.50" or "3 7/8" stand for), so a caller can see whether rounding
    /// made it a whole unit of the one before.
    pub fn rounded(&self, v: f64) -> f64 {
        let d = self.precision();
        match self.style {
            Style::Round => v.round(),
            Style::Truncate => v.trunc(),
            Style::Decimal => (v * d).round() / d,
            Style::Fraction => (v * d.round()).round() / d.round(),
        }
    }

    /// The number alone: "1,234.50".
    pub fn number(&self, v: f64) -> String {
        let d = self.precision();
        match self.style {
            Style::Round => self.group(&format!("{:.0}", v.round()), ""),
            Style::Truncate => self.group(&format!("{:.0}", v.trunc()), ""),
            Style::Decimal => {
                let decimals = d.log10().ceil().max(0.0) as usize;
                let r = (v * d).round() / d;
                let s = format!("{r:.decimals$}");
                let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
                let frac = if self.fixed { frac } else { frac.trim_end_matches('0') };
                self.group(int, frac)
            }
            Style::Fraction => {
                let sign = if v < 0.0 { "-" } else { "" };
                let v = v.abs();
                let den = d.round().max(1.0) as u64;
                let mut whole = v.trunc() as u64;
                let mut num = ((v - v.trunc()) * den as f64).round() as u64;
                let mut den = den;
                if num >= den {
                    whole += 1;
                    num = 0;
                }
                if !self.fixed && num > 0 {
                    let g = gcd(num, den);
                    num /= g;
                    den /= g;
                }
                let w = self.group(&whole.to_string(), "");
                match (whole, num) {
                    (_, 0) => format!("{sign}{w}"),
                    (0, _) => format!("{sign}{num}/{den}"),
                    _ => format!("{sign}{w} {num}/{den}"),
                }
            }
        }
    }

    /// The number with its label: "1,234.50 ft".
    pub fn text(&self, v: f64) -> String {
        let n = self.number(v);
        if self.prefix { format!("{}{}{n}", self.unit, self.ps) } else { format!("{n}{}{}{}", self.ps, self.unit, self.ss) }
    }

    fn group(&self, int: &str, frac: &str) -> String {
        let digits: Vec<char> = int.trim_start_matches('-').chars().collect();
        let mut out = String::new();
        if int.starts_with('-') {
            out.push('-');
        }
        for (i, c) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push_str(&self.thousands);
            }
            out.push(*c);
        }
        if !frac.is_empty() {
            out.push_str(&self.decimal);
            out.push_str(frac);
        }
        out
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Writes `v`, a value in the units of the first element, through the whole array: with `[ft, in(C 12)]`, 3.375 ft
/// reads "3 ft 4.50 in". All but the last element show whole units; the last takes the remainder in its own style.
/// A remainder that rounds up to a whole unit of the one before is carried (2.9999 ft is "3 ft 0.00 in", never
/// "2 ft 12.00 in").
pub fn chain_text(v: f64, formats: &[NumberFormat]) -> String {
    let Some(last) = formats.len().checked_sub(1) else { return String::new() };
    let mut parts = vec![0.0; formats.len()];
    let mut rest = v;
    for i in 0..last {
        parts[i] = rest.trunc();
        rest = (rest - parts[i]) * formats[i + 1].c;
    }
    parts[last] = formats[last].rounded(rest);
    for i in (1..=last).rev() {
        if parts[i] >= formats[i].c {
            parts[i] -= formats[i].c;
            parts[i - 1] += 1.0;
        }
    }
    let texts: Vec<String> = formats
        .iter()
        .zip(&parts)
        .enumerate()
        .map(|(i, (f, p))| if i == last { f.text(*p) } else { NumberFormat { style: Style::Truncate, ..f.clone() }.text(*p) })
        .collect();
    // A default `/SS` (a single space) must not leave a blank at the end of a part, or double the one between parts.
    texts.iter().map(|t| t.trim()).collect::<Vec<_>>().join(" ")
}
