// Adapted from pdfcraft (MIT OR Apache-2.0), crates/js/src/lib.rs @ b8ddf71 (util.printf, util.printd, util.printx)
// Changes: pure functions over plain values (the object model lives in prelude.js), util.scand added.
//! Acrobat's `util.printf`, `util.printd`, `util.printx` and `util.scand`, as pure functions.

use super::{MAX_PRINTF_OUTPUT_BYTES, MAX_PRINTF_WIDTH_PRECISION};

/// One argument of `util.printf`: the JavaScript number it converts to (`NaN` when it has none)
/// and its string form.
#[derive(Clone, Debug)]
pub struct PrintfArg {
    pub num: f64,
    pub text: String,
}

/// `util.printf(cFormat, …)`: `%[,n][+ 0#][width][.prec](d|f|s|x)`, where `,0` groups thousands with
/// commas (the default), `,1` doesn't, `,2` uses `.` and `,`, `,3` uses `.` without grouping.
pub fn printf(fmt: &str, args: &[PrintfArg]) -> Result<String, String> {
    let mut out = String::new();
    let check = |len: usize| -> Result<(), String> {
        if len > MAX_PRINTF_OUTPUT_BYTES {
            return Err(format!("util.printf output exceeds {MAX_PRINTF_OUTPUT_BYTES} bytes"));
        }
        Ok(())
    };
    let mut next = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            check(out.len().saturating_add(c.len_utf8()))?;
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            check(out.len().saturating_add(1))?;
            out.push('%');
            continue;
        }
        let mut sep = 1u8;
        if chars.peek() == Some(&',') {
            chars.next();
            sep = chars.next().and_then(|d| d.to_digit(10)).unwrap_or(0) as u8;
        }
        let (mut plus, mut space, mut zero) = (false, false, false);
        while let Some(&f) = chars.peek() {
            match f {
                '+' => plus = true,
                ' ' => space = true,
                '0' => zero = true,
                '#' => {}
                _ => break,
            }
            chars.next();
        }
        let mut width = 0usize;
        while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
            width = width.saturating_mul(10).saturating_add(d as usize);
            chars.next();
        }
        let mut prec = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p = 0usize;
            while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                p = p.saturating_mul(10).saturating_add(d as usize);
                chars.next();
            }
            prec = Some(p);
        }
        if width > MAX_PRINTF_WIDTH_PRECISION || prec.is_some_and(|p| p > MAX_PRINTF_WIDTH_PRECISION) {
            return Err(format!("util.printf width or precision exceeds {MAX_PRINTF_WIDTH_PRECISION}"));
        }
        let conv = chars.next().unwrap_or('s');
        let arg = args.get(next);
        next += 1;
        let num = arg.map_or(f64::NAN, |a| a.num);
        let body = match conv {
            'd' | 'f' => {
                let p = if conv == 'd' { 0 } else { prec.unwrap_or(6) };
                let n = if conv == 'd' { num.trunc() } else { num };
                let mut t = if n.is_finite() { group(&format!("{:.*}", p, n.abs()), sep) } else { "NaN".to_string() };
                if n < 0.0 {
                    t.insert(0, '-');
                } else if plus {
                    t.insert(0, '+');
                } else if space {
                    t.insert(0, ' ');
                }
                t
            }
            'x' => format!("{:X}", if num.is_finite() { num as i64 } else { 0 }),
            _ => {
                let t = arg.map_or_else(String::new, |a| a.text.clone());
                match prec {
                    Some(p) => t.chars().take(p).collect(),
                    None => t,
                }
            }
        };
        let pad = width.saturating_sub(body.chars().count());
        check(out.len().saturating_add(pad).saturating_add(body.len()))?;
        if zero && matches!(conv, 'd' | 'f') {
            let (sign, digits) = if body.starts_with(['-', '+', ' ']) { body.split_at(1) } else { ("", body.as_str()) };
            out.push_str(sign);
            out.push_str(&"0".repeat(pad));
            out.push_str(digits);
        } else {
            out.push_str(&" ".repeat(pad));
            out.push_str(&body);
        }
    }
    Ok(out)
}

/// Group the integer part of a plain decimal number per `util.printf`'s `,n` flag.
fn group(t: &str, sep: u8) -> String {
    let (int, frac) = t.split_once('.').map_or((t, None), |(a, b)| (a, Some(b)));
    let (thousands, point) = match sep {
        0 => (Some(','), '.'),
        2 => (Some('.'), ','),
        3 => (None, ','),
        _ => (None, '.'),
    };
    let mut g = String::new();
    for (i, ch) in int.chars().enumerate() {
        if let Some(th) = thousands
            && i > 0
            && (int.len() - i).is_multiple_of(3)
        {
            g.push(th);
        }
        g.push(ch);
    }
    if let Some(f) = frac {
        g.push(point);
        g.push_str(f);
    }
    g
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// The parts of a JavaScript `Date`, local time: year, month (0-11), day, weekday (0 = Sunday), hours,
/// minutes, seconds.
pub type DateParts = [i64; 7];

/// `util.printd(cFormat, oDate)`: yyyy yy mmmm mmm mm m dddd ddd dd d HH H hh h MM M ss s tt, and the
/// numeric formats 0 (`D:yyyymmddHHMMss`), 1 (`yyyy.mm.dd HH:MM:ss`) and 2 (`m/d/yy h:MM:ss tt`).
pub fn printd(fmt: &str, parts: DateParts) -> String {
    let [y, mo, day, wd, h, mi, sec] = parts;
    let mut out = String::new();
    let b = fmt.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let run = b[i..].iter().take_while(|x| **x == c).count();
        let push_num = |out: &mut String, v: i64, n: usize| {
            if n >= 2 {
                out.push_str(&format!("{v:02}"));
            } else {
                out.push_str(&v.to_string());
            }
        };
        let month = MONTHS[mo.rem_euclid(12) as usize];
        let day_name = DAYS[wd.rem_euclid(7) as usize];
        match c {
            b'y' => {
                if run >= 4 {
                    out.push_str(&format!("{y:04}"));
                } else {
                    out.push_str(&format!("{:02}", y.rem_euclid(100)));
                }
            }
            b'm' => match run {
                r if r >= 4 => out.push_str(month),
                3 => out.push_str(&month[..3]),
                n => push_num(&mut out, mo + 1, n),
            },
            b'd' => match run {
                r if r >= 4 => out.push_str(day_name),
                3 => out.push_str(&day_name[..3]),
                n => push_num(&mut out, day, n),
            },
            b'H' => push_num(&mut out, h, run),
            b'h' => push_num(&mut out, if h % 12 == 0 { 12 } else { h % 12 }, run),
            b'M' => push_num(&mut out, mi, run),
            b's' => push_num(&mut out, sec, run),
            b't' => {
                let ampm = if h < 12 { "am" } else { "pm" };
                out.push_str(if run >= 2 { ampm } else { &ampm[..1] });
            }
            b'\\' if i + 1 < b.len() => {
                // The escaped character, which may be a multi-byte one.
                let ch = fmt[i + 1..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += 1 + ch.len_utf8();
                continue;
            }
            _ => {
                // Copy a whole (possibly multi-byte) character.
                let ch = fmt[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
        }
        i += run;
    }
    out
}

/// The format `util.printd` is given as a number.
pub fn printd_preset(n: i64) -> &'static str {
    match n {
        0 => "D:yyyymmddHHMMss",
        1 => "yyyy.mm.dd HH:MM:ss",
        _ => "m/d/yy h:MM:ss tt",
    }
}

/// `util.printx(cFormat, cSource)`: `?` any character, `X` letter or digit, `A` letter, `9` digit, `*`
/// the rest, `\` escape, `>` / `<` / `=` upper / lower / as is.
pub fn printx(fmt: &str, src: &str) -> String {
    let fmt: Vec<char> = fmt.chars().collect();
    let src: Vec<char> = src.chars().collect();
    let (mut out, mut j, mut case) = (String::new(), 0, 0);
    let mut i = 0;
    let push = |out: &mut String, c: char, case: i32| match case {
        1 => out.extend(c.to_uppercase()),
        -1 => out.extend(c.to_lowercase()),
        _ => out.push(c),
    };
    while i < fmt.len() {
        let f = fmt[i];
        i += 1;
        let take = |j: &mut usize, ok: &dyn Fn(char) -> bool| -> Option<char> {
            while *j < src.len() {
                let c = src[*j];
                *j += 1;
                if ok(c) {
                    return Some(c);
                }
            }
            None
        };
        match f {
            '?' => {
                if let Some(c) = take(&mut j, &|_| true) {
                    push(&mut out, c, case)
                }
            }
            'X' => {
                if let Some(c) = take(&mut j, &|c| c.is_alphanumeric()) {
                    push(&mut out, c, case)
                }
            }
            'A' => {
                if let Some(c) = take(&mut j, &|c| c.is_alphabetic()) {
                    push(&mut out, c, case)
                }
            }
            '9' => {
                if let Some(c) = take(&mut j, &|c| c.is_ascii_digit()) {
                    push(&mut out, c, case)
                }
            }
            '*' => {
                for c in src[j.min(src.len())..].iter().copied() {
                    push(&mut out, c, case);
                }
                j = src.len();
            }
            '\\' if i < fmt.len() => {
                out.push(fmt[i]);
                i += 1;
            }
            '>' => case = 1,
            '<' => case = -1,
            '=' => case = 0,
            c => out.push(c),
        }
    }
    out
}

/// `util.scand(cFormat, cDate)`: the date and time in `text` read against `fmt`, as year, month (1-12),
/// day, hours, minutes, seconds. `None` when it is not a date (the script gets `null`).
pub fn scand(fmt: &str, text: &str) -> Option<[u32; 6]> {
    let dt = crate::forms::af::parse_date(text, fmt)?;
    Some([u32::try_from(dt.y).ok()?, dt.m, dt.d, dt.hh, dt.mm, dt.ss])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(n: f64) -> PrintfArg {
        PrintfArg { num: n, text: crate::forms::af::js_number(n) }
    }

    fn s(t: &str) -> PrintfArg {
        PrintfArg { num: t.trim().parse().unwrap_or(f64::NAN), text: t.into() }
    }

    #[test]
    fn printf_formats_like_acrobat() {
        assert_eq!(printf("%,2d items", &[a(1234.0)]).unwrap(), "1.234 items");
        assert_eq!(printf("$%,0.2f", &[a(1234.5)]).unwrap(), "$1,234.50");
        assert_eq!(printf("%05d|%+d|%s|%.2s|%x", &[a(42.0), a(7.0), s("hello"), s("world"), a(255.0)]).unwrap(), "00042|+7|hello|wo|FF");
        assert_eq!(printf("100%%", &[]).unwrap(), "100%");
        assert_eq!(printf("%8.3f|", &[a(-3.14159)]).unwrap(), "  -3.142|");
        assert!(printf("%99999d", &[a(1.0)]).is_err(), "huge width refused");
    }

    #[test]
    fn printd_and_printx_and_scand() {
        // Sunday 4 Feb 2024 14:05:09.
        let d = [2024, 1, 4, 0, 14, 5, 9];
        assert_eq!(printd("yyyy-mm-dd HH:MM:ss", d), "2024-02-04 14:05:09");
        assert_eq!(printd("dddd, mmmm d, yyyy h:MM tt", d), "Sunday, February 4, 2024 2:05 pm");
        assert_eq!(printd(printd_preset(1), d), "2024.02.04 14:05:09");
        assert_eq!(printd("\\d: d", d), "d: 4");
        assert_eq!(printx("(999) 999-9999", "5551234567"), "(555) 123-4567");
        assert_eq!(printx(">AA<AA", "abcd"), "ABcd");
        assert_eq!(scand("mm/dd/yyyy", "3/9/2021"), Some([2021, 3, 9, 0, 0, 0]));
        assert_eq!(scand("mm/dd/yyyy", "13/9/2021"), None);
    }
}
