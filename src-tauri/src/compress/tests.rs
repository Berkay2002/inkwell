//! Tests run on scratch copies of the study samples (never in place) and on generated fixtures.
//! `cargo test --lib compress -- --test-threads=1` (LIBCLANG_PATH=E:\Tools\LLVM\bin).

use super::*;
use crate::engine::fixture;
use mupdf::pdf::document::Encryption;
use mupdf::pdf::{InsertImageOptions, PageImageSource};
use mupdf::text_page::TextPageFlags;
use mupdf::{Colorspace, Pixmap, Rect, Size};
use std::sync::atomic::AtomicUsize;

static N: AtomicUsize = AtomicUsize::new(0);

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/study/samples")
}

/// A scratch directory of its own, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("inkwell-compress-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }

    fn copy(&self, sample: &str) -> PathBuf {
        let to = self.0.join(sample);
        fs::copy(samples().join(sample), &to).unwrap();
        to
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(input: &Path, opts: &Options) -> Result<Report, Error> {
    compress(input, opts, &AtomicBool::new(false), &mut |_| {})
}

fn open(p: &Path) -> Document {
    Document::open(p.to_str().unwrap()).unwrap()
}

fn diff(a: &Path, b: &Path, pages: usize) -> VisualCheck {
    visual::compare(&open(a), &open(b), pages, &AtomicBool::new(false), |_, _| {}).unwrap()
}

fn text(p: &Path) -> Vec<String> {
    let d = open(p);
    (0..d.page_count().unwrap())
        .map(|i| d.load_page(i).unwrap().to_text_page(TextPageFlags::empty()).unwrap().to_text().unwrap())
        .collect()
}

// ---------------------------------------------------------------- fixtures

fn lcg(s: &mut u32) -> u32 {
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    *s >> 24
}

/// Smooth, photo-like content with a little noise, so JPEG/JPX behave like they do on real photos.
fn photo(w: i32, h: i32, rgb: bool, seed: u32) -> Pixmap {
    let cs = if rgb { Colorspace::device_rgb() } else { Colorspace::device_gray() };
    let mut p = Pixmap::new_with_w_h(&cs, w, h, false).unwrap();
    let n = if rgb { 3 } else { 1 };
    let mut s = seed;
    let data = p.samples_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..n {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                let v = 128.0 + 70.0 * ((fx * 9.0 + c as f32).sin() * (fy * 7.0 + seed as f32).cos()) + 30.0 * (fx * 3.0 + fy * 5.0).sin();
                data[(y * w as usize + x) * n + c] = (v + (lcg(&mut s) as f32 - 128.0) * 0.08).clamp(0.0, 255.0) as u8;
            }
        }
    }
    p
}

/// Black and white "text lines": 8-bit gray holding only 0 and 255, which MuPDF classifies as bitonal.
fn lines(w: i32, h: i32) -> Pixmap {
    let mut p = Pixmap::new_with_w_h(&Colorspace::device_gray(), w, h, false).unwrap();
    let data = p.samples_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let on = (y % 24 < 14) && ((x / 6 + y / 24 * 7) % 5 != 0);
            data[y * w as usize + x] = if on { 0 } else { 255 };
        }
    }
    p
}

/// `pages` pages, each with a colour photo (233 ppi), a gray one (300 ppi), a monochrome "scan" (300 ppi)
/// and one logo that is the same object on every page.
fn image_heavy(path: &Path, pages: usize) {
    let mut doc = PdfDocument::new();
    let logo = photo(100, 100, true, 99);
    let mut logo_xref = None;
    for i in 0..pages {
        let mut page = doc.new_page(Size::A4).unwrap();
        let o = InsertImageOptions::default();
        page.insert_image(&mut doc, Rect::new(36.0, 36.0, 252.0, 190.0), PageImageSource::Pixmap(&photo(700, 494, true, i as u32 + 1)), o).unwrap();
        page.insert_image(&mut doc, Rect::new(300.0, 36.0, 444.0, 140.0), PageImageSource::Pixmap(&photo(600, 433, false, i as u32 + 50)), o).unwrap();
        page.insert_image(&mut doc, Rect::new(36.0, 300.0, 324.0, 500.0), PageImageSource::Pixmap(&lines(1200, 833)), o).unwrap();
        let src = match logo_xref {
            None => PageImageSource::Pixmap(&logo),
            Some(x) => PageImageSource::ExistingXref(x),
        };
        let placed = page.insert_image(&mut doc, Rect::new(500.0, 700.0, 560.0, 760.0), src, o).unwrap();
        logo_xref = Some(placed.xref);
    }
    doc.save(path.to_str().unwrap()).unwrap();
}

fn text_only(path: &Path) {
    let bytes = fixture::build(&fixture::Fixture { pages: 6, chapter_every: 2, needle_every: 2, lines: 40 });
    fs::write(path, bytes).unwrap();
}

// ---------------------------------------------------------------- tests

/// The comparison the brief asks for. Acrobat's outputs come from the study; both sides are measured with
/// the same render-diff against `opt-in-01.pdf`, so the numbers are comparable.
/// Prints the table (`--nocapture`) and checks each Inkwell preset against its Acrobat counterpart.
#[test]
fn presets_beat_acrobat_at_similar_visual_difference() {
    let s = Scratch::new();
    let input = s.copy("opt-in-01.pdf");
    // (Inkwell preset, the Acrobat run it is held against)
    let pairs = [
        (Preset::Smallest, "opt-out-04-72ppi-minq.pdf", "04: 72 ppi above 108, JPEG minimum"),
        (Preset::Balanced, "opt-out-01-compress-single.pdf", "01: Single file (Standard + JPEG2000 medium)"),
        (Preset::HighQuality, "opt-out-06-jpx-max-150.pdf", "06: JPEG2000 maximum, 150 ppi"),
    ];
    println!("| Acrobat run | bytes | max | mean | PSNR | Inkwell preset | bytes | max | mean | PSNR | saved vs Acrobat |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for (preset, acrobat, label) in pairs {
        let a_path = samples().join(acrobat);
        let a_size = fs::metadata(&a_path).unwrap().len();
        let a = diff(&input, &a_path, 8);

        let mut o = Options::preset(preset);
        o.output = Some(s.path(&format!("{preset:?}.pdf")));
        let r = run(&input, &o).unwrap();
        let v = r.visual.as_ref().unwrap();
        println!(
            "| {label} | {a_size} | {} | {:.3} | {:.1} | {preset:?} | {} | {} | {:.3} | {:.1} | {:.0}% |",
            a.max_diff, a.mean_diff, a.psnr, r.size_after, v.max_diff, v.mean_diff, v.psnr,
            (1.0 - r.size_after as f64 / a_size as f64) * 100.0
        );
        assert!(!r.kept_original);
        assert!(r.size_after <= a_size, "{preset:?}: {} B is bigger than Acrobat's {a_size} B", r.size_after);
        assert!(v.psnr >= a.psnr - 0.5, "{preset:?}: PSNR {:.1} is below Acrobat's {:.1}", v.psnr, a.psnr);
    }
}

#[test]
fn every_sample_survives_every_preset() {
    let s = Scratch::new();
    let mut names: Vec<String> = fs::read_dir(samples())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".pdf") && !n.starts_with("opt-out") && !n.contains("_Part"))
        .collect();
    names.sort();
    for name in names {
        let input = s.copy(&name);
        let before = text(&input);
        for p in [Preset::Smallest, Preset::Balanced, Preset::HighQuality] {
            let mut o = Options::preset(p);
            o.output = Some(s.path(&format!("{name}.{p:?}.pdf")));
            let r = run(&input, &o).unwrap_or_else(|e| panic!("{name} {p:?}: {e}"));
            assert!(r.size_after <= r.size_before, "{name} {p:?} grew");
            assert_eq!(text(&r.output), before, "{name} {p:?}: text changed");
            let v = r.visual.as_ref().unwrap();
            println!("{name:40} {p:?}: {} -> {} B, max {} mean {:.3} psnr {:.1}", r.size_before, r.size_after, v.max_diff, v.mean_diff, v.psnr);
            if r.before.images == 0 {
                assert_eq!(v.max_diff, 0, "{name} {p:?}: pixels changed without any image");
            }
            // A garbled image (wrong colourspace, stride, inverted data) lands near 29 dB; legitimate
            // Smallest-preset loss on the samples stays well above this.
            assert!(v.psnr >= 30.0 && v.mean_diff <= 4.0, "{name} {p:?}: {v:?}");
        }
    }
}

#[test]
fn text_only_pdf_does_not_grow() {
    let s = Scratch::new();
    let input = s.path("text.pdf");
    text_only(&input);
    for p in [Preset::Smallest, Preset::Balanced, Preset::HighQuality] {
        let mut o = Options::preset(p);
        o.output = Some(s.path(&format!("text.{p:?}.pdf")));
        let r = run(&input, &o).unwrap();
        assert!(r.size_after <= r.size_before);
        assert_eq!(r.images.seen, 0);
        assert_eq!(text(&r.output), text(&input));
        assert_eq!(r.visual.unwrap().max_diff, 0);
    }
}

#[test]
fn never_worse_keeps_the_original_bytes() {
    let s = Scratch::new();
    let input = s.copy("opt-in-01.pdf");
    let mut o = Options::preset(Preset::Smallest);
    o.output = Some(s.path("small.pdf"));
    let small = run(&input, &o).unwrap();
    assert!(!small.kept_original);

    // Writing the small file back out without object streams or garbage collection can only make it bigger.
    let mut o = Options::preset(Preset::Custom);
    o.images.color.codec = Codec::Retain;
    o.images.color.above_ppi = 0;
    o.images.gray = o.images.color;
    o.images.mono = o.images.color;
    o.object_streams = false;
    o.garbage = 0;
    o.output = Some(s.path("again.pdf"));
    let r = run(&small.output, &o).unwrap();
    assert!(r.kept_original, "{} -> {}", r.size_before, r.size_after);
    assert_eq!(r.saved_bytes, 0);
    assert_eq!(fs::read(&r.output).unwrap(), fs::read(&small.output).unwrap());
    assert!(r.visual.is_none());
}

#[test]
fn refuses_to_overwrite() {
    let s = Scratch::new();
    let input = s.copy("redact-source.pdf");

    // the default name
    let existing = default_output(&input);
    fs::write(&existing, b"precious").unwrap();
    match run(&input, &Options::default()) {
        Err(Error::OutputExists(p)) => assert_eq!(p, existing),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&existing).unwrap(), b"precious");

    // an explicit name, and the input itself
    let mut o = Options::default();
    o.output = Some(existing.clone());
    assert!(matches!(run(&input, &o), Err(Error::OutputExists(_))));
    o.output = Some(input.clone());
    assert!(matches!(run(&input, &o), Err(Error::SameFile)));
    assert_eq!(fs::read(&input).unwrap(), fs::read(samples().join("redact-source.pdf")).unwrap());

    // nothing but the two files, no temp leftovers
    assert_eq!(s.files(), vec!["redact-source (compressed).pdf".to_string(), "redact-source.pdf".to_string()]);
}

#[test]
fn default_output_sits_next_to_the_input() {
    let s = Scratch::new();
    let input = s.copy("opt-in-01.pdf");
    let r = run(&input, &Options::default()).unwrap();
    assert_eq!(r.output, s.path("opt-in-01 (compressed).pdf"));
    assert!(r.output.exists());
    assert_eq!(s.files().len(), 2);
}

#[test]
fn cancel_stops_the_job_and_leaves_nothing_behind() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    image_heavy(&input, 10);
    let before = fs::read(&input).unwrap();

    let cancel = AtomicBool::new(false);
    let mut seen_images = 0;
    let r = compress(&input, &Options::default(), &cancel, &mut |p| {
        if p.stage == Stage::Images {
            seen_images += 1;
            if p.page >= 3 {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    });
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    assert!(seen_images >= 3 && seen_images < 10, "stopped after {seen_images} page callbacks");
    assert_eq!(s.files(), vec!["heavy.pdf".to_string()]);
    assert_eq!(fs::read(&input).unwrap(), before);

    // cancelled before it starts
    let r = compress(&input, &Options::default(), &AtomicBool::new(true), &mut |_| {});
    assert!(matches!(r, Err(Error::Cancelled)));
    assert_eq!(s.files(), vec!["heavy.pdf".to_string()]);
}

#[test]
fn spawned_job_reports_progress_and_joins() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    image_heavy(&input, 4);
    let seen = Arc::new(parking_lot::Mutex::new(Vec::<f32>::new()));
    let sink = seen.clone();
    let job = spawn(input, Options::default(), move |p| sink.lock().push(p.fraction));
    let r = job.join().unwrap();
    assert!(r.saved_bytes > 0);
    let f = seen.lock();
    assert!(f.windows(2).all(|w| w[0] <= w[1]), "progress went backwards: {f:?}");
    assert!(f.len() > 8 && *f.last().unwrap() == 1.0);
}

#[test]
fn image_classes_dedupe_and_quality() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    let pages = 6;
    image_heavy(&input, pages);
    let mut o = Options::preset(Preset::Balanced);
    o.output = Some(s.path("heavy.out.pdf"));
    let r = run(&input, &o).unwrap();

    assert_eq!(r.before.images as usize, pages * 3 + 1);
    // the logo is one object again, not one per page
    assert!(r.after.images as usize <= pages * 3 + 1, "{} images", r.after.images);
    // photos: 233 ppi is above 225 so it is resampled; the 300 ppi gray one too
    assert!(r.images.resampled as usize >= pages * 2, "{:?}", r.images);
    assert!(r.after.bytes(Category::Images) * 3 < r.before.bytes(Category::Images), "{} -> {}", r.before.bytes(Category::Images), r.after.bytes(Category::Images));
    assert_eq!(r.images.failed, 0);

    // the scan became CCITT G4 and is still pixel-identical
    let doc = open_pdf(&r.output, None).unwrap();
    let fax = (1..doc.xref_len().unwrap())
        .filter(|&x| doc.xref_object(x as i32).ok().flatten().is_some_and(|o| o.to_string().contains("CCITTFaxDecode")))
        .count();
    // identical scans on every page are merged into one object by the garbage collector
    assert_eq!(fax, 1, "{fax} fax images");
    assert!(r.after.images as usize <= pages * 2 + 2, "{} images", r.after.images);
    let v = r.visual.unwrap();
    assert!(v.psnr > 36.0, "{v:?}");
}

#[test]
fn quality_setting_is_monotonic() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    image_heavy(&input, 2);
    let mut last = (u64::MAX, 0.0f64);
    for (i, q) in [30u8, 60, 90].into_iter().enumerate().rev() {
        let mut o = Options::preset(Preset::Balanced);
        o.images.color.quality = q;
        o.images.gray.quality = q;
        o.output = Some(s.path(&format!("q{i}.pdf")));
        let r = run(&input, &o).unwrap();
        let psnr = r.visual.unwrap().psnr;
        // going down the list: lower quality -> smaller file, lower PSNR
        if last.0 != u64::MAX {
            assert!(r.size_after < last.0 && psnr < last.1, "q{q}: {} B {psnr:.1} dB after {} B {:.1} dB", r.size_after, last.0, last.1);
        }
        last = (r.size_after, psnr);
    }
}

#[test]
fn strip_options_remove_only_what_was_asked() {
    let s = Scratch::new();
    let input = s.path("extras.pdf");
    {
        let mut doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 3, chapter_every: 1, needle_every: 0, lines: 6 })).unwrap();
        doc.add_embedded_file("note.txt", b"attached", mupdf::pdf::EmbeddedFileOptions::new("note.txt")).unwrap();
        let js = doc.new_object_from_str("<< /S /JavaScript /JS (app.alert\\(1\\)) >>").unwrap();
        doc.catalog().unwrap().dict_put("OpenAction", js).unwrap();
        let info = doc.new_object_from_str("<< /Title (Extras) /Author (Me) >>").unwrap();
        let info = doc.add_object(&info).unwrap();
        doc.trailer().unwrap().dict_put("Info", info).unwrap();
        doc.save(input.to_str().unwrap()).unwrap();
    }
    let has = |p: &Path, what: &str| -> bool {
        let d = open_pdf(p, None).unwrap();
        match what {
            "info" => d.trailer().unwrap().get_dict("Info").unwrap().is_some(),
            "outlines" => d.catalog().unwrap().get_dict("Outlines").unwrap().is_some(),
            "files" => !d.embedded_files().unwrap().is_empty(),
            "js" => d.catalog().unwrap().get_dict("OpenAction").unwrap().is_some(),
            _ => unreachable!(),
        }
    };
    for w in ["info", "outlines", "files", "js"] {
        assert!(has(&input, w), "fixture lacks {w}");
    }

    // defaults keep everything
    let mut o = Options::preset(Preset::Balanced);
    o.output = Some(s.path("kept.pdf"));
    let r = run(&input, &o).unwrap();
    for w in ["info", "outlines", "files", "js"] {
        assert!(has(&r.output, w), "default run dropped {w}");
    }

    // one at a time
    for (name, strip) in [
        ("info", Strip { metadata: true, ..Strip::default() }),
        ("outlines", Strip { bookmarks: true, ..Strip::default() }),
        ("files", Strip { embedded_files: true, ..Strip::default() }),
        ("js", Strip { javascript: true, ..Strip::default() }),
    ] {
        let mut o = Options::preset(Preset::Balanced);
        o.strip = strip;
        o.output = Some(s.path(&format!("no-{name}.pdf")));
        let r = run(&input, &o).unwrap();
        for w in ["info", "outlines", "files", "js"] {
            assert_eq!(has(&r.output, w), w != name, "strip {name}: {w}");
        }
    }
}

#[test]
fn thumbnails_are_dropped_on_request() {
    let s = Scratch::new();
    let input = s.copy("scan-source.pdf");
    let thumbs = |p: &Path| {
        let d = open_pdf(p, None).unwrap();
        d.find_page(0).unwrap().get_dict("Thumb").unwrap().is_some()
    };
    assert!(thumbs(&input));
    let mut o = Options::preset(Preset::Balanced);
    o.strip.thumbnails = true;
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.thumbnails, 1);
    assert!(!thumbs(&r.output));
}

#[test]
fn encrypted_input_needs_the_password_and_stays_protected() {
    let s = Scratch::new();
    let input = s.path("locked.pdf");
    {
        let doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture::default())).unwrap();
        let mut w = PdfWriteOptions::default();
        w.set_encryption(Encryption::Aes256).set_owner_password("owner").set_user_password("user");
        doc.save_with_options(input.to_str().unwrap(), w).unwrap();
    }
    assert!(matches!(run(&input, &Options::default()), Err(Error::NeedsPassword)));
    let mut o = Options::default();
    o.password = Some("nope".into());
    assert!(matches!(run(&input, &o), Err(Error::BadPassword)));
    o.password = Some("user".into());
    let r = run(&input, &o).unwrap();
    let mut out = open(&r.output);
    assert!(out.needs_password().unwrap(), "the compressed copy lost its encryption");
    assert!(out.authenticate("user").unwrap());
    assert_eq!(s.files().len(), 2);
}

#[test]
fn audit_adds_up() {
    let s = Scratch::new();
    let input = s.copy("opt-in-01.pdf");
    let a = audit_file(&input, None).unwrap();
    let sum: u64 = a.categories.iter().map(|c| c.bytes).sum();
    assert_eq!(sum, a.total);
    assert_eq!(a.total, 2_657_232);
    // Acrobat's audit of the same file: images 2,641,338 B (99.40 %), fonts 14,040 B
    assert!((a.bytes(Category::Images) as i64 - 2_641_338).abs() < 1_000, "{}", a.bytes(Category::Images));
    assert!(a.bytes(Category::Fonts) > 10_000 && a.bytes(Category::Fonts) < 20_000);
    assert_eq!(a.images, 2);
    assert_eq!(a.largest[0].bytes, 1_750_529);
}

#[test]
fn report_shows_where_the_bytes_went() {
    let s = Scratch::new();
    let input = s.copy("opt-in-01.pdf");
    let r = run(&input, &Options::default()).unwrap();
    assert!(r.after.bytes(Category::Images) < r.before.bytes(Category::Images) / 5);
    assert_eq!(r.after.total, r.size_after);
    assert_eq!(r.images.seen, 2);
    assert!(r.timings.iter().any(|t| t.stage == "images"));
    let v = r.visual.unwrap();
    assert_eq!((v.pages_checked, v.pages_total), (2, 2));
    // serialises for the UI
    let json = serde_json::to_string(&r.after).unwrap();
    assert!(json.contains("\"images\""));
}

// ---------------------------------------------------------------- review follow-ups

fn text_doc_with(extra: impl FnOnce(&mut PdfDocument)) -> PdfDocument {
    let mut doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 6, chapter_every: 1, needle_every: 0, lines: 6 })).unwrap();
    extra(&mut doc);
    doc
}

fn has_entry(p: &Path, key: &str) -> bool {
    open_pdf(p, None).unwrap().catalog().unwrap().get_dict(key).unwrap().is_some()
}

#[test]
fn stripping_is_not_undone_by_never_worse() {
    let s = Scratch::new();
    let packed = s.path("packed.pdf");
    let doc = text_doc_with(|d| {
        let js = d.new_object_from_str("<< /S /JavaScript /JS (app.alert\\(1\\)) >>").unwrap();
        d.catalog().unwrap().dict_put("OpenAction", js).unwrap();
    });
    // already as tight as the writer makes it: objects packed, garbage collected
    doc.save_with_options(packed.to_str().unwrap(), write_options(&Options::default())).unwrap();
    assert!(has_entry(&packed, "OpenAction"));

    // writing it back loosely is bigger than the input, so without a strip the original is kept
    let mut o = Options::preset(Preset::Custom);
    o.object_streams = false;
    o.garbage = 0;
    o.output = Some(s.path("plain.pdf"));
    let r = run(&packed, &o).unwrap();
    assert!(r.kept_original && has_entry(&r.output, "OpenAction"));

    // ... but asked to drop JavaScript, the stripped copy is written even though it is bigger
    o.strip.javascript = true;
    o.output = Some(s.path("stripped.pdf"));
    let r = run(&packed, &o).unwrap();
    assert!(!r.kept_original);
    assert!(r.size_after > r.size_before, "{} -> {}", r.size_before, r.size_after);
    assert!(!has_entry(&r.output, "OpenAction"));
    assert_eq!(r.stripped.javascript, 1);
    assert!(r.warnings.iter().any(|w| w.contains("larger than the original")), "{:?}", r.warnings);
    assert_eq!(r.saved_bytes, 0);
}

#[test]
fn signed_warning_only_when_the_copy_is_written() {
    let s = Scratch::new();
    let sign = |d: &mut PdfDocument| {
        let f = d.new_object_from_str("<< /Fields [] /SigFlags 3 >>").unwrap();
        d.catalog().unwrap().dict_put("AcroForm", f).unwrap();
    };
    // loose input: the compressed copy is written
    let loose = s.path("loose.pdf");
    text_doc_with(sign).save(loose.to_str().unwrap()).unwrap();
    let r = run(&loose, &Options::default()).unwrap();
    assert!(!r.kept_original);
    assert!(r.warnings.iter().any(|w| w.contains("signed")), "{:?}", r.warnings);

    // tight input: nothing to gain, the original stays and so do its signatures
    let tight = s.path("tight.pdf");
    text_doc_with(sign).save_with_options(tight.to_str().unwrap(), write_options(&Options::default())).unwrap();
    let r = run(&tight, &Options::default()).unwrap();
    assert!(r.kept_original);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn concurrent_jobs_on_one_input_never_corrupt_the_output() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    image_heavy(&input, 3);
    let jobs: Vec<Job> = (0..3).map(|_| spawn(input.clone(), Options::default(), |_| {})).collect();
    let results: Vec<_> = jobs.into_iter().map(Job::join).collect();
    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert!(ok >= 1, "{results:?}");
    for r in &results {
        assert!(matches!(r, Ok(_) | Err(Error::OutputExists(_))), "{r:?}");
    }
    let out = s.path("heavy (compressed).pdf");
    assert_eq!(open(&out).page_count().unwrap(), 3);
    assert!(diff(&input, &out, 3).psnr > 36.0);
    // no temp files left by the losers
    assert_eq!(s.files(), vec!["heavy (compressed).pdf".to_string(), "heavy.pdf".to_string()]);
}

#[test]
fn publish_never_replaces_and_has_a_copy_fallback() {
    let s = Scratch::new();
    let (tmp, out) = (s.path("t.tmp"), s.path("o.pdf"));
    fs::write(&tmp, b"payload").unwrap();
    publish(&tmp, &out).unwrap();
    assert_eq!(fs::read(&out).unwrap(), b"payload");
    assert!(matches!(publish(&tmp, &out), Err(Error::OutputExists(_))));

    // the path taken on file systems without hard links
    let out2 = s.path("o2.pdf");
    copy_exclusive(&tmp, &out2).unwrap();
    assert_eq!(fs::read(&out2).unwrap(), b"payload");
    assert!(matches!(copy_exclusive(&tmp, &out2), Err(Error::OutputExists(_))));
    assert_eq!(fs::read(&out2).unwrap(), b"payload");
}

#[test]
fn nonsense_options_are_rejected_before_any_work() {
    let s = Scratch::new();
    let input = s.copy("redact-source.pdf");
    let cases: Vec<(&str, Box<dyn Fn(&mut Options)>)> = vec![
        ("to_ppi 0", Box::new(|o| o.images.color.to_ppi = 0)),
        ("above 10", Box::new(|o| o.images.gray.above_ppi = 10)),
        ("above 5000", Box::new(|o| o.images.mono.above_ppi = 5000)),
        ("to above threshold", Box::new(|o| { o.images.color.above_ppi = 150; o.images.color.to_ppi = 300; })),
        ("quality 0", Box::new(|o| o.images.color.quality = 0)),
        ("quality 101", Box::new(|o| o.images.gray.quality = 101)),
        ("garbage 5", Box::new(|o| o.garbage = 5)),
    ];
    for (name, f) in cases {
        let mut o = Options::preset(Preset::Custom);
        f(&mut o);
        match run(&input, &o) {
            Err(e @ Error::InvalidOptions(_)) => assert_eq!(serde_json::to_string(&e).unwrap(), "\"invalid-options\""),
            other => panic!("{name}: {other:?}"),
        }
    }
    // "never downsample" is still fine
    let mut o = Options::preset(Preset::Custom);
    o.images.color.above_ppi = 0;
    o.images.color.to_ppi = 0;
    run(&input, &o).unwrap();
    assert_eq!(s.files().len(), 2, "rejected runs left files: {:?}", s.files());
}

/// A 1-bit stencil mask (ImageMask true), painted black through a page: MuPDF's pixmap for it is alpha only.
#[test]
fn stencil_masks_get_ccitt_and_look_the_same() {
    let s = Scratch::new();
    let input = s.path("stencil.pdf");
    {
        let mut doc = PdfDocument::new();
        let mut bits = vec![0u8; 100 * 600];
        for (i, b) in bits.iter_mut().enumerate() {
            *b = if (i / 100 / 20) % 2 == 0 { 0x0F } else { 0xC3 };
        }
        let dict = doc.new_object_from_str("<< /Type /XObject /Subtype /Image /Width 800 /Height 600 /ImageMask true /BitsPerComponent 1 >>").unwrap();
        let image = doc.add_stream(&mupdf::Buffer::from_bytes(&bits).unwrap(), Some(&dict), false).unwrap();
        let mut page = doc.new_page(Size::A4).unwrap();
        let o = InsertImageOptions::default();
        page.insert_image(&mut doc, Rect::new(36.0, 36.0, 324.0, 261.0), PageImageSource::ExistingXref(image.as_indirect().unwrap()), o).unwrap();
        doc.save(input.to_str().unwrap()).unwrap();
    }
    let r = run(&input, &Options::default()).unwrap();
    assert_eq!(r.images.replaced, 1, "{:?}", r.images);
    assert!(r.size_after * 3 < r.size_before, "{} -> {}", r.size_before, r.size_after);
    let d = open_pdf(&r.output, None).unwrap();
    let fax = (1..d.xref_len().unwrap())
        .filter_map(|x| d.xref_object(x as i32).ok().flatten())
        .any(|o| {
            let t = o.to_string();
            t.contains("CCITTFaxDecode") && t.contains("/ImageMask true")
        });
    assert!(fax, "the stencil was not re-encoded as a CCITT image mask");
    assert_eq!(r.visual.unwrap().max_diff, 0);
}
