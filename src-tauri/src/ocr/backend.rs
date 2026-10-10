//! Recognition backends. A backend turns a grey bitmap into words with pixel boxes; everything else
//! (rendering, coordinates, the PDF text layer) lives outside and is backend-independent.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::OcrError;

/// 8-bit grey page raster, tightly packed (`pixels.len() == w * h`).
pub struct Bitmap {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
}

impl Bitmap {
    /// Turned clockwise by 90, 180 or 270 degrees (anything else returns a copy).
    pub fn rotated(&self, cw_deg: u16) -> Bitmap {
        let (w, h) = (self.w as usize, self.h as usize);
        let px = &self.pixels;
        let (nw, nh) = if cw_deg % 180 == 90 { (h, w) } else { (w, h) };
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                // destination of source pixel (x, y)
                let (dx, dy) = match cw_deg % 360 {
                    90 => (h - 1 - y, x),
                    180 => (w - 1 - x, h - 1 - y),
                    270 => (y, w - 1 - x),
                    _ => (x, y),
                };
                out[dy * nw + dx] = px[y * w + x];
            }
        }
        Bitmap { w: nw as u32, h: nh as u32, pixels: out }
    }

    /// Half size by 2x2 averaging (orientation trials do not need full resolution).
    pub fn halved(&self) -> Bitmap {
        let (w, h) = ((self.w / 2).max(1) as usize, (self.h / 2).max(1) as usize);
        let sw = self.w as usize;
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let at = |dx: usize, dy: usize| self.pixels[((2 * y + dy).min(self.h as usize - 1)) * sw + (2 * x + dx).min(sw - 1)] as u32;
                out.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4) as u8);
            }
        }
        Bitmap { w: w as u32, h: h as u32, pixels: out }
    }
}

/// A recognised word; `rect` is `[x0, y0, x1, y1]` in bitmap pixels, y down.
#[derive(Clone, Debug)]
pub struct RawWord {
    pub text: String,
    pub rect: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct RawLine {
    pub words: Vec<RawWord>,
}

#[derive(Clone, Debug, Default)]
pub struct RawPage {
    pub lines: Vec<RawLine>,
    /// Text angle the engine reports (degrees, clockwise positive), when it reports one.
    pub angle_deg: Option<f32>,
}

pub trait Recognizer: Send + Sync {
    fn name(&self) -> &'static str;
    /// BCP-47 tags of the languages this backend can recognise right now.
    fn languages(&self) -> Result<Vec<String>, OcrError>;
    /// Largest bitmap side the engine accepts.
    fn max_dimension(&self) -> u32;
    /// `languages` is in priority order (BCP-47 tags); empty = the backend's default.
    fn recognize(&self, bmp: &Bitmap, languages: &[String]) -> Result<RawPage, OcrError>;
    /// Degrees (0/90/180/270) to turn `bmp` clockwise to make its text upright, when the engine has
    /// a dedicated detector (Tesseract OSD). `None` = no opinion; the caller then tries rotations
    /// with [`recognize`](Self::recognize).
    fn osd_rotation(&self, _bmp: &Bitmap) -> Option<u16> {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Windows.Media.Ocr
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
pub use winrt::WinRtOcr;

#[cfg(windows)]
mod winrt {
    use super::*;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::core::HSTRING;

    /// Windows' built-in OCR: no install, languages are the user's installed OCR language packs.
    #[derive(Default)]
    pub struct WinRtOcr;

    fn werr(e: windows::core::Error) -> OcrError {
        OcrError::Backend(format!("Windows.Media.Ocr: {e}"))
    }

    fn engine_for(tag: Option<&str>) -> Result<OcrEngine, OcrError> {
        let e = match tag {
            Some(tag) => {
                let lang = Language::CreateLanguage(&HSTRING::from(tag)).map_err(werr)?;
                if !OcrEngine::IsLanguageSupported(&lang).map_err(werr)? {
                    return Err(OcrError::LanguageUnavailable(tag.to_string()));
                }
                OcrEngine::TryCreateFromLanguage(&lang).map_err(werr)?
            }
            None => OcrEngine::TryCreateFromUserProfileLanguages().map_err(werr)?,
        };
        Ok(e)
    }

    impl Recognizer for WinRtOcr {
        fn name(&self) -> &'static str {
            "windows"
        }

        fn languages(&self) -> Result<Vec<String>, OcrError> {
            let v = OcrEngine::AvailableRecognizerLanguages().map_err(werr)?;
            let mut out = Vec::new();
            for l in v {
                out.push(l.LanguageTag().map_err(werr)?.to_string());
            }
            Ok(out)
        }

        fn max_dimension(&self) -> u32 {
            OcrEngine::MaxImageDimension().unwrap_or(10000)
        }

        fn recognize(&self, bmp: &Bitmap, languages: &[String]) -> Result<RawPage, OcrError> {
            // One engine per call: creation is cheap next to recognition and keeps this `Sync`.
            // The first requested language that has a pack wins (the API has no per-word confidence
            // to merge several engines' results with, see docs/ocr-engine.md).
            let engine = match languages.iter().find_map(|t| engine_for(Some(t)).ok()) {
                Some(e) => e,
                None => engine_for(languages.first().map(String::as_str))?,
            };
            let w = DataWriter::new().map_err(werr)?;
            w.WriteBytes(&bmp.pixels).map_err(werr)?;
            let buf = w.DetachBuffer().map_err(werr)?;
            let sb = SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Gray8, bmp.w as i32, bmp.h as i32)
                .map_err(werr)?;
            let res = engine.RecognizeAsync(&sb).map_err(werr)?.join().map_err(werr)?;

            let angle_deg = res.TextAngle().ok().and_then(|a| a.Value().ok()).map(|v| v as f32);
            let mut page = RawPage { lines: Vec::new(), angle_deg };
            for line in res.Lines().map_err(werr)? {
                let mut l = RawLine::default();
                for word in line.Words().map_err(werr)? {
                    let r = word.BoundingRect().map_err(werr)?;
                    l.words.push(RawWord {
                        text: word.Text().map_err(werr)?.to_string(),
                        rect: [r.X, r.Y, r.X + r.Width, r.Y + r.Height],
                    });
                }
                if !l.words.is_empty() {
                    page.lines.push(l);
                }
            }
            Ok(page)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tesseract (command-line, TSV word boxes)
// ---------------------------------------------------------------------------------------------

/// Tesseract 5 through its command line (`--oem 1`, TSV output). Needs `tesseract.exe` and a
/// `tessdata` folder with the `*.traineddata` files; neither ships with Windows.
pub struct TesseractCli {
    pub exe: PathBuf,
    pub tessdata: Option<PathBuf>,
}

const ISO3: &[(&str, &str)] = &[
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
];

impl TesseractCli {
    /// Where to look for `tesseract.exe`, in order: the host's `ocr.tesseractPath` setting
    /// (`configured`), `INKWELL_TESSERACT`, then `Tesseract-OCR	esseract.exe` under `%ProgramFiles%`
    /// and `%ProgramFiles(x86)%`. Neither `PATH` nor the current directory is searched, and no
    /// location is hard-coded. Existence is not checked here.
    pub fn candidates(configured: Option<&Path>, env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
        let mut c: Vec<PathBuf> = Vec::new();
        if let Some(p) = configured.filter(|p| !p.as_os_str().is_empty()) {
            c.push(p.to_path_buf());
        }
        if let Some(p) = env("INKWELL_TESSERACT").filter(|p| !p.is_empty()) {
            c.push(p.into());
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = env(var).filter(|d| !d.is_empty()) {
                c.push(PathBuf::from(dir).join("Tesseract-OCR").join("tesseract.exe"));
            }
        }
        c
    }

    /// First existing candidate that has a `tessdata` folder beside it. The resolved path is logged
    /// before it is ever run.
    pub fn discover(configured: Option<&Path>) -> Option<Self> {
        for cand in Self::candidates(configured, &|k| std::env::var_os(k)) {
            if !cand.is_file() {
                continue;
            }
            // Resolved absolute path. canonicalize's `\\?\` prefix on Windows breaks Tesseract's own
            // path handling (its --tessdata-dir), so it is stripped for plain drive paths.
            let exe = cand.canonicalize().unwrap_or(cand);
            let exe = match exe.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
                Some(plain) if !plain.starts_with(r"UNC\") => PathBuf::from(plain),
                _ => exe,
            };
            let Some(tessdata) = exe.parent().map(|d| d.join("tessdata")).filter(|d| d.is_dir()) else {
                eprintln!("ocr: skipping {} (no tessdata folder beside it)", exe.display());
                continue;
            };
            eprintln!("ocr: using Tesseract at {}", exe.display());
            return Some(Self { exe, tessdata: Some(tessdata) });
        }
        None
    }

    fn lang_arg(languages: &[String]) -> String {
        if languages.is_empty() {
            return "eng".into();
        }
        let v: Vec<String> = languages
            .iter()
            .map(|l| {
                let primary = l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase();
                ISO3.iter().find(|(a, _)| *a == primary).map(|(_, b)| b.to_string()).unwrap_or(primary)
            })
            .collect();
        v.join("+")
    }

    fn tessdata_dir(&self) -> Option<&Path> {
        self.tessdata.as_deref()
    }
}

/// Binary PGM (P5) is the cheapest format both sides can speak: no encoder needed.
fn pgm(bmp: &Bitmap) -> Vec<u8> {
    let mut v = format!("P5\n{} {}\n255\n", bmp.w, bmp.h).into_bytes();
    v.extend_from_slice(&bmp.pixels);
    v
}

impl Recognizer for TesseractCli {
    fn name(&self) -> &'static str {
        "tesseract"
    }

    fn languages(&self) -> Result<Vec<String>, OcrError> {
        let mut cmd = Command::new(&self.exe);
        cmd.arg("--list-langs");
        if let Some(d) = self.tessdata_dir() {
            cmd.arg("--tessdata-dir").arg(d);
        }
        let out = cmd.output().map_err(|e| OcrError::Backend(format!("tesseract: {e}")))?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty() && *l != "osd")
            .map(|l| ISO3.iter().find(|(_, b)| *b == l).map(|(a, _)| a.to_string()).unwrap_or_else(|| l.to_string()))
            .collect())
    }

    fn max_dimension(&self) -> u32 {
        32000
    }

    fn recognize(&self, bmp: &Bitmap, languages: &[String]) -> Result<RawPage, OcrError> {
        let mut cmd = Command::new(&self.exe);
        cmd.args(["stdin", "stdout", "--oem", "1", "--psm", "3", "-l"]).arg(Self::lang_arg(languages));
        if let Some(d) = self.tessdata_dir() {
            cmd.arg("--tessdata-dir").arg(d);
        }
        cmd.arg("tsv");
        // Pages already run in parallel: keep each process on one thread.
        cmd.env("OMP_THREAD_LIMIT", "1");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| OcrError::Backend(format!("tesseract: {e}")))?;
        let data = pgm(bmp);
        let mut stdin = child.stdin.take().expect("piped");
        // Feed on a thread so a full stdout pipe cannot deadlock a large page.
        let feeder = std::thread::spawn(move || stdin.write_all(&data));
        let out = child.wait_with_output().map_err(|e| OcrError::Backend(format!("tesseract: {e}")))?;
        let _ = feeder.join();
        if !out.status.success() {
            return Err(OcrError::Backend(format!(
                "tesseract exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(parse_tsv(&String::from_utf8_lossy(&out.stdout)))
    }

    fn osd_rotation(&self, bmp: &Bitmap) -> Option<u16> {
        let tessdata = self.tessdata_dir()?;
        if !tessdata.join("osd.traineddata").is_file() {
            return None;
        }
        let mut cmd = Command::new(&self.exe);
        cmd.args(["stdin", "stdout", "--psm", "0", "-l", "osd", "--tessdata-dir"]).arg(tessdata);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn().ok()?;
        let data = pgm(bmp);
        let mut stdin = child.stdin.take()?;
        let feeder = std::thread::spawn(move || stdin.write_all(&data));
        let out = child.wait_with_output().ok()?;
        let _ = feeder.join();
        parse_osd(&String::from_utf8_lossy(&out.stdout))
    }
}

/// `Rotate: 270` and `Orientation confidence: 11.3` from `--psm 0` output; low confidence = `None`.
pub(crate) fn parse_osd(text: &str) -> Option<u16> {
    let field = |key: &str| {
        text.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().parse::<f32>().ok())
    };
    let rotate = field("Rotate:")?;
    let confidence = field("Orientation confidence:")?;
    (confidence >= 2.0).then_some((rotate as u16) % 360)
}

/// Word rows (level 5) grouped by block/paragraph/line, in the order Tesseract emits them.
pub(crate) fn parse_tsv(tsv: &str) -> RawPage {
    let mut page = RawPage::default();
    let mut key = (u32::MAX, 0, 0, 0);
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.splitn(12, '\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let p = |i: usize| f[i].parse::<i32>().unwrap_or(0);
        let text = f[11].trim();
        if text.is_empty() || f[10].parse::<f32>().unwrap_or(0.0) < 0.0 {
            continue;
        }
        let k = (p(2) as u32, p(3), p(4), 0);
        if k != key {
            page.lines.push(RawLine::default());
            key = k;
        }
        let (l, t, w, h) = (p(6) as f32, p(7) as f32, p(8) as f32, p(9) as f32);
        page.lines.last_mut().unwrap().words.push(RawWord { text: text.to_string(), rect: [l, t, l + w, t + h] });
    }
    page
}
