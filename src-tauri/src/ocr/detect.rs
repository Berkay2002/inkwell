//! The language a document is probably written in, so the sheet can start on it.
//!
//! In order: the text the document already has (pages with a real text layer, no OCR needed), then a
//! quick read of the first page that needs OCR with a few installed candidates (100 dpi, the one whose
//! reading looks most like its own language wins), then the user's Windows languages. Never
//! downloads or installs anything, and reads at most [`MAX_CANDIDATES`] times.

use std::sync::atomic::{AtomicBool, Ordering};

use mupdf::TextPageFlags;
use serde::Serialize;

use super::backend::Recognizer;
use super::lang::{self, Catalog, primary};
use super::{Input, MIN_TEXT_CHARS, OcrError, render};

/// Sample reads per detection: each is 0.1-0.3 s with Windows OCR.
pub const MAX_CANDIDATES: usize = 5;
/// Resolution of the sample read: enough for the stopword rate, a quarter of a normal page's cost.
const SAMPLE_DPI: u32 = 100;
/// Characters of existing text that are enough to decide.
const TEXT_ENOUGH: usize = 8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// From the text the document already has.
    Text,
    /// From a sample read of the first page that needs OCR.
    Sample,
    /// Nothing to go on: the user's Windows languages.
    Profile,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Detection {
    /// What to start the sheet on: primary tags, in priority order.
    pub languages: Vec<String>,
    pub source: Source,
    /// The language the evidence points at (it may not be installed; `languages` still names it so
    /// the sheet can say what is missing).
    pub guess: Option<String>,
    /// 0..1, how sure the guess is.
    pub confidence: f32,
}

fn reading_text(raw: &super::backend::RawPage) -> String {
    raw.lines.iter().map(|l| l.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n")
}

/// `windows` / `tesseract`: the installed engines (either may be absent). `profile`: the user's
/// Windows languages, display language first.
pub fn detect(
    input: &Input,
    catalog: &Catalog,
    profile: &[String],
    windows: Option<&dyn Recognizer>,
    tesseract: Option<&dyn Recognizer>,
    cancel: &AtomicBool,
) -> Result<Detection, OcrError> {
    let fallback = |source: Source| Detection {
        languages: lang::default_languages(profile, catalog),
        source,
        guess: None,
        confidence: 0.0,
    };
    let doc = input.open()?;
    let count = doc.page_count()?.max(0) as u32;

    // 1. Text the document already has.
    let mut text = String::new();
    let mut first_scan: Option<u32> = None;
    for i in 0..count.min(60) {
        if cancel.load(Ordering::Relaxed) {
            return Err(OcrError::Cancelled);
        }
        let t = doc.load_page(i as i32)?.to_text_page(TextPageFlags::empty())?.to_text()?;
        if t.chars().filter(|c| !c.is_whitespace()).count() >= MIN_TEXT_CHARS {
            if text.len() < TEXT_ENOUGH {
                text.push_str(&t);
                text.push('\n');
            }
        } else if first_scan.is_none() {
            first_scan = Some(i);
        }
        if text.len() >= TEXT_ENOUGH && first_scan.is_some() {
            break;
        }
    }
    if let Some(g) = lang::guess(&text) {
        return Ok(Detection { languages: vec![g.tag.clone()], source: Source::Text, guess: Some(g.tag), confidence: g.confidence });
    }

    // 2. A sample read with a few candidates the machine can actually read.
    let mut candidates: Vec<String> = Vec::new();
    for t in profile.iter().map(|t| primary(t)).chain(std::iter::once("en".to_string())).chain(catalog.languages.iter().map(|l| l.tag.clone())) {
        if catalog.has(&t) && !candidates.contains(&t) {
            candidates.push(t);
        }
    }
    candidates.truncate(MAX_CANDIDATES);
    if candidates.len() < 2 && catalog.languages.iter().filter(|l| l.windows.is_some() || l.tesseract).count() < 2 {
        // One language to choose from: nothing to detect.
        return Ok(fallback(Source::Profile));
    }
    let page = first_scan.unwrap_or(0);
    let max_dim = windows.map(|w| w.max_dimension()).or_else(|| tesseract.map(|t| t.max_dimension())).unwrap_or(10_000);
    let (bmp, _) = render(&doc, page, SAMPLE_DPI, max_dim)?;
    let mut readings: Vec<(String, String)> = Vec::new();
    for cand in &candidates {
        if cancel.load(Ordering::Relaxed) {
            return Err(OcrError::Cancelled);
        }
        let info = catalog.get(cand);
        let (engine, tag) = match (info.and_then(|l| l.windows.clone()), windows, tesseract) {
            (Some(tag), Some(w), _) => (w, tag),
            (_, _, Some(t)) if info.is_some_and(|l| l.tesseract) => (t, cand.clone()),
            _ => continue,
        };
        match engine.recognize(&bmp, std::slice::from_ref(&tag), cancel) {
            Ok(raw) => readings.push((cand.clone(), reading_text(&raw))),
            Err(OcrError::Cancelled) => return Err(OcrError::Cancelled),
            Err(_) => {}
        }
    }
    if let Some(best) = lang::best_reading(&readings) {
        let confidence = readings
            .iter()
            .find(|(l, _)| *l == best)
            .and_then(|(l, t)| lang::stopword_rate(t, l))
            .unwrap_or(0.0);
        return Ok(Detection { languages: vec![best.clone()], source: Source::Sample, guess: Some(best), confidence });
    }

    // 3. Nothing convincing.
    Ok(fallback(Source::Profile))
}

#[cfg(test)]
mod tests {
    use super::super::backend::{Bitmap, RawLine, RawPage, RawWord};
    use super::*;
    use std::sync::Mutex;

    /// Answers each language with a fixed text; records which languages it was asked for.
    struct Reads {
        texts: Vec<(&'static str, &'static str)>,
        asked: Mutex<Vec<String>>,
    }

    impl Recognizer for Reads {
        fn name(&self) -> &'static str {
            "reads"
        }
        fn languages(&self) -> Result<Vec<String>, OcrError> {
            Ok(self.texts.iter().map(|(l, _)| l.to_string()).collect())
        }
        fn max_dimension(&self) -> u32 {
            10_000
        }
        fn recognize(&self, _: &Bitmap, languages: &[String], _: &AtomicBool) -> Result<RawPage, OcrError> {
            let l = languages.first().cloned().unwrap_or_default();
            self.asked.lock().unwrap().push(l.clone());
            let text = self.texts.iter().find(|(t, _)| primary(t) == primary(&l)).map_or("", |(_, t)| *t);
            let words = text.split_whitespace().map(|w| RawWord { text: w.into(), rect: [0.0, 0.0, 10.0, 10.0], conf: None }).collect();
            Ok(RawPage { lines: vec![RawLine { words }], angle_deg: None })
        }
    }

    const SV: &str = "Styrelsen har granskat förslaget och det kommer att skickas till alla medlemmar som är med på mötet";
    const EN_JUNK: &str = "Styrelsen har granskat forslaget och det kommer att skickas till alla medlemmar som ar med pa motet";

    fn scan_pdf() -> Vec<u8> {
        // A one-page PDF with no text: an empty page.
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offs = Vec::new();
        for (n, body) in [
            (1, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>"),
        ] {
            offs.push(out.len());
            out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for o in &offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    fn text_pdf(text: &str) -> Vec<u8> {
        let c = format!("BT /F1 12 Tf 1 0 0 1 20 150 Tm ({text}) Tj ET\n");
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offs = Vec::new();
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2000 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{c}endstream", c.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        for (i, body) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in &offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn cat() -> Catalog {
        Catalog::build(&["en-US".to_string(), "sv-SE".to_string()], None, None)
    }

    #[test]
    fn existing_text_decides_without_reading_anything() {
        // ASCII only: the Type1 font would mangle the rest, and the stopwords are enough.
        let pdf = text_pdf("Styrelsen har granskat forslaget och det kommer att skickas till alla medlemmar som ar med pa motet och att det var en del av det");
        let win = Reads { texts: vec![("en-US", "x"), ("sv-SE", "x")], asked: Mutex::new(Vec::new()) };
        let d = detect(&Input::Bytes(&pdf), &cat(), &["sv-SE".into()], Some(&win), None, &AtomicBool::new(false)).unwrap();
        assert_eq!(d.source, Source::Text);
        assert_eq!(d.guess.as_deref(), Some("sv"));
        assert!(win.asked.lock().unwrap().is_empty(), "no OCR was needed to know");
    }

    #[test]
    fn a_scan_is_sampled_with_the_candidates_and_the_best_reading_wins() {
        let pdf = scan_pdf();
        let win = Reads { texts: vec![("en-US", EN_JUNK), ("sv-SE", SV)], asked: Mutex::new(Vec::new()) };
        // English is first in the profile: Swedish still wins on its reading.
        let d = detect(&Input::Bytes(&pdf), &cat(), &["en-US".into(), "sv-SE".into()], Some(&win), None, &AtomicBool::new(false)).unwrap();
        assert_eq!((d.source, d.languages.clone(), d.guess.as_deref()), (Source::Sample, vec!["sv".to_string()], Some("sv")));
        assert_eq!(win.asked.lock().unwrap().len(), 2);
    }

    #[test]
    fn nothing_convincing_falls_back_to_the_profile_languages() {
        let pdf = scan_pdf();
        let win = Reads { texts: vec![("en-US", "x y"), ("sv-SE", "q")], asked: Mutex::new(Vec::new()) };
        let d = detect(&Input::Bytes(&pdf), &cat(), &["sv-SE".into()], Some(&win), None, &AtomicBool::new(false)).unwrap();
        assert_eq!((d.source, d.languages), (Source::Profile, vec!["sv".to_string(), "en".to_string()]));
    }

    #[test]
    fn a_cancelled_detection_says_so() {
        let pdf = scan_pdf();
        let win = Reads { texts: vec![("en-US", "x"), ("sv-SE", "x")], asked: Mutex::new(Vec::new()) };
        let stop = AtomicBool::new(true);
        let r = detect(&Input::Bytes(&pdf), &cat(), &[], Some(&win), None, &stop);
        assert!(matches!(r, Err(OcrError::Cancelled)));
    }
}
