//! Fast substitute-font lookup for fonts a PDF references but does not embed.
//!
//! MuPDF's stock system-font hook (font-kit/DirectWrite) enumerates and opens every installed
//! font to match a PostScript name, which costs 0.5-2 s on the first page of a document. This
//! loader maps names to files in the Windows font folders with a table and one directory scan.
//! Anything it does not know falls through to MuPDF's built-in base-14 fonts.

use mupdf::font_loader::{FontHints, FontLoader};
use mupdf::{CjkFontOrdering, Font};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// family key -> file stems for [regular, bold, italic, bold italic].
const FAMILIES: &[(&str, [&str; 4])] = &[
    ("calibri", ["calibri", "calibrib", "calibrii", "calibriz"]),
    ("calibrilight", ["calibril", "calibril", "calibrili", "calibrili"]),
    ("cambria", ["cambria", "cambriab", "cambriai", "cambriaz"]),
    ("candara", ["candara", "candarab", "candarai", "candaraz"]),
    ("consolas", ["consola", "consolab", "consolai", "consolaz"]),
    ("constantia", ["constan", "constanb", "constani", "constanz"]),
    ("corbel", ["corbel", "corbelb", "corbeli", "corbelz"]),
    ("segoeui", ["segoeui", "segoeuib", "segoeuii", "segoeuiz"]),
    ("segoeuisemibold", ["seguisb", "seguisb", "seguisbi", "seguisbi"]),
    ("tahoma", ["tahoma", "tahomabd", "tahoma", "tahomabd"]),
    ("verdana", ["verdana", "verdanab", "verdanai", "verdanaz"]),
    ("georgia", ["georgia", "georgiab", "georgiai", "georgiaz"]),
    ("trebuchetms", ["trebuc", "trebucbd", "trebucit", "trebucbi"]),
    ("comicsansms", ["comic", "comicbd", "comici", "comicz"]),
    ("impact", ["impact", "impact", "impact", "impact"]),
    ("palatinolinotype", ["pala", "palab", "palai", "palabi"]),
    ("lucidaconsole", ["lucon", "lucon", "lucon", "lucon"]),
    ("franklingothicmedium", ["framd", "framd", "framdit", "framdit"]),
    ("gillsans", ["gil_____", "gilb____", "gili____", "gilbi___"]),
    ("arialnarrow", ["arialn", "arialnb", "arialni", "arialnbi"]),
    ("arialblack", ["ariblk", "ariblk", "ariblk", "ariblk"]),
    ("symbol", ["symbol", "symbol", "symbol", "symbol"]),
    ("wingdings", ["wingding", "wingding", "wingding", "wingding"]),
    ("webdings", ["webdings", "webdings", "webdings", "webdings"]),
    ("microsoftyahei", ["msyh", "msyhbd", "msyh", "msyhbd"]),
    ("simsun", ["simsun", "simsun", "simsun", "simsun"]),
    ("simhei", ["simhei", "simhei", "simhei", "simhei"]),
    ("msgothic", ["msgothic", "msgothic", "msgothic", "msgothic"]),
    ("msmincho", ["msmincho", "msmincho", "msmincho", "msmincho"]),
    ("malgungothic", ["malgun", "malgunbd", "malgun", "malgunbd"]),
    ("yugothic", ["yugothr", "yugothb", "yugothr", "yugothb"]),
    ("meiryo", ["meiryo", "meiryob", "meiryo", "meiryob"]),
];

/// File stem (lower case) -> font file, from the Windows font folders.
fn scan_font_dirs() -> HashMap<String, PathBuf> {
    let mut dirs = Vec::new();
    if let Some(w) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(w).join("Fonts"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(l).join(r"Microsoft\Windows\Fonts"));
    }
    let mut map = HashMap::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            if matches!(ext.as_deref(), Some("ttf" | "ttc" | "otf")) {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    map.entry(stem.to_ascii_lowercase()).or_insert(p.clone());
                }
            }
        }
    }
    map
}

#[derive(Default)]
pub struct WindowsFonts {
    index: OnceLock<HashMap<String, PathBuf>>,
    loaded: Mutex<HashMap<(PathBuf, i32), Option<Vec<u8>>>>,
}

impl WindowsFonts {
    fn index(&self) -> &HashMap<String, PathBuf> {
        self.index.get_or_init(scan_font_dirs)
    }

    fn font_from_stem(&self, display: &str, stem: &str) -> Option<Font> {
        let path = self.index().get(stem)?.clone();
        let mut cache = self.loaded.lock();
        let data = cache
            .entry((path.clone(), 0))
            .or_insert_with(|| std::fs::read(&path).ok())
            .as_ref()?;
        Font::from_bytes_with_index(display, 0, data).ok()
    }
}

/// `ABCDEF+Calibri-BoldItalic` -> (`calibri`, bold, italic).
fn normalise(name: &str, hints: FontHints) -> (String, bool, bool) {
    let name = match name.split_once('+') {
        Some((p, rest)) if p.len() == 6 && p.chars().all(|c| c.is_ascii_uppercase()) => rest,
        _ => name,
    };
    let mut s: String = name
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | ',' | '_'))
        .collect::<String>()
        .to_ascii_lowercase();
    let mut bold = hints.bold;
    let mut italic = hints.italic;
    for (tok, is_bold) in [("bold", true), ("italic", false), ("oblique", false), ("regular", false)] {
        if s.contains(tok) {
            s = s.replace(tok, "");
            if tok == "bold" {
                bold = is_bold;
            } else if tok != "regular" {
                italic = true;
            }
        }
    }
    for suffix in ["psmt", "mt", "ps"] {
        if let Some(t) = s.strip_suffix(suffix) {
            s = t.to_string();
        }
    }
    (s, bold, italic)
}

impl FontLoader for WindowsFonts {
    fn load_font(&self, name: &str, hints: FontHints) -> Option<Font> {
        let (family, bold, italic) = normalise(name, hints);
        let (_, stems) = FAMILIES.iter().find(|(k, _)| *k == family)?;
        let stem = stems[bold as usize + 2 * italic as usize];
        self.font_from_stem(name, stem)
    }

    fn load_cjk_font(&self, name: &str, ordering: CjkFontOrdering, serif: bool) -> Option<Font> {
        let stems: &[&str] = match (ordering, serif) {
            (CjkFontOrdering::AdobeJapan, true) => &["msmincho", "yumin"],
            (CjkFontOrdering::AdobeJapan, false) => &["msgothic", "yugothr", "meiryo"],
            (CjkFontOrdering::AdobeGb, true) => &["simsun", "msyh"],
            (CjkFontOrdering::AdobeGb, false) => &["msyh", "simhei"],
            (CjkFontOrdering::AdobeCns, _) => &["msjh", "mingliu"],
            (CjkFontOrdering::AdobeKorea, true) => &["batang", "malgun"],
            (CjkFontOrdering::AdobeKorea, false) => &["malgun", "gulim"],
        };
        stems.iter().find_map(|s| self.font_from_stem(name, s))
    }
}

/// Families the page loader above leaves to MuPDF's built-in fonts but that Windows has: used by
/// Preflight to embed the real font (family key -> stems for regular, bold, italic, bold italic).
const EXTRA_FAMILIES: &[(&str, [&str; 4])] = &[
    ("arial", ["arial", "arialbd", "ariali", "arialbi"]),
    ("timesnewroman", ["times", "timesbd", "timesi", "timesbi"]),
    ("couriernew", ["cour", "courbd", "couri", "courbi"]),
];

/// The TrueType file (`.ttf`) of an installed font by the name a PDF gives it (`ABCDEF+Calibri-Bold`,
/// `Arial,Italic`, `TimesNewRomanPS-BoldMT`), or `None`. Nothing is substituted: the family must be
/// known and the file present.
pub fn installed_file(name: &str) -> Option<PathBuf> {
    static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    let (family, bold, italic) = normalise(name, FontHints::default());
    let (_, stems) = FAMILIES.iter().chain(EXTRA_FAMILIES.iter()).find(|(k, _)| *k == family)?;
    let stem = stems[bold as usize + 2 * italic as usize];
    let path = INDEX.get_or_init(scan_font_dirs).get(stem)?.clone();
    (path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ttf"))).then_some(path)
}

pub fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| mupdf::set_font_loader(WindowsFonts::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_pdf_font_names() {
        let h = FontHints::default();
        assert_eq!(normalise("ABCDEF+Calibri-BoldItalic", h), ("calibri".into(), true, true));
        assert_eq!(normalise("TrebuchetMS,Bold", h), ("trebuchetms".into(), true, false));
        assert_eq!(normalise("Segoe UI", h), ("segoeui".into(), false, false));
        assert_eq!(normalise("Verdana-Italic", h), ("verdana".into(), false, true));
        assert_eq!(normalise("GillSansMT", h).0, "gillsans");
        assert_eq!(normalise("Arial-BoldMT", h), ("arial".into(), true, false));
        assert_eq!(normalise("TimesNewRomanPS-BoldItalicMT", h), ("timesnewroman".into(), true, true));
    }
}
