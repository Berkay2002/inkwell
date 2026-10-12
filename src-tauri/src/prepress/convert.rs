//! Convert colours (Acrobat's "Convert Colors"): RGB and gray, and spot colours, become CMYK, with the
//! values computed through the print condition's ICC profile (MuPDF's lcms2) or, without one, MuPDF's
//! own conversion. One undo step; text, vectors, shadings and images all convert.
//!
//! The pages are rewritten by MuPDF's colour filter (`vendor/mupdf-sys/wrapper/prepress.c`); this module
//! picks the profile, then closes the gaps the filter leaves (transparency groups' blending space) and
//! sets the output intent. What colours the file still uses is read with the PDF/X checker's survey.

use super::{Result, bad};
use crate::a11y::Progress;
use crate::engine::edit::{EditSource, EditState, Saved};
use crate::engine::{DocId, Engine};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Buffer, ConvertColors, IccColorspace, IccKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where the CMYK values come from.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum IntentSource {
    /// MuPDF's plain conversion (no profile).
    #[default]
    Device,
    /// The CMYK profile of the document's own output intent.
    Document,
    /// The profile passed in; it also becomes the document's PDF/X output intent.
    Profile,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConvertSpec {
    /// 0-based; every page when absent.
    pub pages: Option<Vec<u32>>,
    pub gray: bool,
    pub rgb: bool,
    pub spot: bool,
    /// Gray becomes K alone.
    #[serde(default = "yes")]
    pub gray_to_k: bool,
    /// Pure black (gray 0, RGB 0 0 0) becomes K alone, however the profile would mix it.
    #[serde(default = "yes")]
    pub preserve_black: bool,
    #[serde(default)]
    pub intent: IntentSource,
    /// The name written as the output intent's condition when `intent` is `Profile`.
    pub condition: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConvertReport {
    /// Pages converted (0-based).
    pub pages: Vec<u32>,
    /// DeviceGray, DeviceRGB, DeviceCMYK still in use anywhere in the file afterwards (not counting
    /// ones a Default colour space covers).
    pub remaining: [bool; 3],
    pub used_profile: bool,
    pub notes: Vec<String>,
}

/// Checks `data` is a CMYK ICC profile.
fn check_cmyk_profile(data: &[u8]) -> Result<()> {
    if data.len() < 128 || &data[36..40] != b"acsp" {
        return Err(bad("The file is not an ICC profile"));
    }
    if &data[16..20] != b"CMYK" {
        return Err(bad("The profile is not a CMYK profile"));
    }
    if (u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize) > data.len() {
        return Err(bad("The ICC profile is cut short"));
    }
    Ok(())
}

fn name_of(o: &PdfObject, key: &str) -> Option<String> {
    o.get_dict(key).ok().flatten().and_then(|v| v.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned())
}

/// The CMYK profile of the first output intent that has one.
fn document_profile(doc: &PdfDocument) -> Result<Option<Vec<u8>>> {
    let cat = doc.catalog()?;
    let Some(arr) = cat.get_dict("OutputIntents")?.filter(|o| o.is_array().unwrap_or(false)) else { return Ok(None) };
    for oi in arr.array_iter()?.flatten() {
        if let Some(p) = oi.get_dict("DestOutputProfile")?.filter(|p| p.is_stream().unwrap_or(false)) {
            let data = p.read_stream()?;
            if check_cmyk_profile(&data).is_ok() {
                return Ok(Some(data));
            }
        }
    }
    Ok(None)
}

/// What a group's blending space is made of: `Some(true)` RGB-like (DeviceRGB, CalRGB, Lab, a 3-component
/// ICC profile), `Some(false)` gray-like, None for CMYK and anything else.
fn blending_space(cs: &PdfObject) -> Option<bool> {
    if cs.is_name().ok()? {
        return match String::from_utf8_lossy(&cs.as_name().ok()?).as_ref() {
            "DeviceRGB" => Some(true),
            "DeviceGray" => Some(false),
            _ => None,
        };
    }
    if !cs.is_array().ok()? {
        return None;
    }
    let first = cs.get_array(0).ok().flatten()?;
    match String::from_utf8_lossy(&first.as_name().ok()?).as_ref() {
        "CalRGB" | "Lab" => Some(true),
        "CalGray" => Some(false),
        "ICCBased" => match cs.get_array(1).ok().flatten()?.get_dict("N").ok().flatten()?.as_int().ok()? {
            3 => Some(true),
            1 => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// A transparency group of the page or a form whose blending space is a converted kind is set to DeviceCMYK.
fn fix_groups(doc: &PdfDocument, page: &PdfObject, spec: &ConvertSpec, seen: &mut HashSet<i32>) -> Result<()> {
    let want = |rgb: bool| if rgb { spec.rgb } else { spec.gray };
    fn group(o: &PdfObject, want: &dyn Fn(bool) -> bool) -> Result<()> {
        if let Some(mut g) = o.get_dict("Group")?.filter(|g| g.is_dict().unwrap_or(false)) {
            if g.get_dict("CS")?.and_then(|cs| blending_space(&cs)).is_some_and(want) {
                g.dict_put("CS", PdfObject::new_name("DeviceCMYK")?)?;
            }
        }
        Ok(())
    }
    group(page, &want)?;
    // forms nested in the page's resources
    let mut stack = vec![page.clone()];
    while let Some(holder) = stack.pop() {
        let Some(res) = holder.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false)) else { continue };
        let Some(xo) = res.get_dict("XObject")?.filter(|x| x.is_dict().unwrap_or(false)) else { continue };
        for i in 0..xo.dict_len()? {
            let Some(form) = xo.get_dict_val(i as i32)? else { continue };
            if name_of(&form, "Subtype").as_deref() != Some("Form") {
                continue;
            }
            if let Ok(num) = form.as_indirect() {
                if num != 0 && !seen.insert(num) {
                    continue;
                }
            }
            group(&form, &want)?;
            stack.push(form);
        }
    }
    let _ = doc;
    Ok(())
}

/// Writes (or replaces) the document's PDF/X output intent with `profile`.
fn set_output_intent(doc: &mut PdfDocument, profile: &[u8], condition: &str) -> Result<()> {
    let mut cat = doc.catalog()?;
    let mut sd = doc.new_dict()?;
    sd.dict_put("N", PdfObject::new_int(4)?)?;
    sd.dict_put("Alternate", PdfObject::new_name("DeviceCMYK")?)?;
    let stream = doc.add_stream(&Buffer::from_bytes(profile)?, Some(&sd), false)?;
    let mut oi = doc.new_dict()?;
    oi.dict_put("Type", PdfObject::new_name("OutputIntent")?)?;
    oi.dict_put("S", PdfObject::new_name("GTS_PDFX")?)?;
    oi.dict_put("OutputConditionIdentifier", PdfObject::new_string(condition)?)?;
    oi.dict_put("Info", PdfObject::new_string(condition)?)?;
    oi.dict_put("DestOutputProfile", stream)?;
    let oi = doc.add_object(&oi)?;
    // the other PDF/X intents are replaced; intents of other kinds stay
    let mut arr = doc.new_array()?;
    if let Some(old) = cat.get_dict("OutputIntents")?.filter(|o| o.is_array().unwrap_or(false)) {
        for e in old.array_iter()?.flatten() {
            if name_of(&e, "S").as_deref() != Some("GTS_PDFX") {
                arr.array_push(e)?;
            }
        }
    }
    arr.array_push(oi)?;
    cat.dict_put("OutputIntents", arr)?;
    Ok(())
}

/// Converts the colours `spec` picks, as one undo step. `profile` is the CMYK ICC profile for
/// `IntentSource::Profile`.
pub fn convert_colors(
    eng: &Engine,
    doc: DocId,
    spec: &ConvertSpec,
    profile: Option<&[u8]>,
    source: EditSource,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(EditState, ConvertReport)> {
    convert(eng, doc, spec, profile, source, progress, cancel, None).map(|(state, report, _)| (state, report))
}

/// [`convert_colors`] and then a Save as `to` under the same edit guard, so that nothing (an undo, another
/// edit) can land between the conversion and the file. The conversion stands when the write fails: the third
/// value is the save's own result. The document's own file is refused before anything is converted.
#[allow(clippy::too_many_arguments)]
pub fn convert_colors_and_save(
    eng: &Engine,
    doc: DocId,
    spec: &ConvertSpec,
    profile: Option<&[u8]>,
    source: EditSource,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
    to: &str,
) -> Result<(EditState, ConvertReport, Result<(EditState, Option<Saved>)>)> {
    let (state, report, saved) = convert(eng, doc, spec, profile, source, progress, cancel, Some(to))?;
    Ok((state, report, saved.expect("a save was asked for")))
}

#[allow(clippy::too_many_arguments)]
fn convert(
    eng: &Engine,
    doc: DocId,
    spec: &ConvertSpec,
    profile: Option<&[u8]>,
    source: EditSource,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
    to: Option<&str>,
) -> Result<(EditState, ConvertReport, Option<Result<(EditState, Option<Saved>)>>)> {
    if !(spec.gray || spec.rgb || spec.spot) {
        return Err(bad("Pick the colours to convert: RGB, gray or spot colours"));
    }
    if spec.intent == IntentSource::Profile {
        check_cmyk_profile(profile.ok_or_else(|| bad("Choose a CMYK ICC profile"))?)?;
    }
    let mut report: Option<ConvertReport> = None;
    let label = match spec.intent {
        IntentSource::Device => "Convert colours to CMYK".to_string(),
        IntentSource::Document => "Convert colours to the output intent".to_string(),
        IntentSource::Profile => format!("Convert colours to {}", spec.condition.as_deref().unwrap_or("CMYK")),
    };
    let step = |d: &mut PdfDocument| -> Result<Vec<u32>> {
        super::refuse_certified(d, "Converting its colours")?;
        let count = d.page_count()?.max(0) as u32;
        let pages: Vec<u32> = match &spec.pages {
            Some(p) => {
                if let Some(bad_page) = p.iter().find(|&&n| n >= count) {
                    return Err(crate::engine::EngineError::BadPage(*bad_page));
                }
                let mut p = p.clone();
                p.sort_unstable();
                p.dedup();
                p
            }
            None => (0..count).collect(),
        };
        let mut notes = Vec::new();
        let bytes: Option<Vec<u8>> = match spec.intent {
            IntentSource::Device => None,
            IntentSource::Profile => profile.map(<[u8]>::to_vec),
            IntentSource::Document => match document_profile(d)? {
                Some(b) => Some(b),
                None => return Err(bad("The document has no CMYK output intent to convert to")),
            },
        };
        let icc = bytes.as_deref().map(|b| IccColorspace::new(IccKind::Cmyk, b)).transpose()?;
        let what = ConvertColors { gray: spec.gray, rgb: spec.rgb, spot: spec.spot, gray_to_k: spec.gray_to_k, preserve_black: spec.preserve_black };
        let mut seen = HashSet::new();
        for (i, &p) in pages.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(bad("cancelled"));
            }
            d.convert_colors(p as i32, &what, icc.as_ref())?;
            let page = d.find_page(p as i32)?;
            fix_groups(d, &page, spec, &mut seen)?;
            progress(Progress { done: i as u32 + 1, pages: pages.len() as u32, stage: "convert" });
        }
        if spec.intent == IntentSource::Profile {
            let name = spec.condition.clone().filter(|c| !c.trim().is_empty()).unwrap_or_else(|| "Custom CMYK".into());
            set_output_intent(d, bytes.as_deref().unwrap_or_default(), &name)?;
            notes.push(format!("Set the output intent to {name}"));
        }
        if spec.rgb || spec.gray {
            notes.push("Images were rewritten as uncompressed CMYK; a recompress (Compress) makes them small again".into());
        }
        let remaining = crate::preflight::device_colours(d, &mut |_| {}, cancel)?;
        report = Some(ConvertReport { pages: pages.clone(), remaining, used_profile: icc.is_some(), notes });
        Ok(pages)
    };
    let (state, saved) = match to {
        None => (eng.apply_custom(doc, label, source, false, step)?, None),
        Some(to) => {
            let (state, saved) = eng.apply_custom_and_save(doc, label, source, to, step)?;
            (state, Some(saved))
        }
    };
    Ok((state, report.expect("set by the step"), saved))
}
