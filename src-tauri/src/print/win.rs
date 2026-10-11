//! Windows printing through GDI: the printers and what each can do (paper sizes, trays, two-sided,
//! colour), and a print job that takes bitmaps of finished sheets.
//!
//! A job is a printer device context made from a DEVMODE the driver validated. Each sheet is one
//! page: its bands are sent with StretchDIBits at the printer's own resolution (capped at
//! `MAX_DPI`; the driver scales beyond that). Changing a sheet from portrait to landscape between
//! pages is a ResetDC with a new orientation, the way Acrobat's Auto orientation does it.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use serde::Serialize;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::Printing::*;
use windows_sys::Win32::Storage::Xps::*;

/// The highest resolution sheets are rendered at, in dots per inch. A 1200 dpi laser gains nothing
/// visible from 1200 dpi page bitmaps and a letter page would take 100 MB per band set.
pub const MAX_DPI: i32 = 600;

#[derive(Debug, thiserror::Error)]
pub enum WinError {
    #[error("{0}")]
    Printer(String),
    #[error("cancelled")]
    Cancelled,
}

type Result<T> = std::result::Result<T, WinError>;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

fn from_wide_buf(b: &[u16]) -> String {
    let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

// ---------------------------------------------------------------- printers

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Printer {
    pub name: String,
    pub is_default: bool,
    /// Prints to a file: the spooler would ask where. (Microsoft Print to PDF, the XPS writer.)
    pub to_file: bool,
}

pub fn default_printer() -> Option<String> {
    let mut n = 0u32;
    unsafe {
        GetDefaultPrinterW(null_mut(), &mut n);
        if n == 0 {
            return None;
        }
        let mut b = vec![0u16; n as usize];
        (GetDefaultPrinterW(b.as_mut_ptr(), &mut n) != 0).then(|| from_wide_buf(&b))
    }
}

pub fn printers() -> Vec<Printer> {
    let default = default_printer();
    let mut need = 0u32;
    let mut count = 0u32;
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    unsafe {
        EnumPrintersW(flags, null(), 5, null_mut(), 0, &mut need, &mut count);
        if need == 0 {
            return Vec::new();
        }
        // 8-byte aligned storage for the PRINTER_INFO_5W array. A printer may be added between the
        // two calls, so the buffer is asked for again (a few times) when it was too small.
        let mut buf = Vec::new();
        let mut listed = false;
        for _ in 0..3 {
            buf = vec![0u64; (need as usize).div_ceil(8)];
            let size = need;
            if EnumPrintersW(flags, null(), 5, buf.as_mut_ptr().cast(), size, &mut need, &mut count) != 0 {
                listed = true;
                break;
            }
            if need <= size {
                break;
            }
        }
        if !listed {
            return Vec::new();
        }
        let infos = std::slice::from_raw_parts(buf.as_ptr().cast::<PRINTER_INFO_5W>(), count as usize);
        let mut out: Vec<Printer> = infos
            .iter()
            .map(|i| {
                let name = from_wide(i.pPrinterName);
                let port = from_wide(i.pPortName).to_ascii_uppercase();
                Printer { is_default: default.as_deref() == Some(name.as_str()), to_file: port.starts_with("PORTPROMPT") || port == "FILE:", name }
            })
            .collect();
        out.sort_by(|a, b| b.is_default.cmp(&a.is_default).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        out
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paper {
    /// The driver's DMPAPER_* number.
    pub id: u16,
    pub name: String,
    /// Points, portrait.
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tray {
    pub id: u16,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub papers: Vec<Paper>,
    pub trays: Vec<Tray>,
    pub duplex: bool,
    pub color: bool,
    /// Does the driver collate by itself? (Inkwell collates itself either way.)
    pub collate: bool,
    pub max_copies: u32,
}

/// What `printer` can do, or None when the driver does not answer.
pub fn capabilities(printer: &str) -> Option<Capabilities> {
    let name = wide(printer);
    let cap = |what: PRINTER_DEVICE_CAPABILITIES, out: *mut u16| unsafe { DeviceCapabilitiesW(name.as_ptr(), null(), what, out, null()) };
    // Fixed-width record arrays: ids as WORD, names as 64 (paper) or 24 (tray) wide characters.
    let n_papers = cap(DC_PAPERS, null_mut());
    if n_papers < 0 {
        return None;
    }
    let n = n_papers as usize;
    let mut ids = vec![0u16; n];
    let mut names = vec![0u16; n * 64];
    // POINT array in tenths of a millimetre.
    let mut sizes = vec![0i32; n * 2];
    if n > 0 {
        cap(DC_PAPERS, ids.as_mut_ptr());
        cap(DC_PAPERNAMES, names.as_mut_ptr());
        cap(DC_PAPERSIZE, sizes.as_mut_ptr().cast());
    }
    let papers = (0..n)
        .filter(|i| sizes[i * 2] > 0 && sizes[i * 2 + 1] > 0)
        .map(|i| Paper {
            id: ids[i],
            name: from_wide_buf(&names[i * 64..i * 64 + 64]),
            // tenths of a millimetre to points
            width: sizes[i * 2] as f64 / 10.0 / 25.4 * 72.0,
            height: sizes[i * 2 + 1] as f64 / 10.0 / 25.4 * 72.0,
        })
        .collect();
    let n_bins = cap(DC_BINS, null_mut()).max(0) as usize;
    let mut bin_ids = vec![0u16; n_bins];
    let mut bin_names = vec![0u16; n_bins * 24];
    if n_bins > 0 {
        cap(DC_BINS, bin_ids.as_mut_ptr());
        cap(DC_BINNAMES, bin_names.as_mut_ptr());
    }
    let trays = (0..n_bins).map(|i| Tray { id: bin_ids[i], name: from_wide_buf(&bin_names[i * 24..i * 24 + 24]) }).collect();
    Some(Capabilities {
        papers,
        trays,
        duplex: cap(DC_DUPLEX, null_mut()) == 1,
        color: cap(DC_COLORDEVICE, null_mut()) == 1,
        collate: cap(DC_COLLATE, null_mut()) == 1,
        max_copies: cap(DC_COPIES, null_mut()).max(1) as u32,
    })
}

// ---------------------------------------------------------------- jobs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    None,
    /// Flip on the paper's long edge (book style when portrait).
    Long,
    /// Flip on the paper's short edge (calendar style when portrait).
    Short,
}

#[derive(Debug, Clone)]
pub struct JobOptions {
    pub printer: String,
    pub title: String,
    /// Write the output here instead of sending it to the printer's port (print-to-file drivers).
    pub output: Option<std::path::PathBuf>,
    pub paper: Option<u16>,
    pub tray: Option<u16>,
    pub duplex: Edge,
    pub gray: bool,
}

/// Page geometry of the device context after the last ResetDC.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub dpi_x: i32,
    pub dpi_y: i32,
    /// The whole paper in device pixels.
    pub width: i32,
    pub height: i32,
    /// Where the printable area starts, in device pixels.
    pub off_x: i32,
    pub off_y: i32,
}

pub struct Job {
    dc: HDC,
    /// 8-byte aligned DEVMODEW storage, driver data included.
    devmode: Vec<u64>,
    opts: JobOptions,
    landscape: Option<bool>,
    in_doc: bool,
    in_page: bool,
    pub geometry: Geometry,
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            if self.in_doc {
                AbortDoc(self.dc);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
        }
    }
}

/// DEVMODE `dmDuplex` for a binding edge. The documented values are absolute: DMDUP_VERTICAL is
/// long-edge binding and DMDUP_HORIZONTAL short-edge, whatever the orientation (a PrintTicket
/// TwoSidedLongEdge maps to VERTICAL). The orientation is not mixed in.
fn duplex_value(e: Edge) -> i16 {
    match e {
        Edge::None => DMDUP_SIMPLEX as i16,
        Edge::Long => DMDUP_VERTICAL as i16,
        Edge::Short => DMDUP_HORIZONTAL as i16,
    }
}

fn devmode_ptr(buf: &mut [u64]) -> *mut DEVMODEW {
    buf.as_mut_ptr().cast()
}

/// The driver's DEVMODE for `printer`, with our fields set and validated by the driver.
fn make_devmode(o: &JobOptions, landscape: bool) -> Result<Vec<u64>> {
    let name = wide(&o.printer);
    unsafe {
        let mut h = PRINTER_HANDLE { Value: null_mut() };
        if OpenPrinterW(name.as_ptr(), &mut h, null()) == 0 {
            return Err(WinError::Printer(format!("cannot open the printer {}", o.printer)));
        }
        let size = DocumentPropertiesW(null_mut(), h, name.as_ptr(), null_mut(), null(), 0);
        if size <= 0 {
            ClosePrinter(h);
            return Err(WinError::Printer(format!("the driver of {} does not answer", o.printer)));
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let ok = DocumentPropertiesW(null_mut(), h, name.as_ptr(), devmode_ptr(&mut buf), null(), DM_OUT_BUFFER) > 0;
        if !ok {
            ClosePrinter(h);
            return Err(WinError::Printer(format!("the driver of {} gave no settings", o.printer)));
        }
        let dm = &mut *devmode_ptr(&mut buf);
        let mut fields = dm.dmFields;
        dm.Anonymous1.Anonymous1.dmOrientation = if landscape { DMORIENT_LANDSCAPE as i16 } else { DMORIENT_PORTRAIT as i16 };
        fields |= DM_ORIENTATION;
        if let Some(p) = o.paper {
            dm.Anonymous1.Anonymous1.dmPaperSize = p as i16;
            fields |= DM_PAPERSIZE;
        }
        if let Some(t) = o.tray {
            dm.Anonymous1.Anonymous1.dmDefaultSource = t as i16;
            fields |= DM_DEFAULTSOURCE;
        }
        // Inkwell repeats sheets itself for copies and collation: exactly one copy per page we send.
        dm.Anonymous1.Anonymous1.dmCopies = 1;
        fields |= DM_COPIES;
        dm.dmColor = if o.gray { DMCOLOR_MONOCHROME } else { DMCOLOR_COLOR };
        fields |= DM_COLOR;
        dm.dmDuplex = duplex_value(o.duplex);
        fields |= DM_DUPLEX;
        dm.dmFields = fields;
        // Let the driver merge and validate what was set (private driver data stays consistent).
        let mut merged = vec![0u64; buf.len()];
        let r = DocumentPropertiesW(
            null_mut(),
            h,
            name.as_ptr(),
            devmode_ptr(&mut merged),
            devmode_ptr(&mut buf),
            DM_IN_BUFFER | DM_OUT_BUFFER,
        );
        ClosePrinter(h);
        Ok(if r > 0 { merged } else { buf })
    }
}

impl Job {
    /// Opens the printer's device context and starts the document.
    pub fn start(opts: JobOptions) -> Result<Job> {
        let mut devmode = make_devmode(&opts, false)?;
        let driver = wide("WINSPOOL");
        let name = wide(&opts.printer);
        let dc = unsafe { CreateDCW(driver.as_ptr(), name.as_ptr(), null(), devmode_ptr(&mut devmode)) };
        if dc.is_null() {
            return Err(WinError::Printer(format!("cannot print to {}", opts.printer)));
        }
        let mut job = Job { dc, devmode, opts, landscape: Some(false), in_doc: false, in_page: false, geometry: Geometry { dpi_x: 300, dpi_y: 300, width: 0, height: 0, off_x: 0, off_y: 0 } };
        let title = wide(&job.opts.title);
        let out = job.opts.output.as_ref().map(|p| wide(&p.to_string_lossy()));
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: title.as_ptr(),
            lpszOutput: out.as_ref().map_or(null(), |o| o.as_ptr()),
            lpszDatatype: null(),
            fwType: 0,
        };
        if unsafe { StartDocW(job.dc, &info) } <= 0 {
            return Err(WinError::Printer(format!("{} did not accept the job", job.opts.printer)));
        }
        job.in_doc = true;
        job.read_geometry();
        Ok(job)
    }

    fn read_geometry(&mut self) {
        let g = |i: GET_DEVICE_CAPS_INDEX| unsafe { GetDeviceCaps(self.dc, i as i32) };
        self.geometry = Geometry {
            dpi_x: g(LOGPIXELSX).max(1),
            dpi_y: g(LOGPIXELSY).max(1),
            width: g(PHYSICALWIDTH),
            height: g(PHYSICALHEIGHT),
            off_x: g(PHYSICALOFFSETX),
            off_y: g(PHYSICALOFFSETY),
        };
    }

    /// Starts the next page; turns the paper when `landscape` differs from the last one.
    pub fn begin_sheet(&mut self, landscape: bool) -> Result<()> {
        if self.landscape != Some(landscape) {
            self.devmode = make_devmode(&self.opts, landscape)?;
            let r = unsafe { ResetDCW(self.dc, devmode_ptr(&mut self.devmode)) };
            if r.is_null() {
                return Err(WinError::Printer(format!("{} cannot switch paper orientation", self.opts.printer)));
            }
            self.landscape = Some(landscape);
            self.read_geometry();
        }
        if unsafe { StartPage(self.dc) } <= 0 {
            return Err(WinError::Printer("the printer stopped the job".into()));
        }
        self.in_page = true;
        Ok(())
    }

    /// Sends a band: `rows` rows of `width` pixels, top-down, 24-bit BGR with 4-byte aligned rows.
    /// `dest` is the rectangle on the paper in device pixels (x, y, w, h), origin at the paper's corner.
    pub fn draw_band(&mut self, bgr: &[u8], width: i32, rows: i32, dest: (i32, i32, i32, i32)) -> Result<()> {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -rows,
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB,
            ..Default::default()
        };
        let scaled = dest.2 != width || dest.3 != rows;
        unsafe {
            SetStretchBltMode(self.dc, if scaled { HALFTONE } else { COLORONCOLOR });
            let r = StretchDIBits(
                self.dc,
                dest.0 - self.geometry.off_x,
                dest.1 - self.geometry.off_y,
                dest.2,
                dest.3,
                0,
                0,
                width,
                rows,
                bgr.as_ptr().cast::<c_void>(),
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            if r == 0 || r == GDI_ERROR as i32 {
                return Err(WinError::Printer("the printer rejected a page image".into()));
            }
        }
        Ok(())
    }

    pub fn end_sheet(&mut self) -> Result<()> {
        self.in_page = false;
        if unsafe { EndPage(self.dc) } <= 0 {
            return Err(WinError::Printer("the printer stopped the job".into()));
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        if self.in_page {
            unsafe { EndPage(self.dc) };
            self.in_page = false;
        }
        // `in_doc` stays set until EndDoc succeeds, so a failure still reaches AbortDoc in drop.
        if unsafe { EndDoc(self.dc) } <= 0 {
            return Err(WinError::Printer("the printer did not finish the job".into()));
        }
        self.in_doc = false;
        Ok(())
    }

    /// The paper the device context has now, in points (portrait after `start`).
    pub fn paper_points(&self) -> (f64, f64) {
        let g = self.geometry;
        (g.width as f64 * 72.0 / g.dpi_x.max(1) as f64, g.height as f64 * 72.0 / g.dpi_y.max(1) as f64)
    }

    /// Throws the job away (the spooler removes what it already has).
    pub fn abort(mut self) {
        if self.in_doc {
            unsafe { AbortDoc(self.dc) };
            self.in_doc = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplex_flags_are_absolute() {
        assert_eq!(duplex_value(Edge::None), DMDUP_SIMPLEX as i16);
        assert_eq!(duplex_value(Edge::Long), DMDUP_VERTICAL as i16);
        assert_eq!(duplex_value(Edge::Short), DMDUP_HORIZONTAL as i16);
    }
}
