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
    visual::compare(&open(a), &open(b), pages, &[], &AtomicBool::new(false), |_, _| {}).unwrap()
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

/// The comparison the feature doc asks for. Acrobat's outputs come from the study; both sides are measured with
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
        .filter(|n| n.ends_with(".pdf") && !n.starts_with("opt-out") && !n.starts_with("prot-out") && !n.contains("_Part"))
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
    doc.save_with_options(packed.to_str().unwrap(), write_options(&Options::default(), 4)).unwrap();
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
    text_doc_with(sign).save_with_options(tight.to_str().unwrap(), write_options(&Options::default(), 4)).unwrap();
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
    publish(&tmp, &out, false).unwrap();
    assert_eq!(fs::read(&out).unwrap(), b"payload");
    assert!(matches!(publish(&tmp, &out, false), Err(Error::OutputExists(_))));

    // a dialog-confirmed name may be replaced
    let (tmp2, out3) = (s.path("t2.tmp"), s.path("o3.pdf"));
    fs::write(&out3, b"old").unwrap();
    fs::write(&tmp2, b"new").unwrap();
    publish(&tmp2, &out3, true).unwrap();
    assert_eq!(fs::read(&out3).unwrap(), b"new");

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

// ---------------------------------------------------------------- estimate

/// The predicted size of every preset is within 30% of what the compressor really writes, on every
/// study sample that has images; prints the numbers (run with --nocapture).
#[test]
fn the_estimate_is_within_30_percent_of_the_real_size_on_the_study_samples() {
    let scratch = Scratch::new();
    let mut checked = 0;
    for name in ["compress-source.pdf", "opt-in-01.pdf", "scan-source.pdf", "ocr-in-01.pdf", "red-in-01.pdf", "red-in-03.pdf", "ai-in-01.pdf", "ai-in-02.pdf"] {
        let input = scratch.copy(name);
        let full = audit_file(&input, None).unwrap();
        if full.images == 0 {
            continue;
        }
        let t = Instant::now();
        let est = estimate::estimate(&input, None, &full, &AtomicBool::new(false)).unwrap();
        let ms = t.elapsed().as_millis();
        for e in &est {
            let mut o = Options::preset(e.preset);
            o.output = Some(scratch.path(&format!("{name}-{:?}.pdf", e.preset)));
            let real = run(&input, &o).unwrap().size_after as f64;
            let err = (e.bytes as f64 - real) / real;
            println!(
                "{name:22} {:12} total {:>9} est {:>9} [{:>9}..{:>9}] real {:>9} err {:+5.1}% ({ms} ms)",
                format!("{:?}", e.preset), full.total, e.bytes, e.low, e.high, real as u64, err * 100.0
            );
            assert!(err.abs() <= 0.30, "{name} {:?}: estimated {} real {real}", e.preset, e.bytes);
            assert!(e.low <= e.bytes && e.bytes <= e.high);
        }
        checked += 1;
    }
    assert!(checked >= 2, "samples with images: {checked}");
}

/// The sheet lists Smallest, Balanced and High quality in that order, so neither the predicted nor the
/// real sizes may run the other way. The tour showed Smallest ~56 KB above Balanced ~55 KB on
/// edit-source.pdf: the image pass had written a 2.3 KB ICC profile for the one image it rewrote
/// (the file had no profile of its own), more than the recompression saved.
#[test]
fn the_presets_never_rank_a_smaller_preset_above_a_larger_one() {
    use super::audit::Category as C;
    let scratch = Scratch::new();
    let mut bad = Vec::new();
    for name in ["edit-source.pdf", "compress-source.pdf", "opt-in-01.pdf", "scan-source.pdf", "form-source.pdf", "red-in-01.pdf", "pages-a.pdf"] {
        let input = scratch.copy(name);
        let full = audit_file(&input, None).unwrap();
        let est = estimate::estimate(&input, None, &full, &AtomicBool::new(false)).unwrap();
        let by = |p: Preset| est.iter().find(|e| e.preset == p).unwrap().bytes;
        let est = [by(Preset::Smallest), by(Preset::Balanced), by(Preset::HighQuality)];
        let mut real = Vec::new();
        for p in [Preset::Smallest, Preset::Balanced, Preset::HighQuality] {
            let mut o = Options::preset(p);
            let out = scratch.path(&format!("{name}-{p:?}.pdf"));
            o.output = Some(out.clone());
            real.push(run(&input, &o).unwrap().size_after);
            // "Other" holds streams that are none of the known kinds: the invented profile used to land there.
            let after = audit_file(&out, None).unwrap();
            if after.bytes(C::Other) > full.bytes(C::Other) + 50 {
                bad.push(format!("{name} {p:?}: {} B of unexplained streams (input {})", after.bytes(C::Other), full.bytes(C::Other)));
            }
        }
        println!("{name:22} total {:>8} est {est:?} real {real:?}", full.total);
        if !(est[0] <= est[1] && est[1] <= est[2]) {
            bad.push(format!("{name}: estimated smallest {}, balanced {}, high {}", est[0], est[1], est[2]));
        }
        // Two presets that write the same content differ by the random /ID of each save (see the test below).
        if !(real[0] <= real[1] + ID_JITTER && real[1] <= real[2] + ID_JITTER) {
            bad.push(format!("{name}: real smallest {}, balanced {}, high {}", real[0], real[1], real[2]));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// The most the size of a save varies with its random /ID: MuPDF writes the new second half as 34 bytes of hex, or as a
/// literal string when that is shorter (as short as 18 bytes), one save in eighty.
const ID_JITTER: u64 = 16;

/// MuPDF gives a saved file a new random second half of its /ID (`change_identity`, pdf-write.c), so the same file
/// came out up to `ID_JITTER` bytes smaller now and then. The estimate wrote the sample several times and let that
/// into its ratios: two presets with the same content swapped places by a byte.
#[test]
fn the_estimate_of_a_file_with_an_id_is_the_same_every_time() {
    let scratch = Scratch::new();
    let input = scratch.copy("edit-source.pdf");
    let full = audit_file(&input, None).unwrap();
    // one save in eighty is shorter and the estimate makes four: before the fix 600 rounds gave 11 different answers
    let runs: std::collections::BTreeSet<Vec<u64>> = (0..100)
        .map(|_| estimate::estimate(&input, None, &full, &AtomicBool::new(false)).unwrap().iter().map(|e| e.bytes).collect())
        .collect();
    assert_eq!(runs.len(), 1, "{runs:?}");
}

/// The estimate keeps the sample's /ID when the file is encrypted: the key of the older encryption forms comes from
/// it, and a sample that lost it would not open again.
#[test]
fn the_estimate_of_an_encrypted_file_still_opens_its_sample() {
    let scratch = Scratch::new();
    let source = scratch.copy("edit-source.pdf");
    // an owner password only: the file opens with no password, and so does the sample
    for (i, enc) in [Encryption::Rc4_128, Encryption::Aes128, Encryption::Aes256].into_iter().enumerate() {
        let input = scratch.path(&format!("locked-{i}.pdf"));
        let mut w = PdfWriteOptions::default();
        w.set_encryption(enc).set_owner_password("owner");
        PdfDocument::open(source.to_str().unwrap()).unwrap().save_with_options(input.to_str().unwrap(), w).unwrap();
        let full = audit_file(&input, None).unwrap();
        let est = estimate::estimate(&input, None, &full, &AtomicBool::new(false));
        assert!(est.is_ok(), "{enc:?}: {est:?}");
    }
}

/// A big, image-heavy file: the sample covers a small part of it; the estimate is close and the range
/// still holds the real size. Prints the time (a release build is the one that matters: < 1 s).
#[test]
fn the_estimate_of_an_image_heavy_file_brackets_the_real_size() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    // Distinct photos on every page (identical images would be merged across the whole file, which a
    // sample of a few pages cannot see; the range is wide for those files).
    let mut doc = PdfDocument::new();
    for i in 0..40u32 {
        let mut page = doc.new_page(Size::A4).unwrap();
        let o = InsertImageOptions::default();
        page.insert_image(&mut doc, Rect::new(36.0, 36.0, 400.0, 300.0), PageImageSource::Pixmap(&photo(1100, 800, true, i * 2 + 1)), o).unwrap();
        page.insert_image(&mut doc, Rect::new(36.0, 320.0, 300.0, 520.0), PageImageSource::Pixmap(&photo(800, 600, false, i * 2 + 2)), o).unwrap();
    }
    doc.save(input.to_str().unwrap()).unwrap();
    let full = audit_file(&input, None).unwrap();
    let t = Instant::now();
    let est = estimate::estimate(&input, None, &full, &AtomicBool::new(false)).unwrap();
    println!("heavy.pdf {} B estimated in {} ms", full.total, t.elapsed().as_millis());
    for e in &est {
        let mut o = Options::preset(e.preset);
        o.output = Some(s.path(&format!("heavy-{:?}.pdf", e.preset)));
        let real = run(&input, &o).unwrap().size_after;
        let err = (e.bytes as f64 - real as f64) / real as f64;
        println!("  {:?}: est {} [{}..{}] real {real} err {:+.1}%", e.preset, e.bytes, e.low, e.high, err * 100.0);
        assert!(err.abs() <= 0.30, "{:?}", e.preset);
        assert!(e.low <= real && real <= e.high, "{:?}: {real} outside {}..{}", e.preset, e.low, e.high);
    }
}

// ---------------------------------------------------------------- optimizer (Advanced settings)

use mupdf::pdf::PdfObject;

fn add(doc: &mut PdfDocument, src: &str) -> PdfObject {
    let o = doc.new_object_from_str(src).unwrap();
    doc.add_object(&o).unwrap()
}

fn xref_of(o: &PdfObject) -> i32 {
    o.as_indirect().unwrap()
}

fn page_ref(doc: &PdfDocument, i: i32) -> String {
    format!("{} 0 R", xref_of(&doc.find_page(i).unwrap()))
}

fn put(doc: &PdfDocument, mut target: PdfObject, key: &str, src: &str) {
    let v = doc.new_object_from_str(src).unwrap();
    target.dict_put(key, v).unwrap();
}

/// Balanced settings with every image class left as it is: MuPDF's image pass does not run at all, so
/// what the discard options do is not mixed up with what it does (images and forms it rewrites are new
/// objects that carry no /Alternates and no /PieceInfo).
fn keep_images() -> Options {
    let mut o = Options::preset(Preset::Balanced);
    let keep = ImageClass { above_ppi: 0, to_ppi: 0, codec: Codec::Retain, quality: 0 };
    o.images = Images { color: keep, gray: keep, mono: keep, only_if_smaller: true };
    o
}

/// A document with one of everything the discard options talk about (3 pages, bookmarks).
fn kitchen_sink(path: &Path) {
    let mut doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 3, chapter_every: 1, needle_every: 0, lines: 6 })).unwrap();
    let (r1, r2, r3) = (page_ref(&doc, 0), page_ref(&doc, 1), page_ref(&doc, 2));

    // document information and an XMP packet
    let info = add(&mut doc, "<< /Title (Sink) /Author (Me) >>");
    doc.trailer().unwrap().dict_put("Info", info).unwrap();
    let xmp_dict = doc.new_object_from_str("<< /Type /Metadata /Subtype /XML >>").unwrap();
    let xmp = doc.add_stream(&mupdf::Buffer::from_bytes(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap(), Some(&xmp_dict), false).unwrap();
    doc.catalog().unwrap().dict_put("Metadata", xmp).unwrap();

    // an attachment, a named destination, a script
    doc.add_embedded_file("note.txt", b"attached", mupdf::pdf::EmbeddedFileOptions::new("note.txt")).unwrap();
    let mut names = doc.catalog().unwrap().get_dict("Names").unwrap().unwrap();
    let dests = doc.new_object_from_str(&format!("<< /Names [ (chap) [{r3} /Fit] ] >>")).unwrap();
    names.dict_put("Dests", dests).unwrap();
    let js = doc.new_object_from_str("<< /S /JavaScript /JS (app.alert\\(1\\)) >>").unwrap();
    doc.catalog().unwrap().dict_put("OpenAction", js).unwrap();

    // a page thumbnail, and an image with an alternate
    let thumb_dict = doc
        .new_object_from_str("<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 >>")
        .unwrap();
    let thumb = doc.add_stream(&mupdf::Buffer::from_bytes(&[0, 80, 160, 255]).unwrap(), Some(&thumb_dict), false).unwrap();
    let mut p0 = doc.find_page(0).unwrap();
    let thumb_ref = doc.new_indirect(xref_of(&thumb), 0).unwrap();
    p0.dict_put("Thumb", thumb_ref).unwrap();
    let mut page = doc.load_pdf_page(0).unwrap();
    let placed = page
        .insert_image(&mut doc, Rect::new(300.0, 300.0, 360.0, 345.0), PageImageSource::Pixmap(&photo(40, 30, true, 5)), InsertImageOptions::default())
        .unwrap();
    drop(page);
    let img = doc.new_indirect(placed.xref, 0).unwrap();
    put(&doc, img, "Alternates", &format!("[ << /Image {} 0 R /DefaultForPrinting true >> ]", xref_of(&thumb)));

    // private data of other applications: catalog, two pages, a form XObject
    let cat = doc.catalog().unwrap();
    put(&doc, cat, "PieceInfo", "<< /App << /Data (x) >> >>");
    put(&doc, doc.find_page(0).unwrap(), "PieceInfo", "<< /App << /Data (y) >> >>");
    put(&doc, doc.find_page(2).unwrap(), "PieceInfo", "<< /App << /Data (z) >> >>");
    let form_dict = doc.new_object_from_str("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /PieceInfo << /App << /Data (f) >> >> >>").unwrap();
    let form = doc.add_stream(&mupdf::Buffer::from_bytes(b"").unwrap(), Some(&form_dict), false).unwrap();
    let mut res = doc.find_page(2).unwrap().get_dict("Resources").unwrap().unwrap();
    let xo = doc.new_object_from_str("<< >>").unwrap();
    res.dict_put("XObject", xo).unwrap();
    res.get_dict("XObject").unwrap().unwrap().dict_put("Fm1", form).unwrap();
    let draw = doc.add_stream(&mupdf::Buffer::from_bytes(b"q /Fm1 Do Q\n").unwrap(), None, false).unwrap();
    let mut p2 = doc.find_page(2).unwrap();
    let old = p2.get_dict("Contents").unwrap().unwrap();
    let mut parts = doc.new_array().unwrap();
    if old.is_array().unwrap() {
        for part in old.array_iter().unwrap().flatten() {
            parts.array_push(part).unwrap();
        }
    } else {
        parts.array_push(old).unwrap();
    }
    parts.array_push(draw).unwrap();
    p2.dict_put("Contents", parts).unwrap();

    // form fields with actions: submit, reset, a reset under /AA next to a script, and a URI that stays
    let mut w = |extra: &str| add(&mut doc, &format!("<< /Type /Annot /Subtype /Widget /Rect [10 10 80 30] {extra} >>"));
    let w1 = w("/FT /Btn /T (submit) /A << /S /SubmitForm /F (http://x) >>");
    let w2 = w("/FT /Btn /T (reset) /A << /S /ResetForm >>");
    let w3 = w("/FT /Tx /T (name) /AA << /Fo << /S /JavaScript /JS (1) >> /K << /S /ResetForm >> >>");
    let w4 = w("/FT /Tx /T (url) /A << /S /URI /URI (http://a) >>");
    let fields: Vec<String> = [&w1, &w2, &w3, &w4].iter().map(|o| format!("{} 0 R", xref_of(o))).collect();
    let acro = doc.new_object_from_str(&format!("<< /Fields [{}] >>", fields.join(" "))).unwrap();
    doc.catalog().unwrap().dict_put("AcroForm", acro).unwrap();
    put(&doc, doc.find_page(0).unwrap(), "Annots", &format!("[{}]", fields.join(" ")));

    // links: three that work, three that lead nowhere
    let mut l = |tag: &str, action: &str| add(&mut doc, &format!("<< /Type /Annot /Subtype /Link /Rect [10 100 80 120] /Contents ({tag}) {action} >>"));
    let links = [
        l("ok-page", &format!("/Dest [{r1} /Fit]")),
        l("dead-name", "/Dest /NoSuchName"),
        l("ok-uri", "/A << /S /URI /URI (http://b) >>"),
        l("dead-none", ""),
        l("ok-name", "/Dest (chap)"),
        l("dead-goto", "/A << /S /GoTo /D (missing) >>"),
    ];
    let refs: Vec<String> = links.iter().map(|o| format!("{} 0 R", xref_of(o))).collect();
    put(&doc, doc.find_page(1).unwrap(), "Annots", &format!("[{}]", refs.join(" ")));
    let _ = r2;
    doc.save(path.to_str().unwrap()).unwrap();
}

#[derive(Debug, Default, Clone, PartialEq)]
struct Probe {
    info: bool,
    xmp: bool,
    outlines: bool,
    files: bool,
    /// Script actions anywhere (open action, field and annotation actions).
    js: u32,
    thumbs: u32,
    /// Submit-form, reset-form and import-data actions.
    form_actions: u32,
    /// Images that carry /Alternates.
    alternates: u32,
    /// PieceInfo on the catalog and the pages / on anything else.
    piece_catalog_pages: u32,
    piece_other: u32,
    /// `/Contents` of every link annotation left, in page order.
    links: Vec<String>,
}

fn probe(p: &Path) -> Probe {
    let d = open_pdf(p, None).unwrap();
    let cat = d.catalog().unwrap();
    let mut pr = Probe {
        info: d.trailer().unwrap().get_dict("Info").unwrap().is_some(),
        xmp: cat.get_dict("Metadata").unwrap().is_some(),
        outlines: cat.get_dict("Outlines").unwrap().is_some(),
        files: !d.embedded_files().unwrap().is_empty(),
        ..Probe::default()
    };
    let kind = |a: &PdfObject| a.get_dict("S").ok().flatten().and_then(|s| s.as_name().ok()).unwrap_or_default();
    let count_action = |a: &PdfObject, pr: &mut Probe| match kind(a).as_slice() {
        b"JavaScript" => pr.js += 1,
        b"SubmitForm" | b"ResetForm" | b"ImportData" => pr.form_actions += 1,
        _ => {}
    };
    if let Some(a) = cat.get_dict("OpenAction").unwrap() {
        if a.is_dict().unwrap() {
            count_action(&a, &mut pr);
        }
    }
    for i in 0..d.page_count().unwrap() {
        let page = d.find_page(i).unwrap();
        pr.thumbs += page.get_dict("Thumb").unwrap().is_some() as u32;
        if let Some(annots) = page.get_dict("Annots").unwrap() {
            for a in annots.array_iter().unwrap().flatten() {
                if a.get_dict("Subtype").unwrap().is_some_and(|s| s.as_name().unwrap() == b"Link") {
                    pr.links.push(a.get_dict("Contents").unwrap().unwrap().as_string().unwrap());
                }
            }
        }
    }
    for x in 1..d.xref_len().unwrap() as i32 {
        let o = d.new_indirect(x, 0).unwrap();
        if !matches!(o.resolve(), Ok(Some(v)) if v.is_dict().unwrap_or(false)) {
            continue;
        }
        let ty = o.get_dict("Type").unwrap().and_then(|t| t.as_name().ok()).unwrap_or_default();
        if o.get_dict("PieceInfo").unwrap().is_some() {
            if ty == b"Catalog" || ty == b"Page" {
                pr.piece_catalog_pages += 1;
            } else {
                pr.piece_other += 1;
            }
        }
        if o.is_stream().unwrap() && o.get_dict("Alternates").unwrap().is_some() {
            pr.alternates += 1;
        }
        if let Some(a) = o.get_dict("A").unwrap() {
            if a.is_dict().unwrap() {
                count_action(&a, &mut pr);
            }
        }
        if let Some(aa) = o.get_dict("AA").unwrap().filter(|a| a.is_dict().unwrap_or(false)) {
            for (_, v) in aa.dict_iter().unwrap().flatten() {
                count_action(&v, &mut pr);
            }
        }
    }
    pr
}

#[test]
fn each_discard_option_removes_exactly_what_it_says() {
    let s = Scratch::new();
    let input = s.path("sink.pdf");
    kitchen_sink(&input);
    let base = probe(&input);
    // the fixture really has one of everything
    assert!(base.info && base.xmp && base.outlines && base.files, "{base:?}");
    assert_eq!((base.js, base.form_actions, base.thumbs, base.alternates), (2, 3, 1, 1), "{base:?}");
    assert_eq!((base.piece_catalog_pages, base.piece_other), (3, 1), "{base:?}");
    assert_eq!(base.links.len(), 6, "{base:?}");

    type Effect = fn(&mut Probe);
    let cases: Vec<(&str, Strip, Effect)> = vec![
        ("metadata", Strip { metadata: true, ..Strip::default() }, |p| {
            p.info = false;
            p.xmp = false;
            p.piece_catalog_pages = 0;
        }),
        ("thumbnails", Strip { thumbnails: true, ..Strip::default() }, |p| p.thumbs = 0),
        ("embedded files", Strip { embedded_files: true, ..Strip::default() }, |p| p.files = false),
        // The reset action next to the script in a field's /AA stays: it is not JavaScript.
        ("javascript", Strip { javascript: true, ..Strip::default() }, |p| p.js = 0),
        ("bookmarks", Strip { bookmarks: true, ..Strip::default() }, |p| p.outlines = false),
        // The script next to the reset action stays: it is not a form action.
        ("form actions", Strip { form_actions: true, ..Strip::default() }, |p| p.form_actions = 0),
        ("alternate images", Strip { alternate_images: true, ..Strip::default() }, |p| p.alternates = 0),
        ("private data", Strip { private_data: true, ..Strip::default() }, |p| {
            p.piece_catalog_pages = 0;
            p.piece_other = 0;
        }),
        ("all links", Strip { links: true, ..Strip::default() }, |p| p.links.clear()),
        ("invalid links", Strip { invalid_links: true, ..Strip::default() }, |p| p.links.retain(|c| c.starts_with("ok-"))),
    ];
    // What the engine does to this file when nothing is asked: the yardstick for every option.
    let mut o = keep_images();
    o.output = Some(s.path("nothing.pdf"));
    let plain = probe(&run(&input, &o).unwrap().output);
    let mut failures = Vec::new();
    if plain != base {
        failures.push(format!("nothing asked changed the file:\n  was  {base:?}\n  is   {plain:?}"));
    }
    for (name, strip, effect) in cases {
        let mut o = keep_images();
        o.strip = strip;
        o.output = Some(s.path(&format!("{}.pdf", name.replace(' ', "-"))));
        let r = run(&input, &o).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut want = base.clone();
        effect(&mut want);
        let got = probe(&r.output);
        if got != want {
            failures.push(format!("{name}:\n  want {want:?}\n  got  {got:?}"));
        }
        if !(r.stripped.any() && !r.kept_original) {
            failures.push(format!("{name}: not reported or kept the original: {:?}", r.stripped));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    // the counts in the report
    let mut o = keep_images();
    o.strip = Strip { links: true, form_actions: true, alternate_images: true, private_data: true, ..Strip::default() };
    o.output = Some(s.path("counted.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!((r.stripped.links, r.stripped.form_actions, r.stripped.alternate_images), (6, 3, 1));
    assert_eq!(r.stripped.private_data, 4);
}

/// Fonts with a file of their own: one plain TrueType subset, one symbolic, one composite.
fn font_sink(path: &Path) {
    let mut doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 2, chapter_every: 0, needle_every: 0, lines: 6 })).unwrap();
    let mut seed = 7u32;
    let mut blob = |n: usize| -> PdfObject {
        let bytes: Vec<u8> = (0..n).map(|_| lcg(&mut seed) as u8).collect();
        let dict = doc.new_object_from_str("<< >>").unwrap();
        doc.add_stream(&mupdf::Buffer::from_bytes(&bytes).unwrap(), Some(&dict), false).unwrap()
    };
    let file_a = blob(20_000);
    let file_b = blob(9_000);
    let file_c = blob(6_000);
    let desc = |doc: &mut PdfDocument, name: &str, flags: u32, file: &PdfObject| {
        add(
            doc,
            &format!(
                "<< /Type /FontDescriptor /FontName /{name} /Flags {flags} /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 {} 0 R >>",
                xref_of(file)
            ),
        )
    };
    let da = desc(&mut doc, "ABCDEF+TestSans", 32, &file_a);
    let db = desc(&mut doc, "Dingbats", 4, &file_b);
    let dc = desc(&mut doc, "GHIJKL+Composite", 32, &file_c);
    let fa = add(&mut doc, &format!("<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+TestSans /Encoding /WinAnsiEncoding /FontDescriptor {} 0 R >>", xref_of(&da)));
    let fb = add(&mut doc, &format!("<< /Type /Font /Subtype /TrueType /BaseFont /Dingbats /FontDescriptor {} 0 R >>", xref_of(&db)));
    let cid = add(&mut doc, &format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /GHIJKL+Composite /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {} 0 R >>", xref_of(&dc)));
    let fc = add(&mut doc, &format!("<< /Type /Font /Subtype /Type0 /BaseFont /GHIJKL+Composite /Encoding /Identity-H /DescendantFonts [{} 0 R] >>", xref_of(&cid)));
    let mut res = doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap();
    let mut fonts = res.get_dict("Font").unwrap().unwrap();
    fonts.dict_put("FA", fa).unwrap();
    fonts.dict_put("FB", fb).unwrap();
    fonts.dict_put("FC", fc).unwrap();
    let _ = &mut res;
    doc.save(path.to_str().unwrap()).unwrap();
}

#[test]
fn unembed_takes_out_only_the_fonts_that_can_go() {
    let s = Scratch::new();
    let input = s.path("fonts.pdf");
    font_sink(&input);
    let a = audit_file(&input, None).unwrap();
    let row = |a: &Audit, n: &str| a.font_list.iter().find(|f| f.name == n).cloned().unwrap_or_else(|| panic!("no font {n}: {:?}", a.font_list));
    let sans = row(&a, "ABCDEF+TestSans");
    assert!(sans.embedded && sans.subset && sans.can_unembed && sans.bytes >= 20_000, "{sans:?}");
    let dingbats = row(&a, "Dingbats");
    assert!(dingbats.embedded && !dingbats.can_unembed, "{dingbats:?}");
    assert!(dingbats.why_not.as_deref().unwrap().contains("symbolic"));
    let comp = row(&a, "GHIJKL+Composite");
    assert!(comp.embedded && !comp.can_unembed && comp.why_not.as_deref().unwrap().contains("composite"), "{comp:?}");
    let before_text = text(&input);

    // by name
    let mut o = Options::preset(Preset::Balanced);
    o.unembed_fonts = vec!["ABCDEF+TestSans".into(), "Dingbats".into()];
    o.output = Some(s.path("by-name.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.unembedded.files, 1);
    assert!(r.unembedded.bytes >= 20_000);
    assert_eq!(r.unembedded.skipped.len(), 1);
    assert_eq!(r.unembedded.skipped[0].0, "Dingbats");
    assert!(r.warnings.iter().any(|w| w.contains("Dingbats stays embedded")), "{:?}", r.warnings);
    let after = audit_file(&r.output, None).unwrap();
    let sans = row(&after, "TestSans");
    assert!(!sans.embedded && !sans.subset, "{sans:?}");
    assert!(row(&after, "Dingbats").embedded && row(&after, "GHIJKL+Composite").embedded);
    assert!(after.bytes(Category::Fonts) + 19_000 <= a.bytes(Category::Fonts), "{} -> {}", a.bytes(Category::Fonts), after.bytes(Category::Fonts));
    assert_eq!(text(&r.output), before_text);
    assert!(!r.kept_original);

    // every font that may be: the others are left alone without a word
    let mut o = Options::preset(Preset::Balanced);
    o.unembed_all = true;
    o.output = Some(s.path("all.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.unembedded.files, 1);
    assert!(r.unembedded.skipped.is_empty() && r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(audit_file(&r.output, None).unwrap().font_list.iter().filter(|f| f.embedded).count(), 2);

    // a name that is not in the file is reported, not an error
    let mut o = Options::preset(Preset::Balanced);
    o.unembed_fonts = vec!["Nope".into()];
    o.output = Some(s.path("nope.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.unembedded.files, 0);
    assert!(r.unembedded.skipped[0].1.contains("No such font"));
}

/// The embedded search index is a copy of the bookmarks too: a pass that drops them must not leave it behind.
#[test]
fn an_embedded_search_index_that_no_longer_matches_does_not_survive_a_compress() {
    use crate::search::catalog::embed;
    let s = Scratch::new();
    let input = s.path("indexed.pdf");
    let mut doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 3, chapter_every: 1, needle_every: 0, lines: 4 })).unwrap();
    embed::add(&mut doc).unwrap();
    doc.save(&input.to_string_lossy()).unwrap();
    drop(doc);
    let indexed = |p: &Path| embed::status(&PdfDocument::open(p.to_str().unwrap()).unwrap());
    assert!(indexed(&input).current);

    let mut o = Options::preset(Preset::Balanced);
    o.strip.bookmarks = true;
    o.output = Some(s.path("no-bookmarks.pdf"));
    let r = run(&input, &o).unwrap();
    assert!(!r.kept_original);
    assert!(!indexed(&r.output).present, "the bookmarks' titles are not kept in the index");
}

/// Fonts that differ only in their /Encoding: three a replacement can follow, three it cannot.
fn encoding_sink(path: &Path) {
    let mut doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 1, chapter_every: 0, needle_every: 0, lines: 4 })).unwrap();
    let mut seed = 11u32;
    let mut font = |doc: &mut PdfDocument, name: &str, subtype: &str, encoding: &str| -> PdfObject {
        let bytes: Vec<u8> = (0..3_000).map(|_| lcg(&mut seed) as u8).collect();
        let dict = doc.new_object_from_str("<< >>").unwrap();
        let file = doc.add_stream(&mupdf::Buffer::from_bytes(&bytes).unwrap(), Some(&dict), false).unwrap();
        let desc = add(
            doc,
            &format!(
                "<< /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 {} 0 R >>",
                xref_of(&file)
            ),
        );
        add(doc, &format!("<< /Type /Font /Subtype /{subtype} /BaseFont /{name} {encoding} /FontDescriptor {} 0 R >>", xref_of(&desc)))
    };
    let list = [
        ("FineDiffs", "Type1", "/Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [ 65 /eacute /Euro 70 /space ] >>"),
        ("FineMac", "Type1", "/Encoding /MacRomanEncoding"),
        ("FineBare", "TrueType", ""),
        ("OddGlyphs", "TrueType", "/Encoding << /Type /Encoding /Differences [ 1 /g12 /glyph34 ] >>"),
        ("OddBare", "Type1", ""),
        ("OddExpert", "Type1", "/Encoding /MacExpertEncoding"),
    ];
    let mut objs = Vec::new();
    for (name, subtype, enc) in list {
        objs.push((name, font(&mut doc, name, subtype, enc)));
    }
    let res = doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap();
    let mut fonts = res.get_dict("Font").unwrap().unwrap();
    for (name, o) in objs {
        fonts.dict_put(name, o).unwrap();
    }
    doc.save(path.to_str().unwrap()).unwrap();
}

#[test]
fn unembed_leaves_fonts_whose_encoding_a_replacement_cannot_follow() {
    let s = Scratch::new();
    let input = s.path("encodings.pdf");
    encoding_sink(&input);
    let a = audit_file(&input, None).unwrap();
    let row = |n: &str| a.font_list.iter().find(|f| f.name == n).cloned().unwrap_or_else(|| panic!("no font {n}: {:?}", a.font_list));
    for n in ["FineDiffs", "FineMac", "FineBare"] {
        let f = row(n);
        assert!(f.embedded && f.can_unembed, "{f:?}");
    }
    let why = |n: &str| {
        let f = row(n);
        assert!(f.embedded && !f.can_unembed, "{f:?}");
        f.why_not.unwrap()
    };
    assert!(why("OddGlyphs").contains("g12"), "{}", why("OddGlyphs"));
    assert!(why("OddBare").contains("no /Encoding"), "{}", why("OddBare"));
    assert!(why("OddExpert").contains("not Standard, WinAnsi or MacRoman"), "{}", why("OddExpert"));

    // "every font that can be" takes exactly the three, and the page that shows them is listed
    let mut o = Options::preset(Preset::Balanced);
    o.unembed_all = true;
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.unembedded.files, 3, "{:?}", r.unembedded);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.unembedded.font_objects.len(), 3);
    let doc = open_pdf(&input, None).unwrap();
    assert_eq!(fonts::pages_using(&doc, &r.unembedded.font_objects).unwrap(), vec![0]);
    assert!(fonts::pages_using(&doc, &[]).unwrap().is_empty());
    let after = audit_file(&r.output, None).unwrap();
    assert_eq!(after.font_list.iter().filter(|f| f.embedded).count(), 3);

    // naming one of the others says why it stayed
    let mut o = Options::preset(Preset::Balanced);
    o.unembed_fonts = vec!["OddGlyphs".into()];
    o.output = Some(s.path("named.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.unembedded.files, 0);
    assert!(r.warnings.iter().any(|w| w.contains("OddGlyphs stays embedded") && w.contains("g12")), "{:?}", r.warnings);
}

#[test]
fn the_visual_check_always_includes_the_pages_the_run_changed() {
    // the sample spreads over the document; flattened pages and pages in a changed font are added to it
    assert_eq!(visual::picks_for(100, 3, &[]), vec![0, 49, 99]);
    assert_eq!(visual::picks_for(100, 3, &[10, 10, 55, 200]), vec![0, 10, 49, 55, 99]);
    // a run that changed hundreds of pages does not render hundreds of them
    let many: Vec<usize> = (0..400).collect();
    let picks = visual::picks_for(400, 8, &many);
    assert!(picks.len() <= 8 + 24, "{}", picks.len());
    assert!(picks.windows(2).all(|w| w[0] < w[1]));

    // end to end: a one-page sample still looks at both pages that were flattened
    let s = Scratch::new();
    let input = s.path("transparent.pdf");
    transparent_sink(&input, false);
    let mut o = Options::preset(Preset::Balanced);
    o.transparency.flatten = true;
    o.visual_pages = 1;
    o.output = Some(s.path("flat.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.flattened.pages, vec![1, 2], "{:?}", r.flattened);
    let v = r.visual.expect("visual check ran");
    assert_eq!(v.per_page.iter().map(|p| p.page).collect::<Vec<_>>(), vec![0, 1]);

    // the number of pages the check may cover is bounded
    let mut o = Options::preset(Preset::Balanced);
    o.visual_pages = 65;
    assert!(matches!(run(&input, &o), Err(Error::InvalidOptions(_))));
}

#[test]
fn a_flattened_page_that_looks_the_same_does_not_come_back_as_different() {
    let s = Scratch::new();
    let input = s.path("transparent.pdf");
    // both transparent pages declare a graphics state their content never uses: flattening changes no pixel
    transparent_sink(&input, false);
    let mut o = keep_images();
    o.transparency.flatten = true;
    o.transparency.ppi = 150;
    o.visual_pages = 1;
    o.output = Some(s.path("flat.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.flattened.pages, vec![1, 2], "{:?}", r.flattened);
    let v = r.visual.as_ref().expect("visual check ran");
    assert_eq!(v.per_page.iter().map(|p| p.page).collect::<Vec<_>>(), vec![0, 1]);
    println!("flat, same resolution: psnr {:.1} max {} mean {:.3}", v.psnr, v.max_diff, v.mean_diff);
    // "Looks the same" in the result screen: at most 8 of 255, nowhere near the red level
    assert!(v.max_diff <= 8, "{v:?}");
    assert!(v.psnr >= 40.0 && v.p999_diff <= 8 && !v.looks_different(), "{v:?}");

    // at the sheet's usual 96 ppi the same pair differs only by resampling: that is what the check avoids
    let at_96 = diff(&input, &r.output, 1);
    println!("flat, 96 ppi: psnr {:.1} max {}", at_96.psnr, at_96.max_diff);
    assert!(v.per_page[0].psnr >= at_96.per_page[0].psnr, "{v:?} vs {at_96:?}");
}

#[test]
fn a_page_with_other_letters_is_reported_as_different_and_named() {
    let s = Scratch::new();
    let original = s.path("original.pdf");
    fs::write(&original, fixture::build(&fixture::Fixture { pages: 3, chapter_every: 0, needle_every: 0, lines: 6 })).unwrap();
    // the same file with every letter of page 2's text moved 13 places along: glyphs swapped, layout kept
    let doc = PdfDocument::from_bytes(&fs::read(&original).unwrap()).unwrap();
    let mut contents = doc.find_page(1).unwrap().get_dict("Contents").unwrap().unwrap();
    let mut inside = false;
    let swapped: Vec<u8> = contents
        .read_stream()
        .unwrap()
        .iter()
        .map(|&b| match b {
            b'(' => {
                inside = true;
                b
            }
            b')' => {
                inside = false;
                b
            }
            b'a'..=b'z' if inside => (b - b'a' + 13) % 26 + b'a',
            b'A'..=b'Z' if inside => (b - b'A' + 13) % 26 + b'A',
            _ => b,
        })
        .collect();
    contents.write_stream_buffer(&mupdf::Buffer::from_bytes(&swapped).unwrap()).unwrap();
    let changed = s.path("changed.pdf");
    doc.save(changed.to_str().unwrap()).unwrap();

    let same = diff(&original, &original, 3);
    assert!(!same.looks_different() && same.max_diff == 0 && same.p999_diff == 0, "{same:?}");
    let v = diff(&original, &changed, 3);
    println!("glyph swap: psnr {:.1} p999 {} max {} worst page {}", v.psnr, v.p999_diff, v.max_diff, v.worst_page);
    assert!(v.looks_different(), "{v:?}");
    assert_eq!(v.worst_page, 1, "{v:?}");
    // the untouched pages are exact
    assert!(v.per_page.iter().filter(|p| p.page != 1).all(|p| p.max == 0), "{v:?}");
}

/// A document whose form fields point at each other, with one parent listed many times.
fn looping_form(path: &Path) {
    let mut doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 1, chapter_every: 0, needle_every: 0, lines: 4 })).unwrap();
    let a = add(&mut doc, "<< /FT /Tx /T (a) >>");
    let b = add(&mut doc, &format!("<< /T (b) /Parent {} 0 R >>", xref_of(&a)));
    let (xa, xb) = (xref_of(&a), xref_of(&b));
    let wide: Vec<String> = (0..300).map(|_| format!("{xa} 0 R")).collect();
    put(&doc, b, "Kids", &format!("[{}]", wide.join(" ")));
    put(&doc, a, "Kids", &format!("[{xb} 0 R {xa} 0 R]"));
    let acro = doc.new_object_from_str(&format!("<< /Fields [{xa} 0 R {xa} 0 R] >>")).unwrap();
    doc.catalog().unwrap().dict_put("AcroForm", acro).unwrap();
    doc.save(path.to_str().unwrap()).unwrap();
}

#[test]
fn the_audit_walks_a_looping_field_tree_once() {
    let s = Scratch::new();
    let input = s.path("loop.pdf");
    looping_form(&input);
    let t = Instant::now();
    let a = audit_file(&input, None).unwrap();
    assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
    let size = fs::metadata(&input).unwrap().len();
    assert_eq!(a.categories.iter().map(|c| c.bytes).sum::<u64>(), size);
    assert!(a.bytes(Category::Forms) > 0);
}

#[test]
fn rewritten_images_say_when_they_lost_their_alternates() {
    let s = Scratch::new();
    let sink = s.path("sink.pdf");
    kitchen_sink(&sink);

    // images left as they are: the image pass does not run, nothing is lost, nothing is said
    let mut o = keep_images();
    o.output = Some(s.path("kept.pdf"));
    let r = run(&sink, &o).unwrap();
    assert_eq!(r.images.replaced, 0);
    assert!(!r.warnings.iter().any(|w| w.contains("alternates")), "{:?}", r.warnings);

    // the pass runs: whatever it replaced took the alternates and the form's private data with it
    let mut o = Options::preset(Preset::Balanced);
    o.images.color = ImageClass { above_ppi: 36, to_ppi: 36, codec: Codec::Jpeg, quality: 50 };
    o.images.only_if_smaller = false;
    o.output = Some(s.path("rewritten.pdf"));
    let r = run(&sink, &o).unwrap();
    println!("images: {:?}; warnings {:?}", r.images, r.warnings);
    assert!(r.images.replaced > 0, "{:?}", r.images);
    assert!(
        r.warnings.iter().any(|w| w.starts_with("Rewritten images lost their alternates and private data")),
        "{:?}",
        r.warnings
    );

    // what the user already asked to drop is not reported as lost
    let mut o2 = o.clone();
    o2.strip.alternate_images = true;
    o2.strip.private_data = true;
    o2.output = Some(s.path("dropped.pdf"));
    let r = run(&sink, &o2).unwrap();
    assert!(!r.warnings.iter().any(|w| w.contains("alternates")), "{:?}", r.warnings);
}

#[test]
fn a_preset_file_is_read_with_care() {
    let s = Scratch::new();
    let dir = s.path("data");
    fs::create_dir_all(&dir).unwrap();

    // more than 1 MB is not one of ours
    fs::write(dir.join(presets::FILE), vec![b' '; 2 << 20]).unwrap();
    assert!(presets::load(&dir).is_empty());

    // a check count out of range comes back clamped, not lost
    let _ = fs::remove_file(dir.join(presets::FILE));
    let mut o = Options::preset(Preset::Custom);
    o.visual_pages = 5;
    presets::save(&dir, "Few", &o).unwrap();
    let text = fs::read_to_string(dir.join(presets::FILE)).unwrap().replace("\"visualPages\": 5", "\"visualPages\": 5000");
    assert!(text.contains("5000"), "{text}");
    fs::write(dir.join(presets::FILE), text).unwrap();
    let back = presets::load(&dir);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].options.visual_pages, VISUAL_PAGES_MAX);

    // saving never leaves a temporary file beside it
    presets::save(&dir, "Second", &Options::preset(Preset::Custom)).unwrap();
    let mut left: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, vec![presets::FILE.to_string()]);
}

/// A page whose graphics state is partly transparent, and one that is not.
fn transparent_sink(path: &Path, rotate_second: bool) {
    let doc = PdfDocument::from_bytes(&fixture::build(&fixture::Fixture { pages: 3, chapter_every: 0, needle_every: 0, lines: 8 })).unwrap();
    let mut p0 = doc.find_page(0).unwrap();
    let mut res = p0.get_dict("Resources").unwrap().unwrap();
    let gs = doc.new_object_from_str("<< /G1 << /Type /ExtGState /ca 0.5 /CA 0.5 >> >>").unwrap();
    res.dict_put("ExtGState", gs).unwrap();
    // a soft-masked page: the second one
    let mut p1 = doc.find_page(1).unwrap();
    let mut res1 = p1.get_dict("Resources").unwrap().unwrap();
    let gs1 = doc.new_object_from_str("<< /G2 << /Type /ExtGState /BM /Multiply >> >>").unwrap();
    res1.dict_put("ExtGState", gs1).unwrap();
    if rotate_second {
        p1.dict_put("Rotate", mupdf::pdf::PdfObject::new_int(90).unwrap()).unwrap();
    }
    let _ = &mut p0;
    doc.save(path.to_str().unwrap()).unwrap();
}

#[test]
fn flatten_turns_a_transparent_page_into_one_picture_and_leaves_the_rest() {
    let s = Scratch::new();
    let input = s.path("transparent.pdf");
    transparent_sink(&input, true);
    let before = text(&input);
    assert!(before.iter().all(|t| !t.trim().is_empty()));

    // off: nothing happens
    let mut o = Options::preset(Preset::Balanced);
    o.output = Some(s.path("off.pdf"));
    let r = run(&input, &o).unwrap();
    assert!(r.flattened.pages.is_empty());
    assert_eq!(text(&r.output), before);

    let mut o = Options::preset(Preset::Balanced);
    o.transparency.flatten = true;
    o.transparency.ppi = 150;
    o.output = Some(s.path("flat.pdf"));
    let r = run(&input, &o).unwrap();
    // page 1 flattened; page 2 uses transparency too but is rotated; page 3 has none
    assert_eq!(r.flattened.pages, vec![1], "{:?}", r.flattened);
    assert_eq!(r.flattened.skipped.len(), 1);
    assert_eq!(r.flattened.skipped[0].0, 2);
    assert!(r.warnings.iter().any(|w| w.contains("Page 2") && w.contains("not flattened")), "{:?}", r.warnings);
    let after = text(&r.output);
    assert!(after[0].trim().is_empty(), "page 1 still has text: {:?}", after[0]);
    assert_eq!(after[1], before[1]);
    assert_eq!(after[2], before[2]);
    // the picture is in the file and the page still looks like the original
    assert!(r.after.images >= 1);
    let v = r.visual.as_ref().expect("visual check ran");
    assert_eq!(v.pages_total, 3);
    println!("flatten: psnr {:.1} max {} mean {:.2}", v.psnr, v.max_diff, v.mean_diff);
    assert!(v.psnr >= 18.0, "{v:?}");
    // never worse does not undo a flatten the user asked for
    assert!(!r.kept_original);

    // the resolution is validated
    let mut o = Options::preset(Preset::Balanced);
    o.transparency.flatten = true;
    o.transparency.ppi = 5;
    assert!(matches!(run(&input, &o), Err(Error::InvalidOptions(_))));
}

#[test]
fn audit_categories_add_up_to_the_file_size_within_one_percent() {
    let s = Scratch::new();
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut names: Vec<String> = fs::read_dir(samples())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".pdf"))
        .collect();
    names.sort();
    for n in names {
        inputs.push(s.copy(&n));
    }
    let sink = s.path("sink.pdf");
    kitchen_sink(&sink);
    let fonts = s.path("fonts.pdf");
    font_sink(&fonts);
    inputs.extend([sink.clone(), fonts]);
    for p in &inputs {
        let a = match audit_file(p, None) {
            Ok(a) => a,
            // a study sample the engine cannot open on its own (needs a password): not what is tested here
            Err(Error::NeedsPassword | Error::BadPassword) => continue,
            Err(e) => panic!("{}: {e}", p.display()),
        };
        let size = fs::metadata(p).unwrap().len();
        assert_eq!(a.total, size);
        let sum: u64 = a.categories.iter().map(|c| c.bytes).sum();
        let off = (sum as f64 - size as f64).abs() / size as f64;
        assert!(off <= 0.01, "{}: categories sum to {sum}, file is {size}", p.display());
        let pct: f64 = a.categories.iter().map(|c| c.percent).sum();
        assert!((pct - 100.0).abs() <= 1.0, "{}: {pct}%", p.display());
    }

    // where the bytes of the kitchen sink are: every kind of object lands in its own category
    let a = audit_file(&sink, None).unwrap();
    for c in [Category::Forms, Category::Links, Category::Bookmarks, Category::Thumbnails, Category::EmbeddedFiles, Category::Metadata] {
        assert!(a.bytes(c) > 0, "{c:?} is empty: {:?}", a.categories);
    }
    // and discarding them takes exactly those bytes out (the audit of the result agrees)
    let mut o = Options::preset(Preset::Balanced);
    o.strip = Strip { thumbnails: true, embedded_files: true, bookmarks: true, links: true, ..Strip::default() };
    o.output = Some(s.path("slim.pdf"));
    let r = run(&sink, &o).unwrap();
    assert_eq!(r.after.bytes(Category::Thumbnails), 0);
    assert_eq!(r.after.bytes(Category::EmbeddedFiles), 0);
    assert_eq!(r.after.bytes(Category::Bookmarks), 0);
    assert_eq!(r.after.bytes(Category::Links), 0);
    assert!(r.after.bytes(Category::Forms) > 0, "fields were not asked to go");
    let sum: u64 = r.after.categories.iter().map(|c| c.bytes).sum();
    assert_eq!(sum, r.size_after);
}

#[test]
fn a_named_preset_round_trips_through_the_file() {
    let s = Scratch::new();
    let dir = s.path("data");
    assert!(presets::load(&dir).is_empty());

    let mut o = Options::preset(Preset::Custom);
    o.images.color = ImageClass { above_ppi: 200, to_ppi: 110, codec: Codec::Jpeg, quality: 42 };
    o.images.gray = ImageClass { above_ppi: 0, to_ppi: 0, codec: Codec::Flate, quality: 0 };
    o.images.mono = ImageClass { above_ppi: 600, to_ppi: 400, codec: Codec::Retain, quality: 0 };
    o.images.only_if_smaller = false;
    o.subset_fonts = true;
    o.unembed_all = true;
    o.transparency = Transparency { flatten: true, ppi: 300 };
    o.compress_streams = false;
    o.object_streams = false;
    o.garbage = 2;
    o.strip = Strip {
        metadata: true,
        thumbnails: true,
        embedded_files: true,
        javascript: true,
        bookmarks: true,
        invalid_bookmarks: true,
        form_actions: true,
        alternate_images: true,
        private_data: true,
        links: true,
        invalid_links: true,
    };
    o.visual_pages = 3;
    // what is not part of the settings: never stored
    o.password = Some("secret".into());
    o.output = Some(s.path("somewhere.pdf"));
    o.unembed_fonts = vec!["ABCDEF+Arial".into()];

    let list = presets::save(&dir, "  Web, small  ", &o).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Web, small");
    let text_on_disk = fs::read_to_string(dir.join(presets::FILE)).unwrap();
    assert!(!text_on_disk.contains("secret") && !text_on_disk.contains("somewhere"), "{text_on_disk}");
    assert!(!text_on_disk.contains("ABCDEF"), "fonts of one document are not part of a preset");

    // a fresh read (another run of the app) gives the same settings back
    let back = presets::load(&dir);
    assert_eq!(back.len(), 1);
    let mut expect = o.clone();
    expect.password = None;
    expect.output = None;
    expect.unembed_fonts = Vec::new();
    assert_eq!(back[0].options, expect);

    // and they run: the loaded options are the ones that were saved
    let input = s.path("heavy.pdf");
    image_heavy(&input, 2);
    let mut run_with = back[0].options.clone();
    run_with.output = Some(s.path("out-loaded.pdf"));
    let loaded_run = run(&input, &run_with).unwrap();
    let mut direct = expect.clone();
    direct.output = Some(s.path("out-direct.pdf"));
    let direct_run = run(&input, &direct).unwrap();
    assert_eq!(loaded_run.size_after, direct_run.size_after);

    // saving under the same name in other capitals replaces; another name adds; delete removes
    let mut o2 = Options::preset(Preset::Smallest);
    o2.preset = Preset::Custom;
    let list = presets::save(&dir, "WEB, SMALL", &o2).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "WEB, SMALL");
    assert_eq!(list[0].options.images.color.above_ppi, 108);
    let list = presets::save(&dir, "Print", &Options::preset(Preset::HighQuality)).unwrap();
    assert_eq!(list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["WEB, SMALL", "Print"]);
    let list = presets::delete(&dir, "web, small").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(presets::load(&dir)[0].name, "Print");

    // refused: empty names, settings the engine would refuse
    assert!(matches!(presets::save(&dir, "   ", &o2), Err(Error::InvalidOptions(_))));
    let mut bad = o2.clone();
    bad.images.color.above_ppi = 10;
    assert!(matches!(presets::save(&dir, "bad", &bad), Err(Error::InvalidOptions(_))));
    assert_eq!(presets::load(&dir).len(), 1);
}

#[test]
fn a_preset_saved_by_an_older_build_loads_with_todays_defaults() {
    let s = Scratch::new();
    let dir = s.path("data");
    fs::create_dir_all(&dir).unwrap();
    // no unembed, transparency, compressStreams or the newer discard options; and a field this build never had
    let old = r#"{"version":1,"presets":[{"name":"Old","options":{"preset":"custom",
        "images":{"color":{"abovePpi":150,"toPpi":100,"codec":"jpeg","quality":50},
                  "gray":{"abovePpi":150,"toPpi":100,"codec":"jpeg","quality":50},
                  "mono":{"abovePpi":300,"toPpi":200,"codec":"fax","quality":0},"onlyIfSmaller":true},
        "subsetFonts":true,"objectStreams":true,"garbage":3,
        "strip":{"metadata":true,"thumbnails":false,"embeddedFiles":false,"javascript":false,"bookmarks":false},
        "visualPages":8,"someFutureSetting":{"x":1}}},
        {"name":"Broken","options":{"garbage":9}}]}"#;
    fs::write(dir.join(presets::FILE), old).unwrap();
    let list = presets::load(&dir);
    // the unusable one is left out, not fatal
    assert_eq!(list.len(), 1);
    let o = &list[0].options;
    assert_eq!(o.images.color.quality, 50);
    assert!(o.strip.metadata && !o.strip.links && !o.strip.invalid_links && !o.strip.form_actions);
    assert!(o.compress_streams && !o.transparency.flatten && !o.unembed_all && o.unembed_fonts.is_empty());
    assert_eq!(o.garbage, 3);
    // a damaged file is an empty list, not a crash
    fs::write(dir.join(presets::FILE), b"{ not json").unwrap();
    assert!(presets::load(&dir).is_empty());
}

#[test]
fn advanced_settings_keep_the_never_worse_rule_and_the_visual_check() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    image_heavy(&input, 3);

    let mut o = Options::preset(Preset::Custom);
    o.images.color = ImageClass { above_ppi: 150, to_ppi: 100, codec: Codec::Jpeg, quality: 55 };
    o.images.gray = ImageClass { above_ppi: 150, to_ppi: 100, codec: Codec::Jpeg, quality: 55 };
    o.strip.thumbnails = true;
    o.output = Some(s.path("custom.pdf"));
    let r = run(&input, &o).unwrap();
    assert!(r.size_after < r.size_before && !r.kept_original);
    let v = r.visual.as_ref().expect("the visual check runs for Advanced settings too");
    assert!(v.psnr >= 25.0, "{v:?}");
    assert_eq!(r.after.total, r.size_after);

    // settings that cannot make the file smaller: the original bytes come back
    let packed = s.path("packed.pdf");
    fs::copy(&r.output, &packed).unwrap();
    let mut o = Options::preset(Preset::Custom);
    o.images.color = ImageClass { above_ppi: 0, to_ppi: 0, codec: Codec::Retain, quality: 0 };
    o.images.gray = o.images.color;
    o.images.mono = o.images.color;
    o.object_streams = false;
    o.compress_streams = false;
    o.garbage = 0;
    o.output = Some(s.path("again.pdf"));
    let r = run(&packed, &o).unwrap();
    assert!(r.kept_original, "{} -> {}", r.size_before, r.size_after);
    assert_eq!(fs::read(&r.output).unwrap(), fs::read(&packed).unwrap());
    assert!(r.visual.is_none());
}

#[test]
fn the_estimate_for_advanced_settings_follows_what_they_discard_and_recompress() {
    let s = Scratch::new();
    let input = s.path("heavy.pdf");
    let mut doc = PdfDocument::new();
    for i in 0..20u32 {
        let mut page = doc.new_page(Size::A4).unwrap();
        let o = InsertImageOptions::default();
        page.insert_image(&mut doc, Rect::new(36.0, 36.0, 400.0, 300.0), PageImageSource::Pixmap(&photo(1100, 800, true, i * 2 + 1)), o).unwrap();
    }
    doc.add_embedded_file("blob.bin", &vec![7u8; 400_000].iter().enumerate().map(|(i, b)| b.wrapping_mul(i as u8)).collect::<Vec<u8>>(), mupdf::pdf::EmbeddedFileOptions::new("blob.bin")).unwrap();
    doc.save(input.to_str().unwrap()).unwrap();
    let full = audit_file(&input, None).unwrap();
    assert!(full.bytes(Category::EmbeddedFiles) > 0);

    let cancel = AtomicBool::new(false);
    let mut keep = Options::preset(Preset::Custom);
    keep.images.color = ImageClass { above_ppi: 150, to_ppi: 100, codec: Codec::Jpeg, quality: 50 };
    let mut drop = keep.clone();
    drop.strip.embedded_files = true;
    let e_keep = estimate::estimate_custom(&input, None, &full, &keep, &cancel).unwrap();
    let e_drop = estimate::estimate_custom(&input, None, &full, &drop, &cancel).unwrap();
    println!("keep {} [{}..{}]  drop {} [{}..{}]  attachment {}", e_keep.bytes, e_keep.low, e_keep.high, e_drop.bytes, e_drop.low, e_drop.high, full.bytes(Category::EmbeddedFiles));
    assert!(e_drop.bytes < e_keep.bytes);
    for (e, o) in [(e_keep, &keep), (e_drop, &drop)] {
        let mut o = o.clone();
        o.output = Some(s.path(&format!("real-{}.pdf", o.strip.embedded_files)));
        let real = run(&input, &o).unwrap().size_after as f64;
        let err = (e.bytes as f64 - real) / real;
        println!("  est {} real {real} err {:+.1}%", e.bytes, err * 100.0);
        assert!(err.abs() <= 0.30, "{} vs {real}", e.bytes);
    }
}

// ---------------------------------------------------------------- invalid bookmarks

/// `outline_pdf_with` files: `/Outlines` is object 4, the one page 5, the font 3, items from 7.
fn bookmark_pdf(path: &Path, catalog: &str, root: &str, items: &[&str]) {
    let items: Vec<String> = items.iter().map(|i| i.to_string()).collect();
    fs::write(path, crate::outline::tests::outline_pdf_with(catalog, root, &items)).unwrap();
}

/// The titles of an outline, children in brackets: `A, B[B1, B2], C`.
fn outline_text(items: &[crate::outline::Bookmark]) -> String {
    items
        .iter()
        .map(|b| if b.children.is_empty() { b.title.clone() } else { format!("{}[{}]", b.title, outline_text(&b.children)) })
        .collect::<Vec<_>>()
        .join(", ")
}

fn outline_of(p: &Path) -> crate::outline::Bookmarks {
    crate::outline::read(&open_pdf(p, None).unwrap(), 0).unwrap()
}

fn invalid_bookmarks_only() -> Options {
    let mut o = Options::preset(Preset::Custom);
    o.strip.invalid_bookmarks = true;
    o
}

const NAMES: &str = "/Names << /Dests << /Names [(good) [5 0 R /Fit]] >> >>";

#[test]
fn only_bookmarks_that_lead_nowhere_are_dropped_and_their_working_children_move_up() {
    let s = Scratch::new();
    let input = s.path("outline.pdf");
    let root = "<< /Type /Outlines /First 7 0 R /Last 22 0 R /Count 14 >>";
    let items = [
        "<< /Title (A) /Parent 4 0 R /Dest [5 0 R /Fit] /Next 8 0 R >>",
        // a page reference to an object that is not in the page tree; its children: one works, one does not
        "<< /Title (B) /Parent 4 0 R /Dest [3 0 R /Fit] /First 9 0 R /Last 10 0 R /Count 3 /Next 11 0 R >>",
        "<< /Title (B1) /Parent 8 0 R /Dest (good) /Next 10 0 R >>",
        "<< /Title (B2) /Parent 8 0 R /Dest (gone) /First 12 0 R /Last 12 0 R /Count 1 >>",
        "<< /Title (C) /Parent 4 0 R /Dest (nowhere) /Next 13 0 R >>",
        // under two removed levels: ends up at the top
        "<< /Title (B2a) /Parent 10 0 R /Dest [5 0 R /Fit] >>",
        "<< /Title (D) /Parent 4 0 R /Dest (good) /Next 14 0 R >>",
        "<< /Title (E) /Parent 4 0 R /A << /S /GoTo /D (nope) >> /Next 15 0 R >>",
        "<< /Title (F) /Parent 4 0 R /A << /S /URI /URI (http://example.com) >> /Next 16 0 R >>",
        // a heading: no destination at all
        "<< /Title (G) /Parent 4 0 R /Next 17 0 R >>",
        "<< /Title (H) /Parent 4 0 R /Dest [/Fit] /Next 18 0 R >>",
        "<< /Title (I) /Parent 4 0 R /A << /S /GoTo /D 42 >> /Next 19 0 R >>",
        "<< /Title (J) /Parent 4 0 R /A << /S /GoToR /F (x.pdf) /D [0 /Fit] >> /Next 20 0 R >>",
        // an integer page: kept while it is a page of this document, dropped when it is not
        "<< /Title (K) /Parent 4 0 R /A << /S /GoTo /D [7 /Fit] >> /Next 21 0 R >>",
        "<< /Title (L) /Parent 4 0 R /Dest [0 /Fit] /Next 22 0 R >>",
        // an object that does not exist
        "<< /Title (M) /Parent 4 0 R /Dest [99 0 R /Fit] >>",
    ];
    bookmark_pdf(&input, NAMES, root, &items);
    assert_eq!(
        outline_text(&outline_of(&input).items),
        "A, B[B1, B2[B2a]], C, D, E, F, G, H, I, J, K, L, M"
    );

    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 8, "{:?}", r.stripped);
    assert!(r.stripped.bookmarks_unchecked.is_none() && !r.stripped.bookmarks && r.stripped.any() && !r.kept_original);
    assert_eq!(outline_text(&outline_of(&r.output).items), "A, B1, B2a, D, F, G, J, L");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    // the links and counts are right in the file: First/Last/Prev/Next/Parent, and /Count on the root
    let doc = open_pdf(&r.output, None).unwrap();
    let root = doc.catalog().unwrap().get_dict("Outlines").unwrap().unwrap();
    let mut ids = Vec::new();
    let mut cur = root.get_dict("First").unwrap();
    while let Some(it) = cur {
        ids.push(xref_of(&it));
        assert_eq!(it.get_dict("Parent").unwrap().map(|p| xref_of(&p)), Some(xref_of(&root)));
        let prev = it.get_dict("Prev").unwrap().map(|p| xref_of(&p));
        assert_eq!(prev, if ids.len() > 1 { Some(ids[ids.len() - 2]) } else { None });
        cur = it.get_dict("Next").unwrap();
    }
    assert_eq!(ids.len(), 8);
    assert_eq!(root.get_dict("Last").unwrap().map(|l| xref_of(&l)), ids.last().copied());
    assert_eq!(root.get_dict("Count").unwrap().unwrap().as_int().unwrap(), 8);
    // what is left does not point at anything that went
    for it in &outline_of(&r.output).items {
        assert!(!matches!(it.dest, crate::outline::Dest::Broken { .. }) || ["J", "L"].contains(&it.title.as_str()), "{}", it.title);
    }

    // the space audit shows the outline shrinking
    assert!(r.after.bytes(Category::Bookmarks) < r.before.bytes(Category::Bookmarks));

    // off by default, in every preset
    for p in [Preset::Smallest, Preset::Balanced, Preset::HighQuality, Preset::Custom] {
        assert!(!Options::preset(p).strip.invalid_bookmarks, "{p:?}");
    }
}

#[test]
fn a_nested_removed_parent_gives_its_children_to_the_nearest_kept_ancestor() {
    let s = Scratch::new();
    let input = s.path("nested.pdf");
    // P (kept, open) > X (gone) > [Y (kept) > Z (gone) > W (kept)], and V (kept) after X
    let root = "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 6 >>";
    let items = [
        "<< /Title (P) /Parent 4 0 R /Dest (good) /First 8 0 R /Last 12 0 R /Count 5 >>",
        "<< /Title (X) /Parent 7 0 R /Dest (gone) /First 9 0 R /Last 9 0 R /Count 3 /Next 12 0 R >>",
        "<< /Title (Y) /Parent 8 0 R /Dest (good) /First 10 0 R /Last 10 0 R /Count -2 >>",
        "<< /Title (Z) /Parent 9 0 R /Dest (gone) /First 11 0 R /Last 11 0 R /Count 1 >>",
        "<< /Title (W) /Parent 10 0 R /Dest (good) >>",
        "<< /Title (V) /Parent 7 0 R /Dest (good) /Prev 8 0 R >>",
    ];
    bookmark_pdf(&input, NAMES, root, &items);
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 2);
    // Y stays closed (its sign), P stays open, and the counts are what the viewer needs
    assert_eq!(outline_text(&outline_of(&r.output).items), "P[Y[W], V]");
    let doc = open_pdf(&r.output, None).unwrap();
    let p = doc.catalog().unwrap().get_dict("Outlines").unwrap().unwrap().get_dict("First").unwrap().unwrap();
    assert_eq!(p.get_dict("Title").unwrap().unwrap().as_string().unwrap(), "P");
    assert_eq!(p.get_dict("Count").unwrap().unwrap().as_int().unwrap(), 2, "P shows Y and V; Y is closed, so W is not counted");
}

#[test]
fn an_outline_that_cannot_be_judged_is_left_as_it_is_and_the_run_goes_on() {
    let s = Scratch::new();
    // a chain that loops back on itself: the walk ends there and the loop is mended
    let cyclic = s.path("cyclic.pdf");
    bookmark_pdf(
        &cyclic,
        NAMES,
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
        &[
            "<< /Title (A) /Parent 4 0 R /Dest (good) /Next 8 0 R >>",
            "<< /Title (B) /Parent 4 0 R /Dest (gone) /Prev 7 0 R /Next 7 0 R >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("cyclic-out.pdf"));
    let r = run(&cyclic, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1);
    assert_eq!(outline_text(&outline_of(&r.output).items), "A");

    // two parents that share a child: not relinked, said so, nothing else refused
    let shared = s.path("shared.pdf");
    bookmark_pdf(
        &shared,
        NAMES,
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 3 >>",
        &[
            "<< /Title (A) /Parent 4 0 R /Dest (gone) /First 9 0 R /Last 9 0 R /Count 1 /Next 8 0 R >>",
            "<< /Title (B) /Parent 4 0 R /Dest (good) /First 9 0 R /Last 9 0 R /Count 1 /Prev 7 0 R >>",
            "<< /Title (C) /Parent 8 0 R /Dest (good) >>",
        ],
    );
    o.output = Some(s.path("shared-out.pdf"));
    let r = run(&shared, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 0);
    assert!(r.stripped.bookmarks_unchecked.is_some());
    assert!(r.warnings.iter().any(|w| w.contains("Invalid bookmarks were not removed")), "{:?}", r.warnings);
    // the walk gives C to the first parent that reaches it; nothing moved
    assert_eq!(outline_text(&outline_of(&r.output).items), "A[C], B");
    assert_eq!(outline_text(&outline_of(&shared).items), "A[C], B");
}

#[test]
fn a_certified_file_is_refused_and_a_signed_one_warns_like_any_compress() {
    let s = Scratch::new();
    let root = "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>";
    let items = ["<< /Title (A) /Parent 4 0 R /Dest (gone) >>"];
    let certified = format!(
        "{NAMES} /Perms << /DocMDP << /Type /Sig /Reference [ {} ] >> >>",
        crate::outline::tests::doc_mdp("/TransformParams << /P 1 >>")
    );
    let input = s.path("certified.pdf");
    bookmark_pdf(&input, &certified, root, &items);
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("never.pdf"));
    match run(&input, &o) {
        Err(Error::Refused(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(!s.path("never.pdf").exists());

    let signed = s.path("signed.pdf");
    bookmark_pdf(&signed, &format!("{NAMES} /AcroForm << /Fields [] /SigFlags 3 >>"), root, &items);
    o.output = Some(s.path("signed-out.pdf"));
    let r = run(&signed, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1);
    assert!(r.warnings.iter().any(|w| w.contains("signed")), "{:?}", r.warnings);
    assert!(outline_of(&r.output).items.is_empty());
}

/// MuPDF reads `/Dests` alone when the catalog has one, but another viewer may read the tree: a name either has stays.
#[test]
fn a_bookmark_whose_name_is_only_in_the_tree_is_kept_next_to_a_dests_dictionary() {
    let s = Scratch::new();
    let input = s.path("both-sources.pdf");
    let catalog = "/Dests << /old [5 0 R /Fit] >> /Names << /Dests << /Names [(treeonly) [5 0 R /Fit]] >> >>";
    bookmark_pdf(
        &input,
        catalog,
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 3 >>",
        &[
            "<< /Title (A) /Parent 4 0 R /Dest (treeonly) /Next 8 0 R >>",
            "<< /Title (B) /Parent 4 0 R /Dest (gone) /Prev 7 0 R /Next 9 0 R >>",
            "<< /Title (C) /Parent 4 0 R /Dest /old /Prev 8 0 R >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1);
    assert_eq!(outline_text(&outline_of(&r.output).items), "A, C");
}

#[test]
fn dropping_all_bookmarks_leaves_nothing_for_the_invalid_option_to_count() {
    let s = Scratch::new();
    let input = s.path("both.pdf");
    bookmark_pdf(&input, NAMES, "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>", &["<< /Title (A) /Parent 4 0 R /Dest (gone) >>"]);
    let mut o = invalid_bookmarks_only();
    o.strip.bookmarks = true;
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert!(r.stripped.bookmarks);
    assert_eq!(r.stripped.invalid_bookmarks, 0);
    assert!(r.stripped.bookmarks_unchecked.is_none());
    assert!(!has_entry(&r.output, "Outlines"));
}

#[test]
fn the_invalid_bookmarks_option_leaves_links_and_everything_else_alone() {
    let s = Scratch::new();
    let input = s.path("sink.pdf");
    kitchen_sink(&input);
    let base = probe(&input);
    assert!(base.links.iter().any(|l| !l.starts_with("ok-")), "the fixture has links the invalid-links pass would drop: {base:?}");
    let mut o = keep_images();
    o.strip.invalid_bookmarks = true;
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!((r.stripped.invalid_bookmarks, r.stripped.invalid_links, r.stripped.links), (0, 0, 0), "{:?}", r.stripped);
    assert!(r.stripped.bookmarks_unchecked.is_none());
    assert_eq!(probe(&r.output), base);
}

#[test]
fn a_destination_name_that_is_not_utf8_is_looked_up_by_its_bytes() {
    let s = Scratch::new();
    let input = s.path("latin1.pdf");
    // /Dests keeps a key with the byte E9; the bookmark names it, another names a key that is not there: a name that
    // is not UTF-8 may be read lossily by another viewer through the tree, so it is not judged and stays
    bookmark_pdf(
        &input,
        "/Dests << /caf#E9 [5 0 R /Fit] >>",
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
        &[
            "<< /Title (Cafe) /Parent 4 0 R /Dest /caf#E9 /Next 8 0 R >>",
            "<< /Title (Other) /Parent 4 0 R /Dest /caf#E8 /Prev 7 0 R >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 0, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "Cafe, Other");
}

#[test]
fn a_goto_without_a_destination_and_a_destination_dictionary_are_judged_like_the_bookmarks_panel_does() {
    let s = Scratch::new();
    let input = s.path("dicts.pdf");
    bookmark_pdf(
        &input,
        NAMES,
        "<< /Type /Outlines /First 7 0 R /Last 12 0 R /Count 6 >>",
        &[
            // no /D: the panel shows a heading, so it is not judged
            "<< /Title (A) /Parent 4 0 R /A << /S /GoTo >> /Next 8 0 R >>",
            // a destination written as a dictionary holding /D
            "<< /Title (B) /Parent 4 0 R /Dest << /D [5 0 R /Fit] >> /Next 9 0 R >>",
            "<< /Title (C) /Parent 4 0 R /Dest << /D (gone) >> /Next 10 0 R >>",
            "<< /Title (D) /Parent 4 0 R /A << /S /GoTo /D null >> /Next 11 0 R >>",
            "<< /Title (E) /Parent 4 0 R /Dest << /D 42 >> /Next 12 0 R >>",
            // a dictionary that holds itself: ends, and is invalid
            "<< /Title (F) /Parent 4 0 R /Dest 13 0 R >>",
            "<< /D 13 0 R >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 3, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "A, B, D");
}

/// MuPDF's viewer re-encodes a string destination before it looks it up in `/Dests` (`pdf_new_text_string`): a
/// UTF-16BE string for a key that is plain ASCII, or whose PDFDocEncoding bytes are the key, still leads there.
#[test]
fn a_utf16_string_destination_is_found_in_the_old_dests_dictionary_by_its_text() {
    let s = Scratch::new();
    let input = s.path("utf16.pdf");
    // /Dests keys: c and caf\351; the strings are UTF-16BE "c", "caf\351", and "gone" (defined nowhere)
    bookmark_pdf(
        &input,
        "/Dests << /c [5 0 R /Fit] /caf#E9 [5 0 R /Fit] >>",
        "<< /Type /Outlines /First 7 0 R /Last 10 0 R /Count 4 >>",
        &[
            "<< /Title (C) /Parent 4 0 R /Dest <FEFF0063> /Next 8 0 R >>",
            "<< /Title (Cafe) /Parent 4 0 R /Dest <FEFF00630061006600E9> /Prev 7 0 R /Next 9 0 R >>",
            "<< /Title (Gone) /Parent 4 0 R /Dest <FEFF0067006F006E0065> /Prev 8 0 R /Next 10 0 R >>",
            "<< /Title (Action) /Parent 4 0 R /A << /S /GoTo /D <FEFF0063> >> /Prev 9 0 R >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "C, Cafe, Action");

    let links = s.path("links.pdf");
    let link = |rest: &str| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] {rest} >>");
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /Dests << /c [4 0 R /Fit] /caf#E9 [4 0 R /Fit] >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>".to_string(),
        link("/Dest <FEFF0063>"),
        link("/A << /S /GoTo /D <FEFF00630061006600E9> >>"),
        link("/Dest <FEFF0067006F006E0065>"),
        link("/Dest << /D <FEFF0063> >>"),
    ];
    fs::write(&links, crate::outline::tests::raw_pdf(&objs)).unwrap();
    let mut o = Options::preset(Preset::Custom);
    o.strip.invalid_links = true;
    o.output = Some(s.path("links-out.pdf"));
    let r = run(&links, &o).unwrap();
    assert_eq!(r.stripped.invalid_links, 1, "{:?}", r.stripped);
    let doc = open_pdf(&r.output, None).unwrap();
    assert_eq!(doc.find_page(0).unwrap().get_dict("Annots").unwrap().unwrap().len().unwrap(), 3);
}

/// The viewer's key is MuPDF's pick, but the optimizer only keeps more: a bookmark whose string is the bytes of a
/// `/Dests` key, or has PDFDocEncoding bytes that are one, stays, and so does one with no faithful text.
#[test]
fn a_string_destination_is_kept_when_any_spelling_of_it_is_a_dests_key() {
    let s = Scratch::new();
    let input = s.path("spellings.pdf");
    // /Dests keys: BOM + A (the bytes of the first string) and caf\351; the third string is a lone surrogate
    bookmark_pdf(
        &input,
        "/Dests << /#EF#BB#BFA [5 0 R /Fit] /caf#E9 [5 0 R /Fit] >>",
        "<< /Type /Outlines /First 7 0 R /Last 10 0 R /Count 4 >>",
        &[
            "<< /Title (Bytes) /Parent 4 0 R /Dest <EFBBBF41> /Next 8 0 R >>",
            "<< /Title (Cafe) /Parent 4 0 R /Dest <FEFF00630061006600E9> /Prev 7 0 R /Next 9 0 R >>",
            "<< /Title (Lone) /Parent 4 0 R /Dest <FEFFD800> /Prev 8 0 R /Next 10 0 R >>",
            "<< /Title (Gone) /Parent 4 0 R /Dest <FEFF0067006F006E0065> /Prev 9 0 R >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "Bytes, Cafe, Lone");
}

/// A name destination is looked up the viewer's way too, so a name whose only `/Dests` key is the one the viewer forms
/// from its text (FE FF 03 A9 for U+03A9) stays; the optimizer keeps the name's own spelling as well.
#[test]
fn a_non_ascii_name_destination_is_kept_by_its_own_key_or_the_viewers() {
    let s = Scratch::new();
    let input = s.path("names.pdf");
    bookmark_pdf(
        &input,
        "/Dests << /#FE#FF#03#A9 [5 0 R /Fit] /#CE#A8 [5 0 R /Fit] >>",
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 3 >>",
        &[
            "<< /Title (Viewer) /Parent 4 0 R /Dest /#CE#A9 /Next 8 0 R >>",
            "<< /Title (Own) /Parent 4 0 R /Dest /#CE#A8 /Prev 7 0 R /Next 9 0 R >>",
            "<< /Title (Gone) /Parent 4 0 R /Dest /#CE#AA /Prev 8 0 R >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "Viewer, Own");
}

#[test]
fn invalid_links_accept_a_destination_dictionary_and_still_drop_a_goto_without_one() {
    let s = Scratch::new();
    let input = s.path("links.pdf");
    let link = |rest: &str| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] {rest} >>");
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /Dests << /ok [4 0 R /Fit] >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>".to_string(),
        link("/Dest << /D [4 0 R /Fit] >>"),
        link("/Dest << /D (gone) >>"),
        link("/Dest << /D /ok >>"),
        link("/A << /S /GoTo >>"),
    ];
    fs::write(&input, crate::outline::tests::raw_pdf(&objs)).unwrap();
    let mut o = Options::preset(Preset::Custom);
    o.strip.invalid_links = true;
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_links, 2, "{:?}", r.stripped);
    let doc = open_pdf(&r.output, None).unwrap();
    let annots = doc.find_page(0).unwrap().get_dict("Annots").unwrap().unwrap();
    assert_eq!(annots.len().unwrap(), 2);
}

#[test]
fn a_destination_name_held_by_an_indirect_object_is_still_looked_up() {
    let s = Scratch::new();
    let input = s.path("indirect.pdf");
    // objects 10 and 11 are the names /good (defined) and /nope (not); items use them through the reference
    bookmark_pdf(
        &input,
        "/Dests << /good [5 0 R /Fit] >>",
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 4 >>",
        &[
            "<< /Title (A) /Parent 4 0 R /Dest 10 0 R /Next 8 0 R >>",
            "<< /Title (B) /Parent 4 0 R /Dest 11 0 R /Next 9 0 R >>",
            "<< /Title (C) /Parent 4 0 R /A << /S /GoTo /D 10 0 R >> /Next 12 0 R >>",
            "/good",
            "/nope",
            // a dictionary whose /D is the indirect name
            "<< /Title (D) /Parent 4 0 R /Dest << /D 10 0 R >> >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "A, C, D");

    // and as a link's destination or its GoTo /D
    let links = s.path("links.pdf");
    let link = |rest: &str| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] {rest} >>");
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /Dests << /ok [4 0 R /Fit] >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 6 0 R 7 0 R] >>".to_string(),
        link("/A << /S /GoTo /D 8 0 R >>"),
        link("/Dest 8 0 R"),
        link("/Dest 9 0 R"),
        "/ok".to_string(),
        "/nope".to_string(),
    ];
    fs::write(&links, crate::outline::tests::raw_pdf(&objs)).unwrap();
    let mut o = Options::preset(Preset::Custom);
    o.strip.invalid_links = true;
    o.output = Some(s.path("links-out.pdf"));
    let r = run(&links, &o).unwrap();
    assert_eq!(r.stripped.invalid_links, 1, "{:?}", r.stripped);
    let doc = open_pdf(&r.output, None).unwrap();
    assert_eq!(doc.find_page(0).unwrap().get_dict("Annots").unwrap().unwrap().len().unwrap(), 2);
}

#[test]
fn a_string_destination_is_decoded_like_mupdf_does_and_one_that_cannot_be_judged_is_kept() {
    let s = Scratch::new();
    // the name tree has the keys "caf\351" (PDFDocEncoding) and U+00E9 U+00E9 (UTF-16BE)
    let tree = "/Names << /Dests << /Names [(caf\\351) [5 0 R /Fit] <FEFF00E900E9> [5 0 R /Fit]] >> >>";
    let input = s.path("tree.pdf");
    bookmark_pdf(
        &input,
        tree,
        "<< /Type /Outlines /First 7 0 R /Last 11 0 R /Count 5 >>",
        &[
            "<< /Title (A) /Parent 4 0 R /Dest <FEFF00E900E9> /Next 8 0 R >>",
            "<< /Title (B) /Parent 4 0 R /Dest (caf\\351) /Next 9 0 R >>",
            // a clean string that is not in the tree, in either encoding
            "<< /Title (C) /Parent 4 0 R /Dest (gone) /Next 10 0 R >>",
            "<< /Title (D) /Parent 4 0 R /Dest <FEFF0078> /Next 11 0 R >>",
            // a lone surrogate does not decode cleanly: not judged, kept
            "<< /Title (E) /Parent 4 0 R /A << /S /GoTo /D <FEFFD800> >> >>",
        ],
    );
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("out.pdf"));
    let r = run(&input, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 2, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "A, B, E");

    // the old /Dests dictionary is keyed by the string's own bytes, which are not UTF-8 here
    let old = s.path("old.pdf");
    bookmark_pdf(
        &old,
        "/Dests << /caf#E9 [5 0 R /Fit] >>",
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
        &[
            "<< /Title (Cafe) /Parent 4 0 R /Dest (caf\\351) /Next 8 0 R >>",
            "<< /Title (Cag) /Parent 4 0 R /Dest (cag\\351) /Prev 7 0 R >>",
        ],
    );
    o.output = Some(s.path("old-out.pdf"));
    let r = run(&old, &o).unwrap();
    assert_eq!(r.stripped.invalid_bookmarks, 1, "{:?}", r.stripped);
    assert_eq!(outline_text(&outline_of(&r.output).items), "Cafe");

    // and the same for a link's destination
    let links = s.path("links.pdf");
    let link = |rest: &str| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] {rest} >>");
    let objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {} >>", tree.replace("5 0 R", "4 0 R")),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>".to_string(),
        link("/Dest <FEFF00E900E9>"),
        link("/A << /S /GoTo /D (caf\\351) >>"),
        link("/Dest (gone)"),
        link("/Dest <FEFFD800>"),
    ];
    fs::write(&links, crate::outline::tests::raw_pdf(&objs)).unwrap();
    let mut o = Options::preset(Preset::Custom);
    o.strip.invalid_links = true;
    o.output = Some(s.path("links-out.pdf"));
    let r = run(&links, &o).unwrap();
    assert_eq!(r.stripped.invalid_links, 1, "{:?}", r.stripped);
    let doc = open_pdf(&r.output, None).unwrap();
    assert_eq!(doc.find_page(0).unwrap().get_dict("Annots").unwrap().unwrap().len().unwrap(), 3);
}

/// Bookmarks and then links, each with the destinations `dests` (strings, in order) against the name tree `tree`
/// (its page is object 5 for bookmarks, 4 for links). Returns the surviving bookmark titles (A, B, ...) and the
/// number of surviving link annotations.
fn string_dests_judged(s: &Scratch, tree: &str, dests: &[&str]) -> (String, usize) {
    let titles = ["A", "B", "C", "D", "E", "F"];
    let n = dests.len();
    let items: Vec<String> = dests
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let next = if i + 1 < n { format!(" /Next {} 0 R", 8 + i) } else { String::new() };
            format!("<< /Title ({}) /Parent 4 0 R /Dest {d}{next} >>", titles[i])
        })
        .collect();
    let input = s.path("b.pdf");
    let root = format!("<< /Type /Outlines /First 7 0 R /Last {} 0 R /Count {n} >>", 6 + n);
    fs::write(&input, crate::outline::tests::outline_pdf_with(tree, &root, &items)).unwrap();
    let mut o = invalid_bookmarks_only();
    o.output = Some(s.path("b-out.pdf"));
    let r = run(&input, &o).unwrap();
    let kept = outline_text(&outline_of(&r.output).items);

    let links = s.path("l.pdf");
    let refs: Vec<String> = (5..5 + n).map(|i| format!("{i} 0 R")).collect();
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {} >>", tree.replace("5 0 R", "4 0 R")),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{}] >>", refs.join(" ")),
    ];
    objs.extend(dests.iter().map(|d| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest {d} >>")));
    fs::write(&links, crate::outline::tests::raw_pdf(&objs)).unwrap();
    let mut o = Options::preset(Preset::Custom);
    o.strip.invalid_links = true;
    o.output = Some(s.path("l-out.pdf"));
    let r = run(&links, &o).unwrap();
    let doc = open_pdf(&r.output, None).unwrap();
    let left = doc.find_page(0).unwrap().get_dict("Annots").unwrap().unwrap().len().unwrap();
    (kept, left)
}

#[test]
fn an_unmarked_utf8_string_destination_is_read_like_mupdf_does_before_pdfdocencoding() {
    let s = Scratch::new();
    // the key is "caf\u{e9}" stored as UTF-8 (C3 A9) with no byte order mark: MuPDF's text for it is "caf\u{e9}",
    // not the PDFDocEncoding "caf\u{c3}\u{a9}"
    let tree = "/Names << /Dests << /Names [(caf\\303\\251) [5 0 R /Fit]] >> >>";
    let (kept, links) = string_dests_judged(
        &s,
        tree,
        &[
            "(caf\\303\\251)",
            // clean and not in the tree
            "(cag\\303\\251)",
            // C0 80 is an overlong NUL: MuPDF reads it as UTF-8, Rust does not, so it is not judged
            "(\\300\\200)",
        ],
    );
    assert_eq!(kept, "A, C");
    assert_eq!(links, 2);
}

#[test]
fn a_string_destination_that_mupdf_cuts_short_or_skips_is_not_judged() {
    let s = Scratch::new();
    // "Mart\u{ed}" in UTF-8 ends in the byte 0xAD, which is only a continuation byte there
    let tree = "/Names << /Dests << /Names [(Mart\\303\\255) [5 0 R /Fit]] >> >>";
    let (kept, links) = string_dests_judged(
        &s,
        tree,
        &[
            "(Mart\\303\\255)",
            "(Mart\\303\\255x)",
            // not UTF-8, so PDFDocEncoding, where 0x7F is NUL and ends the key
            "(\\351\\177)",
            // 0xAD alone is the same
            "(a\\255b)",
            // U+001B is a language escape that MuPDF skips
            "<FEFF0061001B0062>",
            // a byte order mark: U+00AD is an ordinary character, clean and missing
            "<FEFF00AD>",
        ],
    );
    assert_eq!(kept, "A, C, D, E");
    assert_eq!(links, 4);
}
