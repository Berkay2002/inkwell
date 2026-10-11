//! Page transitions (/Trans) and auto-advance (/Dur): what full screen plays between pages.

use super::{Result, bad, get, put_int, put_name};
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};

/// The PDF transition styles (/S).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransStyle {
    Split,
    Blinds,
    Box,
    Wipe,
    Dissolve,
    Glitter,
    /// Replace: no effect.
    R,
    Fly,
    Push,
    Cover,
    Uncover,
    Fade,
}

impl TransStyle {
    const ALL: [(&'static str, TransStyle); 12] = [
        ("Split", Self::Split),
        ("Blinds", Self::Blinds),
        ("Box", Self::Box),
        ("Wipe", Self::Wipe),
        ("Dissolve", Self::Dissolve),
        ("Glitter", Self::Glitter),
        ("R", Self::R),
        ("Fly", Self::Fly),
        ("Push", Self::Push),
        ("Cover", Self::Cover),
        ("Uncover", Self::Uncover),
        ("Fade", Self::Fade),
    ];

    fn name(self) -> &'static str {
        Self::ALL.iter().find(|(_, s)| *s == self).map_or("R", |(n, _)| n)
    }

    fn from_name(n: &str) -> Option<Self> {
        Self::ALL.iter().find(|(name, _)| *name == n).map(|(_, s)| *s)
    }

    /// Styles that use /Dm (horizontal or vertical).
    fn has_dimension(self) -> bool {
        matches!(self, Self::Split | Self::Blinds)
    }

    /// Styles that use /M (in or out).
    fn has_motion(self) -> bool {
        matches!(self, Self::Split | Self::Box | Self::Fly)
    }

    /// Styles that use /Di (a direction in degrees).
    fn has_direction(self) -> bool {
        matches!(self, Self::Wipe | Self::Glitter | Self::Fly | Self::Cover | Self::Uncover | Self::Push)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    H,
    V,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    I,
    O,
}

/// A page's transition. Fields a style does not use are left out when written and read back as none.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub style: TransStyle,
    /// Seconds the effect takes (/D, the PDF default is 1).
    #[serde(default = "one")]
    pub duration: f32,
    pub dimension: Option<Dimension>,
    pub motion: Option<Motion>,
    /// 0 (left to right), 90 (bottom to top), 180, 270 (top to bottom) or 315 (glitter, diagonal).
    pub direction: Option<u32>,
}

fn one() -> f32 {
    1.0
}

/// What one page has.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PageTrans {
    pub transition: Option<Transition>,
    /// Seconds before full screen moves on by itself (/Dur).
    pub auto_advance: Option<f32>,
}

/// What the dialog sets on every page of a range; `None` fields remove what the pages have.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransChange {
    pub transition: Option<Transition>,
    pub auto_advance: Option<f32>,
}

fn dir_ok(d: u32) -> bool {
    matches!(d, 0 | 90 | 180 | 270 | 315)
}

impl TransChange {
    pub fn check(&self) -> Result<()> {
        if let Some(a) = self.auto_advance {
            if !a.is_finite() || !(0.0..=3600.0).contains(&a) {
                return Err(bad("auto-advance must be between 0 and 3600 seconds"));
            }
        }
        if let Some(t) = &self.transition {
            if !t.duration.is_finite() || !(0.0..=60.0).contains(&t.duration) {
                return Err(bad("a transition takes between 0 and 60 seconds"));
            }
            if t.direction.is_some_and(|d| !dir_ok(d)) {
                return Err(bad("a transition direction is 0, 90, 180, 270 or 315 degrees"));
            }
        }
        Ok(())
    }
}

/// The transition and auto-advance of page `page`.
pub fn read_page(doc: &PdfDocument, page: u32) -> Result<PageTrans> {
    let p = doc.find_page(page as i32)?;
    let auto_advance = get(&p, "Dur").and_then(|d| d.as_float().ok()).filter(|d| d.is_finite() && *d >= 0.0);
    let transition = get(&p, "Trans").and_then(|t| t.resolve().ok().flatten()).filter(|t| t.is_dict().unwrap_or(false)).map(|t| {
        let style = get(&t, "S").and_then(|s| s.as_name().ok()).and_then(|n| TransStyle::from_name(&String::from_utf8_lossy(&n))).unwrap_or(TransStyle::R);
        let name = |k: &str| get(&t, k).and_then(|v| v.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned());
        let dir = get(&t, "Di").and_then(|v| v.as_int().ok().filter(|d| *d >= 0).map(|d| d as u32).filter(|d| dir_ok(*d)));
        Transition {
            style,
            duration: get(&t, "D").and_then(|d| d.as_float().ok()).filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(1.0),
            dimension: name("Dm").and_then(|n| match n.as_str() {
                "H" => Some(Dimension::H),
                "V" => Some(Dimension::V),
                _ => None,
            }),
            motion: name("M").and_then(|n| match n.as_str() {
                "I" => Some(Motion::I),
                "O" => Some(Motion::O),
                _ => None,
            }),
            direction: dir,
        }
    });
    Ok(PageTrans { transition, auto_advance })
}

/// Every page's transition and auto-advance.
pub fn read_all(doc: &PdfDocument) -> Result<Vec<PageTrans>> {
    let n = doc.page_count()?.max(0) as u32;
    (0..n).map(|p| read_page(doc, p)).collect()
}

impl Transition {
    /// The transition as a file keeps it: fields the style does not use are none, a duration of
    /// 1 s (the PDF default) stays 1.
    pub fn effective(self) -> Self {
        Self {
            dimension: if self.style.has_dimension() { self.dimension } else { None },
            motion: if self.style.has_motion() { self.motion } else { None },
            direction: if self.style.has_direction() { self.direction } else { None },
            ..self
        }
    }
}

/// Sets (or, for none, removes) /Trans and /Dur on `pages`.
pub fn write(doc: &mut PdfDocument, pages: &[u32], change: &TransChange) -> Result<Vec<u32>> {
    change.check()?;
    let mut touched = Vec::new();
    for &p in pages {
        let mut page = doc.find_page(p as i32)?;
        match &change.transition {
            Some(t) => {
                let mut d = doc.new_dict()?;
                put_name(&mut d, "Type", "Trans")?;
                put_name(&mut d, "S", t.style.name())?;
                if (t.duration - 1.0).abs() > f32::EPSILON {
                    d.dict_put("D", PdfObject::new_real(t.duration)?)?;
                }
                if let (true, Some(dm)) = (t.style.has_dimension(), t.dimension) {
                    put_name(&mut d, "Dm", if dm == Dimension::H { "H" } else { "V" })?;
                }
                if let (true, Some(m)) = (t.style.has_motion(), t.motion) {
                    put_name(&mut d, "M", if m == Motion::I { "I" } else { "O" })?;
                }
                if let (true, Some(di)) = (t.style.has_direction(), t.direction) {
                    put_int(&mut d, "Di", di as i32)?;
                }
                page.dict_put("Trans", d)?;
            }
            None => page.dict_delete("Trans")?,
        }
        match change.auto_advance {
            Some(a) => page.dict_put("Dur", PdfObject::new_real(a)?)?,
            None => page.dict_delete("Dur")?,
        }
        touched.push(p);
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_names_round_trip() {
        for (n, s) in TransStyle::ALL {
            assert_eq!(s.name(), n);
            assert_eq!(TransStyle::from_name(n), Some(s));
        }
        assert_eq!(TransStyle::from_name("Spin"), None);
    }

    #[test]
    fn bad_values_are_refused() {
        let t = Transition { style: TransStyle::Wipe, duration: 1.0, dimension: None, motion: None, direction: Some(45) };
        assert!(TransChange { transition: Some(t), auto_advance: None }.check().is_err());
        let t = Transition { direction: Some(90), ..t };
        assert!(TransChange { transition: Some(t), auto_advance: Some(-1.0) }.check().is_err());
        assert!(TransChange { transition: Some(t), auto_advance: Some(5.0) }.check().is_ok());
    }
}
