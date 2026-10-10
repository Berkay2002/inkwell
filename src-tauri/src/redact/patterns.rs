//! Built-in sensitive-data patterns. Regexes run on the normalized page text from `index_page`
//! (dashes folded to '-', whitespace to ' ', line breaks joined), then a checksum or shape check
//! drops false positives.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Pattern {
    /// Swedish personnummer / samordningsnummer (YYMMDD-NNNC, optional century), Luhn-checked.
    Personnummer,
    /// US social security number, NNN-NN-NNNN.
    Ssn,
    /// IBAN, compact or in groups of four, mod-97 checked.
    Iban,
    /// Payment card number (13-19 digits), Luhn-checked.
    Card,
    Email,
    /// Phone number with separators, `+` country code or `(area)`; dates are excluded.
    Phone,
}

impl Pattern {
    pub const ALL: [Pattern; 6] =
        [Pattern::Personnummer, Pattern::Ssn, Pattern::Iban, Pattern::Card, Pattern::Email, Pattern::Phone];

    fn regex(self) -> &'static Regex {
        static PNR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:19|20)?([0-9]{6})[-+ ]?([0-9]{4})\b").unwrap());
        static SSN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([0-9]{3})[- ]([0-9]{2})[- ]([0-9]{4})\b").unwrap());
        static IBAN: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\b[A-Z]{2}[0-9]{2}(?:(?: ?[A-Z0-9]{4}){2,7}(?: ?[A-Z0-9]{1,3})?|[A-Z0-9]{11,30})\b").unwrap()
        });
        static CARD: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\b(?:[0-9]{4}[ -]?[0-9]{4}[ -]?[0-9]{4}[ -]?[0-9]{1,7}|[0-9]{4}[ -]?[0-9]{6}[ -]?[0-9]{4,5}|[0-9]{13,19})\b").unwrap()
        });
        static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b").unwrap()
        });
        static PHONE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?:\+[0-9]{1,3}[ .-]?(?:\([0-9]{1,4}\)[ .-]?)?|\([0-9]{1,4}\)[ .-]?|\b)[0-9]{2,4}(?:[ .-][0-9]{2,4}){1,4}\b").unwrap()
        });
        match self {
            Pattern::Personnummer => &PNR,
            Pattern::Ssn => &SSN,
            Pattern::Iban => &IBAN,
            Pattern::Card => &CARD,
            Pattern::Email => &EMAIL,
            Pattern::Phone => &PHONE,
        }
    }

    fn valid(self, m: &str) -> bool {
        let digits: Vec<u8> = m.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
        match self {
            // Regexes are ASCII-only ([0-9]), but guard the counts anyway: a panic here aborts `find`.
            Pattern::Personnummer if digits.len() < 10 => false,
            Pattern::Ssn if digits.len() != 9 => false,
            Pattern::Personnummer => {
                let d = &digits[digits.len() - 10..];
                let month = d[2] * 10 + d[3];
                let day = d[4] * 10 + d[5];
                (1..=12).contains(&month) && ((1..=31).contains(&day) || (61..=91).contains(&day)) && luhn(d)
            }
            Pattern::Ssn => {
                let (area, group, serial) = (&digits[..3], &digits[3..5], &digits[5..]);
                area != [0, 0, 0] && area != [6, 6, 6] && group != [0, 0] && serial != [0, 0, 0, 0]
            }
            Pattern::Iban => {
                let s: Vec<u8> = m.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
                (15..=34).contains(&s.len()) && iban_ok(&s)
            }
            Pattern::Card => (13..=19).contains(&digits.len()) && luhn(&digits),
            Pattern::Email => true,
            Pattern::Phone => {
                static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}[-.]\d{2}[-.]\d{2}$").unwrap());
                (7..=15).contains(&digits.len()) && !DATE.is_match(m)
            }
        }
    }

    /// Byte ranges of valid matches in `text`.
    pub fn find(self, text: &str) -> Vec<Range<usize>> {
        self.regex().find_iter(text).filter(|m| self.valid(m.as_str())).map(|m| m.range()).collect()
    }
}

fn luhn(d: &[u8]) -> bool {
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            let x = x as u32;
            if i % 2 == 1 { if x * 2 > 9 { x * 2 - 9 } else { x * 2 } } else { x }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn iban_ok(s: &[u8]) -> bool {
    let (head, tail) = s.split_at(4);
    let mut rem = 0u32;
    for &c in tail.iter().chain(head) {
        let v = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'A'..=b'Z' => (c - b'A') as u32 + 10,
            _ => return false,
        };
        rem = if v >= 10 { (rem * 100 + v) % 97 } else { (rem * 10 + v) % 97 };
    }
    rem == 1
}

#[cfg(test)]
mod tests {
    use super::Pattern::*;
    use super::*;

    fn hits(p: Pattern, text: &str) -> Vec<&str> {
        p.find(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn patterns() {
        assert_eq!(hits(Personnummer, "pnr 19811218-9876 och 811218-9876, fel 811218-9875"), ["19811218-9876", "811218-9876"]);
        assert_eq!(hits(Ssn, "SSN: 123-45-6789, 987-65-4321, bad 000-12-3456"), ["123-45-6789", "987-65-4321"]);
        assert_eq!(hits(Iban, "IBAN SE45 5000 0000 0583 9825 7466 or GB82WEST12345698765432, bad GB82WEST12345698765433"),
            ["SE45 5000 0000 0583 9825 7466", "GB82WEST12345698765432"]);
        assert_eq!(hits(Card, "Credit card: 4111 1111 1111 1111 exp 12/27, amex 3782 822463 10005, bad 4111 1111 1111 1112"),
            ["4111 1111 1111 1111", "3782 822463 10005"]);
        assert_eq!(hits(Email, "mail jane.tester@example.com or legal@example.co.uk."), ["jane.tester@example.com", "legal@example.co.uk"]);
        assert_eq!(hits(Phone, "Phone: (555) 010-4477, +46 70 123 45 67, on 2026-01-15, v1.40.28"), ["(555) 010-4477", "+46 70 123 45 67"]);
        // Non-ASCII digits used to match \d and then panic the validators.
        for p in Pattern::ALL {
            assert!(hits(p, "１２３-４５-６７８９ ١٢٣٤٥٦-٧٨٩٠ ８１１２１８-９８７６").is_empty());
        }
    }
}
