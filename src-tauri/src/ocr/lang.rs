//! Which languages OCR can read, which engine serves them, and a guess at what a document is written in.
//!
//! * [`Catalog`]: the Windows packs and the Tesseract `tessdata` files merged per language.
//! * [`pick_engine`]: the engine for a set of languages. Windows reads one language at a time, so
//!   several languages, or a language Windows lacks, go to Tesseract. A language nobody has comes back
//!   as a [`Missing`] with how to add it. Nothing is ever downloaded.
//! * [`guess`]: the language of a piece of text from its script and a stopword hit rate. It decides the
//!   default language from the text a document already has, and from a quick sample read of page 1.

use serde::Serialize;

/// The two engines (see `backend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Engine {
    Windows,
    Tesseract,
}

/// Tesseract's three-letter names for the primary tags it is asked for (`sv` is `swe`).
pub(crate) const ISO3: &[(&str, &str)] = &[
    ("en", "eng"),
    ("sv", "swe"),
    ("de", "deu"),
    ("fr", "fra"),
    ("es", "spa"),
    ("it", "ita"),
    ("nl", "nld"),
    ("pt", "por"),
    ("da", "dan"),
    ("nb", "nor"),
    ("no", "nor"),
    ("fi", "fin"),
    ("pl", "pol"),
    ("cs", "ces"),
    ("ru", "rus"),
    ("tr", "tur"),
    ("ja", "jpn"),
    ("zh", "chi_sim"),
    ("ko", "kor"),
    ("ar", "ara"),
    ("he", "heb"),
    ("fa", "fas"),
    ("hi", "hin"),
    ("el", "ell"),
    ("uk", "ukr"),
    ("hu", "hun"),
    ("ro", "ron"),
    ("bg", "bul"),
    ("th", "tha"),
    ("vi", "vie"),
    ("id", "ind"),
    ("ca", "cat"),
    ("hr", "hrv"),
    ("sk", "slk"),
    ("sl", "slv"),
    ("lt", "lit"),
    ("lv", "lav"),
    ("et", "est"),
    ("is", "isl"),
];

/// `en-US` -> `en`, `zh_Hans` -> `zh`.
pub fn primary(tag: &str) -> String {
    tag.split(['-', '_']).next().unwrap_or(tag).to_ascii_lowercase()
}

/// The `tessdata` file (without extension) that serves `tag`.
pub fn tessdata_name(tag: &str) -> String {
    let p = primary(tag);
    ISO3.iter().find(|(a, _)| *a == p).map(|(_, b)| (*b).to_string()).unwrap_or(p)
}

const RTL: &[&str] = &["ar", "he", "fa", "ur", "yi"];
const CJK: &[&str] = &["ja", "zh", "ko"];

/// One language as the language list shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LangInfo {
    /// The primary tag (`sv`); also what the request carries.
    pub tag: String,
    /// The full tag of the installed Windows pack (`sv-SE`), when there is one.
    pub windows: Option<String>,
    /// Whether Tesseract has its data file.
    pub tesseract: bool,
    pub rtl: bool,
    pub cjk: bool,
    /// Read, but not as reliable as Latin text: the list says so instead of hiding the language.
    pub experimental: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub languages: Vec<LangInfo>,
    /// `tesseract.exe` (with its tessdata) was found.
    pub tesseract_found: bool,
    /// Where Tesseract's `*.traineddata` files go (the `tessdata` folder beside its exe).
    pub tessdata_dir: Option<String>,
}

impl Catalog {
    /// `windows`: tags of the installed packs; `tesseract`: primary tags of the data files (`None`
    /// when Tesseract is not installed).
    pub fn build(windows: &[String], tesseract: Option<&[String]>, tessdata_dir: Option<String>) -> Catalog {
        let mut languages: Vec<LangInfo> = Vec::new();
        let mut add = |tag: &str, win: Option<&str>, tess: bool| {
            let p = primary(tag);
            if let Some(l) = languages.iter_mut().find(|l| l.tag == p) {
                if l.windows.is_none() {
                    l.windows = win.map(str::to_string);
                }
                l.tesseract |= tess;
                return;
            }
            let rtl = RTL.contains(&p.as_str());
            let cjk = CJK.contains(&p.as_str());
            languages.push(LangInfo { tag: p, windows: win.map(str::to_string), tesseract: tess, rtl, cjk, experimental: rtl || cjk });
        };
        for w in windows {
            add(w, Some(w), false);
        }
        for t in tesseract.unwrap_or(&[]) {
            add(t, None, true);
        }
        languages.sort_by(|a, b| a.tag.cmp(&b.tag));
        Catalog { languages, tesseract_found: tesseract.is_some(), tessdata_dir }
    }

    pub fn get(&self, tag: &str) -> Option<&LangInfo> {
        let p = primary(tag);
        self.languages.iter().find(|l| l.tag == p)
    }

    pub fn has(&self, tag: &str) -> bool {
        self.get(tag).is_some_and(|l| l.windows.is_some() || l.tesseract)
    }
}

/// How to get a language nobody serves yet. Only text: the UI opens the Windows settings page on
/// the user's click, and tells where the Tesseract file goes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Missing {
    pub tag: String,
    /// `ms-settings:regionlanguage`: add the language there with its "Optical character recognition"
    /// feature.
    pub windows_settings: &'static str,
    /// `swe.traineddata`.
    pub tessdata_file: String,
    /// The folder it goes in, when Tesseract is installed.
    pub tessdata_dir: Option<String>,
}

pub const WINDOWS_LANGUAGE_SETTINGS: &str = "ms-settings:regionlanguage";

/// The engine for a set of languages, and what could not be served.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// `None`: no installed engine reads any of the languages.
    pub engine: Option<Engine>,
    /// What to hand the engine, in priority order (full Windows tags for Windows).
    pub languages: Vec<String>,
    /// Requested languages this engine does not read.
    pub dropped: Vec<String>,
    /// Languages no installed engine has, with how to add each.
    pub missing: Vec<Missing>,
    /// A sentence for the sheet when the plan is not exactly what was asked for.
    pub note: Option<String>,
}

/// Picks the engine for `requested` (primary or full tags, priority order). `prefer` is the user's
/// own choice of engine; it is honoured when that engine can serve every language.
pub fn pick_engine(requested: &[String], catalog: &Catalog, prefer: Option<Engine>) -> Plan {
    let wanted: Vec<String> = {
        let mut seen: Vec<String> = Vec::new();
        for r in requested {
            let p = primary(r);
            if !p.is_empty() && !seen.contains(&p) {
                seen.push(p);
            }
        }
        seen
    };
    let missing_of = |tags: &[String]| -> Vec<Missing> {
        tags.iter()
            .map(|t| Missing {
                tag: t.clone(),
                windows_settings: WINDOWS_LANGUAGE_SETTINGS,
                tessdata_file: format!("{}.traineddata", tessdata_name(t)),
                tessdata_dir: catalog.tessdata_dir.clone(),
            })
            .collect()
    };
    let on_windows = |t: &str| catalog.get(t).is_some_and(|l| l.windows.is_some());
    let on_tess = |t: &str| catalog.get(t).is_some_and(|l| l.tesseract);
    let win_tag = |t: &str| catalog.get(t).and_then(|l| l.windows.clone()).unwrap_or_else(|| t.to_string());

    if wanted.is_empty() {
        return Plan { engine: None, languages: Vec::new(), dropped: Vec::new(), missing: Vec::new(), note: None };
    }
    let all_win = wanted.iter().all(|t| on_windows(t));
    let all_tess = catalog.tesseract_found && wanted.iter().all(|t| on_tess(t));
    let one = wanted.len() == 1;

    // Exactly what was asked for.
    let tess_plan = |dropped: Vec<String>, note: Option<String>| Plan {
        engine: Some(Engine::Tesseract),
        languages: wanted.iter().filter(|t| !dropped.contains(t)).cloned().collect(),
        dropped,
        missing: Vec::new(),
        note,
    };
    match prefer {
        Some(Engine::Tesseract) if all_tess => return tess_plan(Vec::new(), None),
        Some(Engine::Windows) if all_win && one => {
            return Plan { engine: Some(Engine::Windows), languages: wanted.iter().map(|t| win_tag(t)).collect(), dropped: Vec::new(), missing: Vec::new(), note: None };
        }
        _ => {}
    }
    if one && all_win {
        return Plan { engine: Some(Engine::Windows), languages: vec![win_tag(&wanted[0])], dropped: Vec::new(), missing: Vec::new(), note: None };
    }
    if all_tess {
        return tess_plan(Vec::new(), None);
    }

    // Not served as a whole: say what is missing, and read what can be read.
    if catalog.tesseract_found {
        let have: Vec<String> = wanted.iter().filter(|t| on_tess(t)).cloned().collect();
        let lacking: Vec<String> = wanted.iter().filter(|t| !on_tess(t) && !on_windows(t)).cloned().collect();
        if !have.is_empty() {
            let dropped: Vec<String> = wanted.iter().filter(|t| !on_tess(t)).cloned().collect();
            let mut p = tess_plan(dropped.clone(), Some("Tesseract has no data for some of these languages; they are left out.".into()));
            p.missing = missing_of(&lacking);
            return p;
        }
    }
    if let Some(first) = wanted.iter().find(|t| on_windows(t)) {
        // Windows reads one language per page: the first installed pack is used.
        let dropped: Vec<String> = wanted.iter().filter(|t| *t != first).cloned().collect();
        let lacking: Vec<String> = wanted.iter().filter(|t| !on_windows(t) && !on_tess(t)).cloned().collect();
        let note = if dropped.is_empty() {
            None
        } else if catalog.tesseract_found || wanted.iter().any(|t| !on_windows(t)) {
            Some("Windows reads one language at a time; Tesseract reads several together.".into())
        } else {
            Some("Windows reads one language at a time. Install Tesseract to read several together.".into())
        };
        return Plan { engine: Some(Engine::Windows), languages: vec![win_tag(first)], dropped, missing: missing_of(&lacking), note };
    }
    Plan { engine: None, languages: Vec::new(), dropped: wanted.clone(), missing: missing_of(&wanted), note: None }
}

/// The default selection: the Windows display language plus English, as far as they are installed.
/// `profile` is the user's Windows languages in order (display language first).
pub fn default_languages(profile: &[String], catalog: &Catalog) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in profile.iter().map(|t| primary(t)).chain(std::iter::once("en".to_string())) {
        if catalog.has(&t) && !out.contains(&t) {
            out.push(t);
        }
        if out.len() == 2 {
            break;
        }
    }
    if out.is_empty() {
        if let Some(l) = catalog.languages.iter().find(|l| l.windows.is_some() || l.tesseract) {
            out.push(l.tag.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Guessing the language of a text
// ---------------------------------------------------------------------------------------------

/// Very common words, per language (lower case). Shared words (`en`, `de`, `i`) are fine: a text is
/// scored against every list and the best one wins.
const STOPWORDS: &[(&str, &[&str])] = &[
    ("en", &["the", "and", "of", "to", "in", "is", "that", "for", "with", "on", "are", "as", "was", "be", "by", "it", "this", "from", "at", "or", "not", "have", "has", "will", "an", "which", "their", "all", "you", "we", "they", "but", "his", "her", "been", "were", "would", "there", "what", "when", "who"]),
    ("sv", &["och", "att", "det", "som", "en", "är", "på", "för", "med", "av", "den", "till", "har", "inte", "om", "ett", "de", "vi", "kan", "jag", "var", "från", "men", "ska", "sig", "så", "eller", "också", "han", "hon", "vid", "mycket", "sin"]),
    ("de", &["der", "die", "und", "das", "nicht", "ist", "ein", "zu", "mit", "von", "den", "auf", "für", "sich", "dem", "eine", "auch", "es", "im", "als", "aber", "wird", "sind", "nach", "bei", "oder", "wie", "nur", "noch", "dass"]),
    ("fr", &["le", "la", "les", "des", "et", "est", "une", "pour", "dans", "que", "qui", "pas", "en", "du", "un", "au", "sur", "avec", "ne", "se", "ce", "sont", "plus", "par", "je", "il", "elle", "nous", "vous", "mais", "ou"]),
    ("es", &["el", "los", "las", "una", "por", "con", "para", "del", "es", "en", "que", "se", "un", "su", "al", "lo", "como", "más", "pero", "sus", "le", "ya", "este", "porque", "esta", "entre", "cuando", "muy"]),
    ("it", &["il", "lo", "la", "le", "gli", "di", "che", "è", "un", "una", "per", "con", "non", "sono", "del", "della", "dei", "nel", "alla", "più", "come", "anche", "ma", "si", "al"]),
    ("nl", &["de", "het", "een", "van", "en", "in", "is", "dat", "op", "te", "zijn", "met", "voor", "niet", "aan", "er", "ook", "als", "bij", "maar", "om", "dan", "nog"]),
    ("pt", &["de", "do", "da", "dos", "das", "em", "um", "uma", "para", "com", "não", "que", "se", "por", "mais", "mas", "como", "foi", "ao", "os", "as"]),
    ("da", &["og", "i", "at", "det", "er", "en", "til", "på", "med", "for", "den", "af", "som", "ikke", "har", "de", "et", "var", "jeg", "om", "men", "sig"]),
    ("nb", &["og", "i", "er", "det", "på", "til", "en", "av", "for", "med", "som", "ikke", "har", "den", "et", "de", "var", "jeg", "om", "men", "seg"]),
    ("fi", &["ja", "on", "ei", "että", "se", "oli", "hän", "mutta", "kun", "niin", "ovat", "tai", "jos", "myös", "kuin", "olla", "mitä"]),
    ("pl", &["i", "w", "nie", "na", "to", "się", "z", "że", "do", "jest", "jak", "ale", "po", "co", "tak", "za", "od", "czy", "dla"]),
];

/// The script a text is mostly written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Script {
    Latin,
    Cyrillic,
    Greek,
    Arabic,
    Hebrew,
    Han,
    Kana,
    Hangul,
    Thai,
    Devanagari,
    Other,
}

fn script_of(c: char) -> Option<Script> {
    Some(match c as u32 {
        0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF => Script::Latin,
        0x370..=0x3FF | 0x1F00..=0x1FFF => Script::Greek,
        0x400..=0x52F => Script::Cyrillic,
        0x590..=0x5FF => Script::Hebrew,
        0x600..=0x6FF | 0x750..=0x77F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
        0x900..=0x97F => Script::Devanagari,
        0xE00..=0xE7F => Script::Thai,
        0x3040..=0x30FF | 0x31F0..=0x31FF => Script::Kana,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF => Script::Han,
        0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => Script::Hangul,
        _ if c.is_alphabetic() => Script::Other,
        _ => return None,
    })
}

/// A language guess with the evidence behind it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Guess {
    /// Primary tag.
    pub tag: String,
    /// 0..1: share of the words that are stopwords of that language (Latin), or of the letters in
    /// that script (others).
    pub confidence: f32,
    pub script: Script,
    /// How many words/letters the guess rests on.
    pub sample: usize,
}

/// The words of `text` as lower-case alphabetic tokens.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphabetic() && c != '\'' && c != '’')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Stopword hit rate of `text` for one language (primary tag); `None` for a language without a list.
pub fn stopword_rate(text: &str, lang: &str) -> Option<f32> {
    let list = STOPWORDS.iter().find(|(l, _)| *l == primary(lang))?.1;
    let toks = tokens(text);
    if toks.is_empty() {
        return Some(0.0);
    }
    let hits = toks.iter().filter(|t| list.contains(&t.as_str())).count();
    Some(hits as f32 / toks.len() as f32)
}

/// The language of `text`, or `None` when there is too little of it, or nothing stands out.
/// Non-Latin scripts decide by script (kana makes Han text Japanese; Cyrillic and Arabic default to
/// `ru` and `ar`: a finer split needs more than stopwords); Latin text by the best stopword rate.
pub fn guess(text: &str) -> Option<Guess> {
    let mut counts = [0usize; 11];
    let mut letters = 0usize;
    for c in text.chars() {
        if let Some(s) = script_of(c) {
            counts[s as usize] += 1;
            letters += 1;
        }
    }
    if letters < 12 {
        return None;
    }
    let order = [
        Script::Latin,
        Script::Cyrillic,
        Script::Greek,
        Script::Arabic,
        Script::Hebrew,
        Script::Han,
        Script::Kana,
        Script::Hangul,
        Script::Thai,
        Script::Devanagari,
        Script::Other,
    ];
    let (top, n) = order.iter().map(|s| (*s, counts[*s as usize])).max_by_key(|(_, n)| *n)?;
    let share = n as f32 / letters as f32;
    if share < 0.6 {
        return None;
    }
    let tag = match top {
        Script::Cyrillic => "ru",
        Script::Greek => "el",
        Script::Arabic => "ar",
        Script::Hebrew => "he",
        Script::Hangul => "ko",
        Script::Thai => "th",
        Script::Devanagari => "hi",
        Script::Kana => "ja",
        Script::Han => {
            // Japanese text mixes kana in; pure Han is taken as Chinese.
            if counts[Script::Kana as usize] * 10 >= counts[Script::Han as usize] { "ja" } else { "zh" }
        }
        Script::Other => return None,
        Script::Latin => {
            let toks = tokens(text);
            if toks.len() < 8 {
                return None;
            }
            let mut scored: Vec<(&str, f32)> = STOPWORDS
                .iter()
                .map(|(l, list)| (*l, toks.iter().filter(|t| list.contains(&t.as_str())).count() as f32 / toks.len() as f32))
                .collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            let (best, rate) = scored[0];
            let next = scored.get(1).map_or(0.0, |s| s.1);
            // Real running text has 20-45% stopwords; junk, a few percent. Close seconds (da/nb) are
            // told apart only by a clear margin.
            if rate < 0.10 || rate < next * 1.08 {
                return None;
            }
            return Some(Guess { tag: best.to_string(), confidence: rate, script: Script::Latin, sample: toks.len() });
        }
    };
    Some(Guess { tag: tag.to_string(), confidence: share, script: top, sample: letters })
}

/// Which of `candidates` (primary tags) a sample read with each of them agrees with best, given the
/// text each reading produced. A candidate wins by the stopword rate of its own list on its own
/// reading; one without a stopword list (a non-Latin language) wins by the share of letters in its
/// script. `None` when nothing is convincing.
pub fn best_reading(readings: &[(String, String)]) -> Option<String> {
    let mut best: Option<(f32, &str)> = None;
    for (lang, text) in readings {
        let score = match stopword_rate(text, lang) {
            Some(r) => r,
            None => {
                let g = guess(text)?;
                if primary(lang) == g.tag { g.confidence * 0.5 } else { 0.0 }
            }
        };
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, lang));
        }
    }
    best.filter(|(s, _)| *s >= 0.08).map(|(_, l)| primary(l))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn cat(win: &[&str], tess: Option<&[&str]>) -> Catalog {
        let t = tess.map(s);
        Catalog::build(&s(win), t.as_deref(), tess.map(|_| r"C:\Tesseract-OCR\tessdata".to_string()))
    }

    #[test]
    fn catalog_merges_both_engines_per_language() {
        let c = cat(&["en-US", "sv-SE", "ja"], Some(&["en", "sv", "de"]));
        let tags: Vec<&str> = c.languages.iter().map(|l| l.tag.as_str()).collect();
        assert_eq!(tags, ["de", "en", "ja", "sv"]);
        let sv = c.get("sv-SE").unwrap();
        assert_eq!(sv.windows.as_deref(), Some("sv-SE"));
        assert!(sv.tesseract);
        let de = c.get("de").unwrap();
        assert!(de.windows.is_none() && de.tesseract);
        assert!(c.get("ja").unwrap().experimental, "CJK is marked, not hidden");
        assert!(c.tesseract_found);
        assert!(!cat(&["en-US"], None).tesseract_found);
    }

    #[test]
    fn one_installed_language_goes_to_windows() {
        let c = cat(&["en-US", "sv-SE"], Some(&["en", "sv"]));
        let p = pick_engine(&s(&["sv"]), &c, None);
        assert_eq!((p.engine, p.languages), (Some(Engine::Windows), s(&["sv-SE"])));
        assert!(p.dropped.is_empty() && p.missing.is_empty() && p.note.is_none());
    }

    #[test]
    fn several_languages_or_one_windows_lacks_go_to_tesseract() {
        let c = cat(&["en-US", "sv-SE"], Some(&["eng", "swe", "deu"].map(|t| primary_of_iso(t)).as_slice()));
        let p = pick_engine(&s(&["sv", "en"]), &c, None);
        assert_eq!((p.engine, p.languages), (Some(Engine::Tesseract), s(&["sv", "en"])));
        let p = pick_engine(&s(&["de"]), &c, None);
        assert_eq!((p.engine, p.languages), (Some(Engine::Tesseract), s(&["de"])));
        // the user's own pick of Tesseract for one language is honoured
        let p = pick_engine(&s(&["en"]), &c, Some(Engine::Tesseract));
        assert_eq!(p.engine, Some(Engine::Tesseract));
    }

    fn primary_of_iso(iso: &str) -> &'static str {
        ISO3.iter().find(|(_, b)| *b == iso).map(|(a, _)| *a).unwrap()
    }

    #[test]
    fn a_language_nobody_has_says_how_to_add_it_and_the_rest_is_still_read() {
        let c = cat(&["en-US"], None);
        let p = pick_engine(&s(&["en", "sv"]), &c, None);
        assert_eq!(p.engine, Some(Engine::Windows));
        assert_eq!(p.languages, s(&["en-US"]));
        assert_eq!(p.dropped, s(&["sv"]));
        assert_eq!(p.missing.len(), 1);
        assert_eq!(p.missing[0].tag, "sv");
        assert_eq!(p.missing[0].windows_settings, "ms-settings:regionlanguage");
        assert_eq!(p.missing[0].tessdata_file, "swe.traineddata");
        assert!(p.note.is_some());

        let none = pick_engine(&s(&["ar"]), &c, None);
        assert_eq!(none.engine, None);
        assert_eq!(none.missing[0].tessdata_file, "ara.traineddata");
    }

    #[test]
    fn several_installed_windows_languages_without_tesseract_use_the_first_and_say_so() {
        let c = cat(&["en-US", "sv-SE"], None);
        let p = pick_engine(&s(&["sv", "en"]), &c, None);
        assert_eq!((p.engine, p.languages.clone()), (Some(Engine::Windows), s(&["sv-SE"])));
        assert_eq!(p.dropped, s(&["en"]));
        assert!(p.missing.is_empty());
        assert!(p.note.unwrap().contains("Tesseract"));
    }

    #[test]
    fn default_is_the_display_language_plus_english() {
        let c = cat(&["en-US", "sv-SE", "ja"], None);
        assert_eq!(default_languages(&s(&["sv-SE", "en-GB"]), &c), s(&["sv", "en"]));
        assert_eq!(default_languages(&s(&["de-DE"]), &c), s(&["en"]), "a language that is not installed is skipped");
        assert_eq!(default_languages(&s(&["fr-FR"]), &cat(&["ja"], None)), s(&["ja"]), "falls back to what exists");
    }

    #[test]
    fn language_of_running_text() {
        let en = "The committee has reviewed the proposal and it will be sent to all of the members before the meeting that is planned for next week.";
        let sv = "Styrelsen har granskat förslaget och det kommer att skickas till alla medlemmar före mötet som är planerat till nästa vecka.";
        let de = "Der Ausschuss hat den Vorschlag geprüft und er wird allen Mitgliedern vor der Sitzung zugeschickt, die für nächste Woche geplant ist.";
        let fr = "Le comité a examiné la proposition et elle sera envoyée à tous les membres avant la réunion qui est prévue pour la semaine prochaine.";
        assert_eq!(guess(en).unwrap().tag, "en");
        assert_eq!(guess(sv).unwrap().tag, "sv");
        assert_eq!(guess(de).unwrap().tag, "de");
        assert_eq!(guess(fr).unwrap().tag, "fr");
    }

    #[test]
    fn scripts_decide_for_non_latin_text() {
        assert_eq!(guess("Это простой текст на русском языке для проверки").unwrap().tag, "ru");
        assert_eq!(guess("هذا نص عربي بسيط للاختبار والتحقق من اللغة").unwrap().tag, "ar");
        assert_eq!(guess("זהו טקסט פשוט בעברית לבדיקת השפה והכתב").unwrap().tag, "he");
        assert_eq!(guess("これは日本語の文章です。言語の確認のための簡単なテキストです。").unwrap().tag, "ja");
        assert_eq!(guess("这是一个用于检查语言的简单中文文本内容示例").unwrap().tag, "zh");
        assert_eq!(guess("이것은 언어를 확인하기 위한 간단한 한국어 문장입니다").unwrap().tag, "ko");
    }

    #[test]
    fn too_little_or_junk_text_is_no_guess() {
        assert!(guess("Hello there").is_none());
        assert!(guess("xqz vkd wrt plm bnv cxz qwe rty uio asd fgh jkl zxc").is_none());
        assert!(guess("").is_none());
    }

    #[test]
    fn a_sample_read_picks_the_candidate_that_reads_as_its_language() {
        let sv_read = "Styrelsen har granskat förslaget och det kommer att skickas till alla medlemmar som är med på mötet";
        let junk = "Styrelsen har granskat forslaget och det kommer att skickas till alla medlemmar som ar med pa motet";
        let r = vec![("en".to_string(), junk.to_string()), ("sv".to_string(), sv_read.to_string())];
        assert_eq!(best_reading(&r).as_deref(), Some("sv"));
        assert_eq!(best_reading(&[("en".to_string(), "x y z".to_string())]), None);
    }
}
